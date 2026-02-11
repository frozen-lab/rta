#![allow(unused)]

use core::{ptr, slice};
use frozen_core::{
    fe::FRes,
    ff::{FFCfg, FF},
    fm::{FMCfg, FM},
};

const MID: u8 = 0x01;

pub use rta_derive::RTA;

pub unsafe trait RTA: Clone + Sized {
    const HASH: u64;
    const SIZE: usize;
}

pub struct Rta<T: RTA> {
    tp: T,
    file: FF,
    mmap: FM,
    lock: std::sync::Mutex<()>,
}

impl<T> Rta<T>
where
    T: RTA + Clone + Sized,
{
    const FILE_SIZE: usize = core::mem::size_of::<DiskInterface<T>>();

    pub fn new(tp: &T, path: std::path::PathBuf) -> FRes<Self> {
        let file_cfg = FFCfg {
            path,
            module_id: MID,
            auto_flush: false,
            flush_duration: std::time::Duration::from_secs(1),
        };
        let file = FF::new(file_cfg, Self::FILE_SIZE as u64)?;

        let mmap_cfg = FMCfg {
            module_id: MID,
            auto_flush: true,
            flush_duration: std::time::Duration::from_millis(200),
        };
        let mmap = FM::new(file.fd(), Self::FILE_SIZE, mmap_cfg)?;

        Ok(Self {
            file,
            mmap,
            tp: tp.clone(),
            lock: std::sync::Mutex::new(()),
        })
    }

    pub fn size(&self) -> usize {
        T::SIZE
    }

    pub fn hash(&self) -> u64 {
        T::HASH
    }

    #[inline(always)]
    pub fn read(&self) -> FRes<T> {
        let r = self.mmap.reader::<DiskInterface<T>>(0)?;
        let val = r.read(|di| {
            let a_valid = Self::valid(&di.obja);
            let b_valid = Self::valid(&di.objb);

            match (a_valid, b_valid) {
                (true, true) => {
                    if di.obja.ver >= di.objb.ver {
                        di.obja.obj.clone()
                    } else {
                        di.objb.obj.clone()
                    }
                }
                (true, false) => di.obja.obj.clone(),
                (false, true) => di.objb.obj.clone(),
                (false, false) => panic!("both metadata copies corrupt"),
            }
        });

        Ok(val)
    }

    #[inline(always)]
    pub fn write(&self, new_val: &T) -> FRes<()> {
        let _g = self.lock.lock().unwrap();
        let w = self.mmap.writer::<DiskInterface<T>>(0)?;

        w.write(|di| {
            let target = Self::select_oldest_mut(di);

            target.obj = new_val.clone();
            target.ver = target.ver.wrapping_add(1);
            target.crc = crc64(Self::to_bytes(&target.obj));
        })?;

        Ok(())
    }

    #[inline]
    const fn to_bytes(tp: &T) -> &[u8] {
        unsafe { slice::from_raw_parts(tp as *const T as *const u8, T::SIZE) }
    }

    #[inline]
    fn select_latest(di: &DiskInterface<T>) -> &DiskObject<T> {
        if di.obja.ver >= di.objb.ver {
            &di.obja
        } else {
            &di.objb
        }
    }

    #[inline]
    fn select_oldest_mut(di: &mut DiskInterface<T>) -> &mut DiskObject<T> {
        if di.obja.ver <= di.objb.ver {
            &mut di.obja
        } else {
            &mut di.objb
        }
    }

    #[inline]
    fn valid(obj: &DiskObject<T>) -> bool {
        crc64(Self::to_bytes(&obj.obj)) == obj.crc
    }
}

#[inline]
fn crc64(_bytes: &[u8]) -> u64 {
    0u64
}

#[repr(C)]
struct DiskInterface<T: RTA> {
    hash: u64,
    obja: DiskObject<T>,
    objb: DiskObject<T>,
}

#[repr(C)]
struct DiskObject<T: RTA> {
    obj: T,
    ver: u64,
    crc: u64,
}
