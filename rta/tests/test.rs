use rta::{RTA, Rta, RtaCfg};

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
    let cfg = RtaCfg {
        module_id: MOD_ID,
        path: path.clone(),
        copies_on_disk: copies,
    };

    (path, cfg)
}

mod new {
    use super::*;

    #[test]
    fn ok_new() {
        let (_path, cfg) = prep_init(0x0A);
        let rta = Rta::<Type>::new(cfg).expect("failed to create rta");

        assert_eq!(unsafe { rta.read() }, Type::default());
    }

    #[test]
    fn ok_existing() {
        let (_path, cfg) = prep_init(0x0A);

        {
            let rta = Rta::<Type>::new(cfg.clone()).expect("failed to create rta");

            unsafe {
                let ticket = rta
                    .write(|t| {
                        t.a = 10;
                        t.b = 20;
                    })
                    .expect("failed to write");

                ticket.wait().expect("failed waiting for durability");
            }
        }

        let rta = Rta::<Type>::new(cfg).expect("failed to reopen");
        assert_eq!(unsafe { rta.read() }, Type { a: 10, b: 20 });
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
        let cfg = RtaCfg {
            module_id: MOD_ID,
            copies_on_disk: 0,
            path: path,
        };
        let _ = Rta::<Type>::new(cfg);
    }
}
