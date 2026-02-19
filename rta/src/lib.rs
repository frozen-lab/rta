use core::{marker::PhantomData, slice};
use frozen_core::{
    error::FrozenRes,
    ffile::FrozenFile,
    fmmap::{FMCfg, FrozenMMap},
};

pub use rta_derive::RTA;

/// module id for [`Rta`] is `1`
const MOD_ID: u8 = 1;

/// default flush duration for [`FrozenMMap`], we perform sync at interval of `256 ms`
const DEFAULT_FLUSH_DURATION: std::time::Duration = std::time::Duration::from_millis(0x100);

/// default config used for [`FrozenMMap`]
const MMAP_CFG: FMCfg = FMCfg {
    module_id: MOD_ID,
    auto_flush: true,
    flush_duration: DEFAULT_FLUSH_DURATION,
};

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
pub unsafe trait RTA: Sized + Default {
    /// deterministic precomputed (at compile time) `u64` hash for type `T`
    const HASH: u64;

    /// object size of type `T`, determined using `core::mem::size_of::<T>()`
    const SIZE: usize;
}

/// Ṛta (ऋत) is a minimal metadata store for durable system state
pub struct Rta<T: RTA> {
    mmap: FrozenMMap,
    lock: std::sync::Mutex<()>,
    _type: PhantomData<T>,
}

impl<T> Rta<T>
where
    T: RTA + Sized + Default,
{
    const SIZE_ON_DISK: usize = core::mem::size_of::<DiskInterface<T>>();

    /// Create a new instance of [`Rta`]
    pub fn new(path: Vec<u8>) -> FrozenRes<Self> {
        let file = FrozenFile::new(path, Self::SIZE_ON_DISK as u64, MOD_ID)?;
        let mmap = FrozenMMap::new(file, Self::SIZE_ON_DISK, MMAP_CFG)?;

        Self::init_if_new(&mmap)?;

        Ok(Self {
            mmap,
            lock: std::sync::Mutex::new(()),
            _type: PhantomData,
        })
    }

    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> FrozenRes<R> {
        let reader = self.mmap.reader::<DiskInterface<T>>(0)?;

        let result = reader.read(|di| {
            let a_valid = di.obja.valid();
            let b_valid = di.objb.valid();

            let chosen = match (a_valid, b_valid) {
                (true, true) => {
                    if di.obja.ver >= di.objb.ver {
                        &di.obja
                    } else {
                        &di.objb
                    }
                }
                (true, false) => &di.obja,
                (false, true) => &di.objb,
                (false, false) => return None,
            };

            Some(f(&chosen.obj))
        });

        result.ok_or_else(|| {
            frozen_core::error::FrozenErr::new(
                MOD_ID,
                0x01,
                0x03,
                b"corrupted state",
                b"no valid object copies".to_vec(),
            )
        })
    }

    pub fn write(&self, f: impl FnOnce(&mut T)) -> FrozenRes<()> {
        let _g = self.lock.lock().expect("Rta write lock poisoned");
        let writer = self.mmap.writer::<DiskInterface<T>>(0)?;

        writer.write(|di| {
            let max_ver = di.obja.ver.max(di.objb.ver);
            let target = di.select_oldest_mut();

            f(&mut target.obj);

            target.ver = max_ver.wrapping_add(1);
            target.crc = crc32(to_bytes(&target.obj));
        })?;

        Ok(())
    }

    fn init_if_new(mmap: &FrozenMMap) -> FrozenRes<()> {
        let reader = mmap.reader::<DiskInterface<T>>(0)?;
        match reader.read(|di| di.state()) {
            DIState::Uninitialized => {
                let writer = mmap.writer::<DiskInterface<T>>(0)?;
                writer.write(|di| di.bootstrap())?;
            }
            DIState::Valid => {}
            DIState::HashMismatch => {
                return Err(frozen_core::error::FrozenErr::new(
                    MOD_ID,
                    0x01,
                    0x01,
                    b"type hash mismatch",
                    b"stored hash != T::HASH".to_vec(),
                ));
            }
            DIState::Corrupted => {
                return Err(frozen_core::error::FrozenErr::new(
                    MOD_ID,
                    0x01,
                    0x02,
                    b"corrupted disk state",
                    b"no valid copies".to_vec(),
                ));
            }
        }

        Ok(())
    }
}

enum DIState {
    Uninitialized,
    Valid,
    HashMismatch,
    Corrupted,
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
    fn state(&self) -> DIState {
        if self.hash == 0 {
            return DIState::Uninitialized;
        }

        if self.hash != T::HASH {
            return DIState::HashMismatch;
        }

        if self.obja.valid() || self.objb.valid() {
            return DIState::Valid;
        }

        DIState::Corrupted
    }

    #[inline]
    fn bootstrap(&mut self) {
        let default = T::default();
        let crc = crc32(to_bytes(&default));

        self.hash = T::HASH;

        // primary copy (valid)
        self.obja.obj = default;
        self.obja.ver = 1;
        self.obja.crc = crc;

        // secondary copy (invalid)
        self.objb.ver = 0;
        self.objb.crc = 0;
    }

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
const fn to_bytes<T: RTA>(t: &T) -> &[u8] {
    unsafe { slice::from_raw_parts(t as *const T as *const u8, T::SIZE) }
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
