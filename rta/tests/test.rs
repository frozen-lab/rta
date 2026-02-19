use rta::{Rta, RTA};
use std::sync::atomic::{AtomicU16, Ordering};
use tempfile::tempdir;

#[repr(C)]
#[derive(Default, RTA)]
struct Meta {
    id: AtomicU16,
    name: &'static str,
}

fn new_rta() -> (tempfile::TempDir, std::path::PathBuf, Rta<Meta>) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("meta.bin");

    let rta = Rta::<Meta>::new(path.as_os_str().as_encoded_bytes().to_vec()).expect("init");

    (dir, path, rta)
}

#[test]
fn bootstrap_is_default() {
    let (_dir, _path, rta) = new_rta();

    rta.read(|m| {
        assert_eq!(m.id.load(Ordering::Relaxed), 0);
        assert_eq!(m.name, "");
    })
    .expect("read");
}

#[test]
fn write_and_reopen_persists() {
    let (_dir, path, rta) = new_rta();

    rta.write(|m| {
        m.id.store(42, Ordering::Relaxed);
        m.name = "hello";
    })
    .expect("write");

    drop(rta);

    let rta = Rta::<Meta>::new(path.as_os_str().as_encoded_bytes().to_vec()).expect("reopen");

    rta.read(|m| {
        assert_eq!(m.id.load(Ordering::Relaxed), 42);
        assert_eq!(m.name, "hello");
    })
    .expect("read");
}

#[test]
fn multiple_writes_keep_latest() {
    let (_dir, _path, rta) = new_rta();

    for i in 0..10 {
        rta.write(|m| {
            m.id.store(i, Ordering::Relaxed);
        })
        .expect("write");
    }

    rta.read(|m| {
        assert_eq!(m.id.load(Ordering::Relaxed), 9);
    })
    .expect("read");
}

#[test]
fn concurrent_reads_are_safe() {
    use std::sync::Arc;
    use std::thread;

    let (_dir, _path, rta) = new_rta();
    let rta = Arc::new(rta);

    rta.write(|m| {
        m.id.store(7, Ordering::Relaxed);
    })
    .expect("write");

    let mut handles = vec![];

    for _ in 0..8 {
        let r = rta.clone();
        handles.push(thread::spawn(move || {
            r.read(|m| {
                assert_eq!(m.id.load(Ordering::Relaxed), 7);
            })
            .expect("read");
        }));
    }

    for h in handles {
        h.join().expect("join");
    }
}

#[test]
fn version_monotonicity() {
    let (_dir, _path, rta) = new_rta();

    for i in 0..100 {
        rta.write(|m| {
            m.id.store(i, Ordering::Relaxed);
        })
        .expect("write");
    }

    rta.read(|m| {
        assert_eq!(m.id.load(Ordering::Relaxed), 99);
    })
    .expect("read");
}

#[repr(C)]
#[derive(Default, RTA)]
struct DifferentMeta {
    id: AtomicU16,
}

#[test]
fn hash_mismatch_detection() {
    let (_dir, path, rta) = new_rta();

    rta.write(|m| {
        m.id.store(99, Ordering::Relaxed);
    })
    .expect("write");

    drop(rta);

    let result = Rta::<DifferentMeta>::new(path.as_os_str().as_encoded_bytes().to_vec());

    assert!(result.is_err());
}
