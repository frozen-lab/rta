#![allow(unused)]

pub use rta_derive::RTA;

pub unsafe trait RTA: Clone + Sized {
    const HASH: u64;
    const SIZE: usize;
}

pub struct Rta<T: RTA> {
    tp: T,
}

impl<T> Rta<T>
where
    T: RTA,
{
    pub fn new(tp: &T) -> Self {
        Self { tp: tp.clone() }
    }

    pub fn size(&self) -> usize {
        T::SIZE
    }

    pub fn hash(&self) -> u64 {
        T::HASH
    }
}
