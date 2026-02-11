use rta::{Rta, RTA};
use tempfile::tempdir;

#[repr(C)]
#[derive(Debug, Clone, Default, RTA, PartialEq)]
struct Meta {
    counter: u64,
    value: u64,
}

fn main() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("tmp_file");

    let rta = Rta::<Meta>::new(path.clone()).expect("init");

    let initial = rta.read().expect("read");
    assert_eq!(initial, Meta::default());
    println!("Initial: {:?}", initial);

    for i in 0..5 {
        let m = Meta {
            counter: i,
            value: i * 100,
        };

        rta.write(&m).expect("write");
        let read_back = rta.read().expect("read back");

        assert_eq!(read_back, m);
        println!("Write {} -> {:?}", i, read_back);
    }

    drop(rta);

    let rta = Rta::<Meta>::open(path.clone()).expect("re-init");
    let persisted = rta.read().expect("read");

    assert_eq!(persisted, Meta { counter: 4, value: 400 });
    println!("Persisted after reopen: {:?}", persisted);
}
