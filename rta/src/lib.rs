use core::{marker::PhantomData, slice};
use frozen_core::{error, ffile, fmmap};
use std::os::unix::ffi::OsStrExt;

/// module id for [`Rta`] is `1`
const MOD_ID: u8 = 1;

/// default flush duration for [`FrozenMMap`], we perform sync at interval of `256 ms`
const DEFAULT_FLUSH_DURATION: std::time::Duration = std::time::Duration::from_millis(0x100);

pub use rta_derive::RTA;

/// Derives the `rta::RTA` trait for a struct `T`
///
/// ## Important
///
/// - type `T` must use `repr(C)`
/// - should not implement Drop (to avoid undefined behaviour)
///
/// ## Why?
///
/// `#[derive(RTA)]` implementation, computes a compile time `HASH`, which is used as unique
/// and deterministic id for a given type `T`
///
/// This is to track any changes in the implementation of type `T`
pub unsafe trait RTA: Clone + Sized + Default {
    const HASH: u64;
    const SIZE: usize;
}

pub struct Rta<T: RTA> {
    mmap: fmmap::FrozenMMap,
    lock: std::sync::Mutex<()>,
    _type: PhantomData<T>,
}

impl<T> Rta<T>
where
    T: RTA + Clone + Sized + Default,
{
    const FILE_SIZE: usize = core::mem::size_of::<DiskInterface<T>>();

    pub fn new(path: std::path::PathBuf) -> error::FrozenRes<Self> {
        if path.exists() {
            panic!("invalid path, path to already existing file");
        }

        if path.is_dir() {
            panic!("path must be of a file, not dir");
        }

        let mmap_cfg = fmmap::FMCfg {
            auto_flush: true,
            module_id: MOD_ID,
            flush_duration: DEFAULT_FLUSH_DURATION,
        };

        let file = ffile::FrozenFile::new(path.as_os_str().as_bytes().to_vec(), Self::FILE_SIZE as u64, MOD_ID)?;
        let mmap = fmmap::FrozenMMap::new(file, Self::FILE_SIZE, mmap_cfg)?;

        {
            let writer = mmap.writer::<DiskInterface<T>>(0)?;
            writer.write(|di| {
                di.hash = T::HASH;

                di.obja.obj = T::default();
                di.obja.ver = 1;
                di.obja.crc = crc32(to_bytes(&di.obja.obj));

                di.objb = di.obja.clone();
            })?;
        }

        Ok(Self {
            mmap,
            _type: PhantomData,
            lock: std::sync::Mutex::new(()),
        })
    }

    pub fn open(path: std::path::PathBuf) -> error::FrozenRes<Self> {
        if !path.exists() {
            panic!("Rta does not exists");
        }

        if !path.is_file() {
            panic!("Path is not a file");
        }

        let mmap_cfg = fmmap::FMCfg {
            module_id: MOD_ID,
            auto_flush: true,
            flush_duration: DEFAULT_FLUSH_DURATION,
        };

        let file = ffile::FrozenFile::new(path.as_os_str().as_bytes().to_vec(), Self::FILE_SIZE as u64, MOD_ID)?;
        let mmap = fmmap::FrozenMMap::new(file, Self::FILE_SIZE, mmap_cfg)?;

        {
            let r = mmap.reader::<DiskInterface<T>>(0)?;
            r.read(|di| {
                if di.hash != T::HASH {
                    panic!("metadata hash mismatch");
                }

                let a = di.obja.valid();
                let b = di.objb.valid();

                if !a && !b {
                    panic!("both metadata copies corrupt");
                }
            });
        }

        Ok(Self {
            mmap,
            _type: PhantomData,
            lock: std::sync::Mutex::new(()),
        })
    }

    pub fn size() -> usize {
        core::mem::size_of::<T>()
    }

    pub fn hash() -> u64 {
        T::HASH
    }

    #[inline(always)]
    pub fn read(&self) -> error::FrozenRes<T> {
        let r = self.mmap.reader::<DiskInterface<T>>(0)?;
        let val = r.read(|di| {
            let a_valid = di.obja.valid();
            let b_valid = di.objb.valid();

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
    pub fn write(&self, new_val: &T) -> error::FrozenRes<()> {
        let _g = self.lock.lock().unwrap();
        let w = self.mmap.writer::<DiskInterface<T>>(0)?;

        w.write(|di| {
            let max_ver = di.obja.ver.max(di.objb.ver);
            let target = di.select_oldest_mut();

            target.obj = new_val.clone();
            target.ver = max_ver.wrapping_add(1);
            target.crc = crc32(to_bytes(&target.obj));
        })?;

        Ok(())
    }
}

#[inline]
fn crc32(bytes: &[u8]) -> u32 {
    const POLY: u32 = 0x82F63B78;
    let mut crc: u32 = 0;

    // hardware streaming CRC32C (x86 only) (Castagnoli, reflected)
    if std::is_x86_feature_detected!("sse4.2") {
        use core::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};

        unsafe {
            let mut ptr = bytes.as_ptr();
            let mut len = bytes.len();

            while len >= 8 {
                let val = core::ptr::read_unaligned(ptr as *const u64);
                crc = _mm_crc32_u64(crc as u64, val) as u32;
                ptr = ptr.add(8);
                len -= 8;
            }

            while len > 0 {
                crc = _mm_crc32_u8(crc, *ptr);
                ptr = ptr.add(1);
                len -= 1;
            }

            return crc;
        }
    }

    // CRC32C fallback
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }

    crc
}

#[repr(C)]
struct DiskInterface<T: RTA> {
    hash: u64,
    obja: DiskObject<T>,
    objb: DiskObject<T>,
}

impl<T> DiskInterface<T>
where
    T: RTA,
{
    #[inline]
    fn select_oldest_mut(&mut self) -> &mut DiskObject<T> {
        if self.obja.ver <= self.objb.ver {
            &mut self.obja
        } else {
            &mut self.objb
        }
    }
}

#[repr(C)]
#[derive(Clone)]
struct DiskObject<T: RTA> {
    obj: T,
    ver: u32,
    crc: u32,
}

impl<T> DiskObject<T>
where
    T: RTA,
{
    #[inline]
    fn valid(&self) -> bool {
        crc32(to_bytes(&self.obj)) == self.crc
    }
}

#[inline]
const fn to_bytes<T: Sized>(t: &T) -> &[u8] {
    unsafe { slice::from_raw_parts(t as *const T as *const u8, core::mem::size_of::<T>()) }
}
