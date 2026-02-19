use rta::{Rta, RTA};
use std::sync::atomic::{AtomicU16, Ordering};

#[repr(C)]
#[derive(Default, RTA)]
struct Meta {
    id: AtomicU16,
    name: &'static str,
}

#[test]
fn basic() {
    let dir = std::path::PathBuf::from("/tmp/rta/examples");
    std::fs::create_dir_all(&dir).expect("create example dir");

    let path = dir.join("metadata.bin");
    if path.exists() {
        std::fs::remove_file(&path).expect("remove existing");
    }

    let rta = Rta::<Meta>::new(path.as_os_str().as_encoded_bytes().to_vec()).expect("init");

    let default = Meta::default();
    assert_eq!(std::mem::size_of_val(&default), Meta::SIZE);

    rta.read(|m| {
        assert_eq!(std::mem::size_of_val(m), Meta::SIZE);
        assert_eq!(m.id.load(Ordering::Relaxed), default.id.load(Ordering::Relaxed));
        assert_eq!(m.name, default.name);
    })
    .expect("read initial");

    let meta = Meta {
        id: AtomicU16::new(0x10),
        name: "Metadata",
    };
    rta.write(|m| *m = meta).expect("write update");

    drop(rta);

    let rta = Rta::<Meta>::new(path.as_os_str().as_encoded_bytes().to_vec()).expect("init");
    rta.read(|m| {
        assert_eq!(std::mem::size_of_val(m), Meta::SIZE);
        assert_eq!(m.id.load(Ordering::Relaxed), 0x10);
        assert_eq!(m.name, "Metadata");
    })
    .expect("read initial");

    let _ = std::fs::remove_file(&path);
}
