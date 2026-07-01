use rta::{RTA, Rta, RtaCfg};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const MOD_ID: u8 = 0x00;

#[repr(C)]
#[repr(align(8))]
#[derive(Debug, Default, Clone, Copy, RTA, PartialEq)]
struct Type {
    a: u32,
    b: u32,
}

#[inline]
fn prep_init(copies: usize) -> (std::path::PathBuf, RtaCfg) {
    let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
    let cfg = RtaCfg { module_id: MOD_ID, path: path.clone(), copies_on_disk: copies };

    (path, cfg)
}

mod new {
    use super::*;

    #[test]
    fn ok_new() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        assert_eq!(unsafe { rta.read() }, Type::default());
    }

    #[test]
    fn ok_existing() {
        let (_path, cfg) = prep_init(0x0A);

        {
            let rta = Rta::<Type>::new(cfg.clone()).unwrap();

            unsafe {
                let ticket = rta
                    .write(|t| {
                        t.a = 0x0A;
                        t.b = 0x14;
                    })
                    .unwrap();

                ticket.wait().unwrap();
            }
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0x0A, b: 0x14 });
    }

    #[test]
    fn err_drop_type() {
        #[repr(C, align(8))]
        #[derive(Default, Clone)]
        struct DropType(Vec<u8>);

        unsafe impl RTA for DropType {
            const HASH: u64 = 1;
            const SIZE: usize = std::mem::size_of::<Self>();
        }

        let (_path, cfg) = prep_init(0x0A);
        assert!(Rta::<DropType>::new(cfg).is_err());
    }

    #[test]
    fn err_zero_sized() {
        #[repr(C, align(8))]
        #[derive(Default, Clone, Copy)]
        struct Zst;

        unsafe impl RTA for Zst {
            const HASH: u64 = 1;
            const SIZE: usize = 0;
        }

        let (_path, cfg) = prep_init(0x0A);
        assert!(Rta::<Zst>::new(cfg).is_err());
    }

    #[test]
    fn err_bad_alignment() {
        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct BadAlign {
            value: u32,
        }

        unsafe impl RTA for BadAlign {
            const HASH: u64 = 1;
            const SIZE: usize = std::mem::size_of::<Self>();
        }

        let (_path, cfg) = prep_init(0x0A);
        assert!(Rta::<BadAlign>::new(cfg).is_err());
    }

    #[test]
    fn err_bad_size() {
        #[repr(C)]
        #[derive(Default, Clone, Copy)]
        struct BadSize {
            a: u16,
            b: u16,
        }

        unsafe impl RTA for BadSize {
            const HASH: u64 = 1;
            const SIZE: usize = std::mem::size_of::<Self>();
        }

        let (_path, cfg) = prep_init(0x0A);
        assert!(Rta::<BadSize>::new(cfg).is_err());
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn err_invalid_cfg() {
        let (path, _) = prep_init(0x0A);
        let cfg = RtaCfg { module_id: MOD_ID, copies_on_disk: 0, path: path };
        let _ = Rta::<Type>::new(cfg);
    }
}

mod recovery {
    use super::*;

    #[test]
    fn ok_single_restart() {
        let (_path, cfg) = prep_init(0x0A);

        {
            let rta = Rta::<Type>::new(cfg.clone()).unwrap();
            unsafe {
                rta.write(|t| {
                    t.a = 0x0A;
                    t.b = 0x14;
                })
                .unwrap()
                .wait()
                .unwrap();
            }
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0x0A, b: 0x14 });
    }

    #[test]
    fn ok_multiple_restarts() {
        let (_path, cfg) = prep_init(0x0A);

        for i in 0..0x0A {
            let rta = Rta::<Type>::new(cfg.clone()).unwrap();
            unsafe {
                rta.write(|t| {
                    t.a = i;
                    t.b = i + 1;
                })
                .unwrap()
                .wait()
                .unwrap();
            }
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 9, b: 0x0A });
    }

    #[test]
    fn ok_latest_value_restored() {
        let (_path, cfg) = prep_init(0x0A);

        {
            let rta = Rta::<Type>::new(cfg.clone()).unwrap();
            for i in 0..0x20 {
                unsafe {
                    rta.write(|t| {
                        t.a = i;
                        t.b = i * 2;
                    })
                    .unwrap()
                    .wait()
                    .unwrap();
                }
            }
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0x1F, b: 0x3E });
    }

    #[test]
    fn ok_wraparound_recovery() {
        let (_path, cfg) = prep_init(2);

        {
            let rta = Rta::<Type>::new(cfg.clone()).unwrap();
            for i in 0..0x64 {
                unsafe {
                    rta.write(|t| {
                        t.a = i;
                        t.b = i;
                    })
                    .unwrap()
                    .wait()
                    .unwrap();
                }
            }
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0x63, b: 0x63 });
    }
}

mod delete {
    use super::*;

    #[test]
    fn ok_delete() {
        let (path, cfg) = prep_init(0x0A);
        let mut rta = Rta::<Type>::new(cfg).unwrap();

        rta.delete().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn ok_delete_then_new() {
        let (_path, cfg) = prep_init(0x0A);

        {
            let mut rta = Rta::<Type>::new(cfg.clone()).unwrap();
            unsafe {
                rta.write(|t| {
                    t.a = 0x7B;
                    t.b = 0x1C8;
                })
                .unwrap()
                .wait()
                .unwrap();
            }

            rta.delete().unwrap();
        }

        let rta = Rta::<Type>::new(cfg).unwrap();
        assert_eq!(unsafe { rta.read() }, Type::default());
    }

    #[test]
    fn err_delete_twice() {
        let (_path, cfg) = prep_init(0x0A);
        let mut rta = Rta::<Type>::new(cfg).unwrap();

        rta.delete().unwrap();

        let res = rta.delete();
        assert!(res.is_err());
    }
}

mod write_read {
    use super::*;

    #[test]
    fn ok_read_default() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        assert_eq!(unsafe { rta.read() }, Type::default());
    }

    #[test]
    fn ok_read_reads_the_latest_write() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        unsafe {
            rta.write(|t| {
                t.a = 0xAA;
                t.b = 0xBB;
            })
            .unwrap()
            .wait()
            .unwrap();
        }

        assert_eq!(unsafe { rta.read() }, Type { a: 0xAA, b: 0xBB });
    }

    #[test]
    fn ok_read_after_many_writes() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        let mut ticket = None;
        for i in 0..0x1000 {
            ticket = Some(
                unsafe {
                    rta.write(|t| {
                        t.a = i;
                        t.b = !i;
                    })
                }
                .unwrap(),
            );
        }

        ticket.unwrap().wait().unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0xFFF, b: !0xFFF });
    }

    #[test]
    fn ok_reads_during_concurrent_writes() {
        let (_path, cfg) = prep_init(4);

        let done = Arc::new(AtomicBool::new(false));
        let rta = Arc::new(Rta::<Type>::new(cfg).unwrap());

        let writer = {
            let rta = Arc::clone(&rta);
            let done = Arc::clone(&done);

            std::thread::spawn(move || {
                let mut ticket = None;
                for i in 0..0x1000 {
                    ticket = Some(
                        unsafe {
                            rta.write(|t| {
                                t.a = i;
                                t.b = i;
                            })
                        }
                        .unwrap(),
                    );
                }

                ticket.unwrap().wait().unwrap();
                done.store(true, Ordering::Release);
            })
        };

        // NOTE: Determining the current state of `Type` is very tough here
        while !done.load(Ordering::Acquire) {
            let _ = unsafe { rta.read() };
        }

        writer.join().unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0xFFF, b: 0xFFF });
    }

    #[test]
    fn ok_write_read_single() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        unsafe {
            rta.write(|t| {
                t.a = 0x10;
                t.b = 0x20;
            })
            .unwrap()
            .wait()
            .unwrap();
        }

        assert_eq!(unsafe { rta.read() }, Type { a: 0x10, b: 0x20 });
    }

    #[test]
    fn ok_multiple_sequential() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        let mut ticket = None;
        for i in 0..0x1000 {
            ticket = Some(
                unsafe {
                    rta.write(|t| {
                        t.a = i;
                        t.b = i + 1;
                    })
                }
                .unwrap(),
            );
        }

        ticket.unwrap().wait().unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0xFFF, b: 0x1000 });
    }

    #[test]
    fn ok_returns_ack_ticket() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).unwrap();

        unsafe {
            let ticket = rta
                .write(|t| {
                    t.a = 0x2A;
                })
                .unwrap();

            ticket.wait().unwrap();
        }

        assert_eq!(unsafe { rta.read() }, Type { a: 0x2A, b: 0 });
    }

    #[test]
    fn ok_concurrent_writes() {
        use std::sync::Arc;

        const THREADS: usize = 4;
        const WRITES: usize = 0x1000;

        let (_path, cfg) = prep_init(0x40);
        let rta = Arc::new(Rta::<Type>::new(cfg).unwrap());

        let mut handles = Vec::new();
        for tid in 0..THREADS {
            let rta = Arc::clone(&rta);

            handles.push(std::thread::spawn(move || {
                let mut ticket = None;

                for i in 0..WRITES {
                    ticket = Some(
                        unsafe {
                            rta.write(|t| {
                                t.a = tid as u32;
                                t.b = i as u32;
                            })
                        }
                        .unwrap(),
                    );
                }

                ticket.unwrap().wait().unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let _ = unsafe { rta.read() };
    }

    #[test]
    fn ok_wraparound_many_times() {
        let (_path, cfg) = prep_init(2);
        let rta = Rta::<Type>::new(cfg).unwrap();

        let mut ticket = None;
        for i in 0..0x1000 {
            ticket = Some(
                unsafe {
                    rta.write(|t| {
                        t.a = i;
                        t.b = i;
                    })
                }
                .unwrap(),
            );
        }

        ticket.unwrap().wait().unwrap();
        assert_eq!(unsafe { rta.read() }, Type { a: 0xFFF, b: 0xFFF });
    }
}
