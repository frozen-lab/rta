use rta::{Rta, RTA};

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
    tempfile::tempdir().expect("tmpdir").keep().join("rta_test")
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

    {
        let _ = Rta::<TestType, MOD_ID>::new(&path).unwrap();
    }

    let res = Rta::<TestType2, MOD_ID>::new(&path);
    assert!(res.is_err());
}

#[test]
fn err_init_all_corrupt() {
    use std::io::{Seek, SeekFrom, Write};
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
