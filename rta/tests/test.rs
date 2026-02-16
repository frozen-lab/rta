use rta::{Rta, RTA};

#[repr(C)]
#[derive(Default, Clone, RTA)]
struct Meta {
    id: u64,
    name: &'static str,
}

#[test]
fn basic() {
    let dir = std::path::PathBuf::from("/tmp/frozen-core/examples");
    std::fs::create_dir_all(&dir).expect("create example dir");

    let path = dir.join("ff_example.bin");
    if path.exists() {
        std::fs::remove_file(&path).expect("remove existing");
    }

    let rta = Rta::<Meta>::new(path.clone()).expect("init");

    let default = Meta::default();
    let initial = rta.read().expect("read");

    assert_eq!(initial.id, default.id);
    assert_eq!(initial.name, default.name);

    let m = Meta {
        id: 0x20,
        name: "Metadata",
    };
    assert!(rta.write(&m).is_ok());

    drop(rta);

    let rta = Rta::<Meta>::open(path.clone()).expect("re_init");
    let persisted = rta.read().expect("read_back");

    assert_eq!(persisted.id, m.id);
    assert_eq!(persisted.name, m.name);
}
