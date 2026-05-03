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
