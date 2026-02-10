#![allow(unused)]

use rta::{Rta, RTA};

#[derive(Clone, RTA)]
struct A {
    a: u32,
}

fn main() {
    let a = A { a: 20 };
    let rta = Rta::new(&a);
    print!("A = {} & {}", rta.size(), rta.hash());
}
