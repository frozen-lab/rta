use rta::{Rta, RTA};
use std::io::{Seek, SeekFrom, Write};

const MOD_ID: u8 = 0x00;

#[repr(C)]
#[derive(Default, Clone, Copy, RTA)]
struct TestType {
    a: u64,
    b: u64,
}

#[repr(C)]
#[derive(Default, Clone, Copy, RTA)]
struct TestType2 {
    a: u64,
    b: u64,
}

fn tmp_path() -> std::path::PathBuf {
    tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf()
}

#[test]
fn ok_init_new() {
    let path = tmp_path();

    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    let val = rta.read().unwrap();

    assert_eq!(val.a, 0);
    assert_eq!(val.b, 0);
}

#[test]
fn ok_init_existing() {
    let path = tmp_path();

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        rta.write(|t| {
            t.a = 0x30;
            t.b = 0x40;
        })
        .unwrap();

        drop(rta);
    }

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        let val = rta.read().unwrap();

        assert_eq!(val.a, 0x30);
        assert_eq!(val.b, 0x40);
    }
}

#[test]
fn err_init_hash_mismatch() {
    let path = tmp_path();
    let _ = Rta::<TestType, MOD_ID>::new(&path).unwrap();

    let res = Rta::<TestType2, MOD_ID>::new(&path);
    assert!(res.is_err());
}

#[test]
fn err_init_all_corrupt() {
    let path = tmp_path();

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        rta.write(|t| t.a = 1).unwrap();
        drop(rta);
    }

    // NOTE: we manually corrupt all entries to induce the error
    {
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();

        file.seek(SeekFrom::Start(0)).expect("seek");
        file.write_all(&[0xFF; 0x1000]).expect("write");
        file.flush().expect("flush");
    }

    let res = Rta::<TestType, MOD_ID>::new(&path);
    assert!(res.is_err());
}

#[test]
fn ok_single_write() {
    let path = tmp_path();

    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    rta.write(|t| {
        t.a = 0x0A;
        t.b = 0x14;
    })
    .unwrap();

    let val = rta.read().unwrap();
    assert_eq!(val.a, 0x0A);
    assert_eq!(val.b, 0x14);
}

#[test]
fn ok_multiple_writes_where_last_wins() {
    let path = tmp_path();
    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();

    for i in 0..0x0A {
        rta.write(|t| {
            t.a = i;
            t.b = i * 2;
        })
        .unwrap();
    }

    let val = rta.read().unwrap();
    assert_eq!(val.a, 9);
    assert_eq!(val.b, 0x12);
}

#[test]
fn ok_write_read_interleaved() {
    let path = tmp_path();
    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();

    rta.write(|t| t.a = 1).unwrap();
    assert_eq!(rta.read().unwrap().a, 1);

    rta.write(|t| t.a = 2).unwrap();
    assert_eq!(rta.read().unwrap().a, 2);

    rta.write(|t| t.a = 3).unwrap();
    assert_eq!(rta.read().unwrap().a, 3);
}

#[test]
fn ok_concurrent_writes() {
    let path = tmp_path();
    let rta = std::sync::Arc::new(Rta::<TestType, MOD_ID>::new(&path).unwrap());

    let mut handles = vec![];
    for i in 0..4 {
        let r = rta.clone();
        handles.push(std::thread::spawn(move || {
            r.write(|t| {
                t.a = i;
            })
            .unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let val = rta.read().unwrap();
    assert!(val.a < 4);
}

#[test]
fn ok_concurrent_reads() {
    let path = tmp_path();
    let rta = std::sync::Arc::new(Rta::<TestType, MOD_ID>::new(&path).unwrap());

    rta.write(|t| {
        t.a = 0x80;
        t.b = 0x100;
    })
    .unwrap();

    let mut handles = vec![];
    for _ in 0..4 {
        let r = rta.clone();
        handles.push(std::thread::spawn(move || {
            let v = r.read().unwrap();
            assert_eq!(v.a, 0x80);
            assert_eq!(v.b, 0x100);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn ok_concurrent_reads_during_concurrent_writes() {
    let path = tmp_path();
    let rta = std::sync::Arc::new(Rta::<TestType, MOD_ID>::new(&path).unwrap());

    let r1 = rta.clone();
    let writer = std::thread::spawn(move || {
        for i in 0..0x20 {
            r1.write(|t| {
                t.a = i;
                t.b = i;
            })
            .unwrap();
        }
    });

    let r2 = rta.clone();
    let reader = std::thread::spawn(move || {
        for _ in 0..0x20 {
            let v = r2.read().unwrap();
            assert_eq!(v.a, v.b);
        }
    });

    writer.join().unwrap();
    reader.join().unwrap();
}

#[test]
fn ok_drop_persists_last_dirty() {
    let path = tmp_path();

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        rta.write(|t| {
            t.a = 0xAA;
            t.b = 0xBB;
        })
        .unwrap();
    }

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        let v = rta.read().unwrap();

        assert_eq!(v.a, 0xAA);
        assert_eq!(v.b, 0xBB);
    }
}

#[test]
fn ok_init_existing_with_partial_corruption() {
    use std::io::{Seek, SeekFrom, Write};
    let path = tmp_path();

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();

        rta.write(|t| {
            t.b = 0x22;
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        rta.write(|t| {
            t.a = 0x11;
        })
        .unwrap();

        drop(rta);
    }

    {
        const SLOT_SIZE: usize = 0x10 + TestType::SIZE;
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();

        // corrupt slot 0
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        // corrupt slot 2
        f.seek(SeekFrom::Start((SLOT_SIZE * 2) as u64)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        // corrupt slot 3
        f.seek(SeekFrom::Start((SLOT_SIZE * 3) as u64)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        f.flush().unwrap();
    }

    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    let v = rta.read().unwrap();

    assert_eq!(v.a, 0x11);
    assert_eq!(v.b, 0x22);
}

#[test]
fn ok_fallback_to_previous_version_when_corrupted() {
    use std::io::{Seek, SeekFrom, Write};
    let path = tmp_path();

    {
        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();

        rta.write(|t| t.a = 1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        rta.write(|t| t.a = 2).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        rta.write(|t| t.a = 3).unwrap();

        drop(rta);
    }

    {
        const SLOT_SIZE: usize = 0x10 + TestType::SIZE;
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();

        // corrupt slot 1
        f.seek(SeekFrom::Start(SLOT_SIZE as u64)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        // corrupt slot 2
        f.seek(SeekFrom::Start((SLOT_SIZE * 2) as u64)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        // corrupt slot 3
        f.seek(SeekFrom::Start((SLOT_SIZE * 3) as u64)).unwrap();
        f.write_all(&[0xFF; SLOT_SIZE]).unwrap();

        f.flush().unwrap();
    }

    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    let v = rta.read().unwrap();
    assert!(v.a == 1);
}

#[test]
fn ok_drop_under_load() {
    let path = tmp_path();
    let rta = std::sync::Arc::new(Rta::<TestType, MOD_ID>::new(&path).unwrap());

    let mut handles = vec![];
    for i in 0..4 {
        let r = rta.clone();
        handles.push(std::thread::spawn(move || {
            r.write(|t| {
                t.a = i;
                t.b = i;
            })
            .unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    drop(rta);

    let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    let v = rta.read().unwrap();
    assert_eq!(v.a, v.b);
}

#[test]
fn ok_stress_mixed_write_read() {
    let path = tmp_path();
    let rta = std::sync::Arc::new(Rta::<TestType, MOD_ID>::new(&path).unwrap());

    let mut handles = vec![];
    for _ in 0..4 {
        let r = rta.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..0x80 {
                r.write(|t| {
                    t.a = i;
                    t.b = i;
                })
                .unwrap();
            }
        }));
    }

    for _ in 0..4 {
        let r = rta.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..0x80 {
                let v = r.read().unwrap();
                assert_eq!(v.a, v.b);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn err_unaligned_type_t() {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct BadAlign {
        a: u32,
    }

    // NOTE: we mock this as the RTA macro prevents BadAlign w/ compiler error
    unsafe impl RTA for BadAlign {
        const HASH: u64 = 1;
        const SIZE: usize = std::mem::size_of::<Self>();
    }

    let path = tmp_path();
    let res = Rta::<BadAlign, MOD_ID>::new(&path);
    assert!(res.is_err());
}

#[test]
fn err_type_with_drop() {
    #[repr(C)]
    #[derive(Default, Clone, RTA)]
    struct HasDrop {
        a: u64,
    }

    impl Drop for HasDrop {
        fn drop(&mut self) {}
    }

    let path = tmp_path();
    let res = Rta::<HasDrop, MOD_ID>::new(&path);
    assert!(res.is_err());
}

#[test]
fn ok_large_type() {
    #[repr(C)]
    #[derive(Clone, Copy, RTA)]
    struct LargeT {
        data: [u64; 0x80],
    }

    impl Default for LargeT {
        fn default() -> Self {
            Self { data: [0u64; 0x80] }
        }
    }

    let path = tmp_path();
    let rta = Rta::<LargeT, MOD_ID>::new(&path).unwrap();

    rta.write(|t| {
        t.data[0] = 0x30;
        t.data[127] = 0x4A;
    })
    .unwrap();

    let v = rta.read().unwrap();
    assert_eq!(v.data[0], 0x30);
    assert_eq!(v.data[127], 0x4A);
}

#[test]
fn ok_open_drop_under_stress() {
    let path = tmp_path();

    for i in 0..0x32 {
        {
            let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
            rta.write(|t| {
                t.a = i;
                t.b = i;
            })
            .unwrap();
        }

        let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
        let v = rta.read().unwrap();
        assert_eq!(v.a, i);
    }
}
