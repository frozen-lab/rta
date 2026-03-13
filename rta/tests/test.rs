use rta::{Rta, RTA};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU16, Ordering},
};
use tempfile::{tempdir, TempDir};

#[repr(C)]
#[derive(Default, RTA)]
struct Meta {
    id: AtomicU16,
    name: &'static str,
}

#[repr(C, align(8))]
#[derive(Default, RTA)]
struct DifferentMeta {
    id: AtomicU16,
}

fn new_tmp() -> (TempDir, PathBuf, Rta<Meta>) {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("meta.bin");
    let rta = Rta::<Meta>::new(&path).expect("new RTA");

    (dir, path, rta)
}

mod new {
    use super::*;

    #[test]
    fn new_inits_with_default() {
        let (_dir, _path, rta) = new_tmp();

        rta.read(|m| {
            assert_eq!(m.id.load(Ordering::Relaxed), 0);
            assert_eq!(m.name, "");
        })
        .expect("read");
    }

    #[test]
    fn new_fails_on_hash_mismatch() {
        let (_dir, path, rta) = new_tmp();

        rta.write(|m| {
            m.id.store(99, Ordering::Relaxed);
        })
        .expect("write");

        drop(rta);

        let result = Rta::<DifferentMeta>::new(&path);

        assert!(result.is_err());
    }
}

mod write_read {
    use super::*;

    #[test]
    fn write_read_cycle() {
        let (_dir, _path, rta) = new_tmp();

        rta.write(|m| {
            m.id.store(42, Ordering::Relaxed);
            m.name = "hello";
        })
        .expect("write");

        rta.read(|m| {
            assert_eq!(m.id.load(Ordering::Relaxed), 42);
            assert_eq!(m.name, "hello");
        })
        .expect("read");
    }

    #[test]
    fn write_persists_across_sessions() {
        let (_dir, path, rta) = new_tmp();

        rta.write(|m| {
            m.id.store(42, Ordering::Relaxed);
            m.name = "hello";
        })
        .expect("write");

        drop(rta);

        let rta = Rta::<Meta>::new(&path).expect("reopen");

        rta.read(|m| {
            assert_eq!(m.id.load(Ordering::Relaxed), 42);
            assert_eq!(m.name, "hello");
        })
        .expect("read");
    }

    #[test]
    fn read_after_multiple_writes_gives_latest() {
        let (_dir, _path, rta) = new_tmp();

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
    fn version_updates_are_monotonic() {
        let (_dir, _path, rta) = new_tmp();

        for i in 0..0x80 {
            rta.write(|m| {
                m.id.store(i, Ordering::Relaxed);
            })
            .expect("write");
        }

        rta.read(|m| {
            assert_eq!(m.id.load(Ordering::Relaxed), 0x80 - 1);
        })
        .expect("read");
    }
}

mod concurrency {
    use super::*;

    #[test]
    fn concurrent_reads_are_safe() {
        use std::sync::Arc;
        use std::thread;

        let (_dir, _path, rta) = new_tmp();
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
}
