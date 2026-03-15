use frozen_core::{
    crc32::Crc32C,
    error::FrozenRes,
    fmmap::{FMCfg, FrozenMMap},
};
use std::{slice, sync, time};

pub use rta_derive::RTA;

/// module id for [`Rta`] is `1`
const MOD_ID: u8 = 1;

/// default flush duration for [`FrozenMMap`], we perform sync at interval of `256 ms`
const DEFAULT_FLUSH_DURATION: std::time::Duration = time::Duration::from_millis(0x100);

static CRC32: sync::OnceLock<Crc32C> = sync::OnceLock::new();

#[inline(always)]
fn crc32() -> &'static Crc32C {
    CRC32.get().expect("CRC32 OnceLock not initialized")
}

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
pub struct Rta<T: RTA + Send + Sync> {
    lock: std::sync::Mutex<()>,
    mmap: FrozenMMap<DiskInterface<T>>,
}

impl<T> Rta<T>
where
    T: RTA + Sized + Default + Send + Sync,
{
    /// Create a new instance of [`Rta`]
    pub fn new(path: &std::path::Path) -> FrozenRes<Self> {
        let _ = CRC32.get_or_init(|| Crc32C::default());
        let mmap = FrozenMMap::<DiskInterface<T>>::new(FMCfg {
            mid: MOD_ID,
            initial_count: 2,
            path: path.to_path_buf(),
            flush_duration: DEFAULT_FLUSH_DURATION,
        })?;

        Self::init_if_new(&mmap)?;

        Ok(Self {
            mmap,
            lock: std::sync::Mutex::new(()),
        })
    }

    #[inline(always)]
    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> FrozenRes<R> {
        let result = self.mmap.read(0, |di| {
            let byte_slice = [to_bytes(&di.obja.obj), to_bytes(&di.objb.obj)];
            let slice = crc32().crc_2x(byte_slice);

            let a_valid = di.obja.valid(slice[0]);
            let b_valid = di.objb.valid(slice[1]);

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

        match result {
            Ok(res) => res.ok_or_else(|| {
                frozen_core::error::FrozenErr::new(
                    MOD_ID,
                    0x01,
                    0x03,
                    b"corrupted state",
                    b"no valid object copies".to_vec(),
                )
            }),
            Err(_) => panic!(),
        }
    }

    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(&mut T)) -> FrozenRes<()> {
        let _g = self.lock.lock().expect("Rta write lock poisoned");

        self.mmap.write(0, |di| {
            let max_ver = di.obja.ver.max(di.objb.ver);
            let target = di.select_oldest_mut();

            f(&mut target.obj);

            target.ver = max_ver.wrapping_add(1);
            target.crc = crc32().crc(to_bytes(&target.obj));
        })?;

        Ok(())
    }

    fn init_if_new(mmap: &FrozenMMap<DiskInterface<T>>) -> FrozenRes<()> {
        match mmap.read(0, |di| {
            let byte_slice = [to_bytes(&di.obja.obj), to_bytes(&di.objb.obj)];
            let slice = crc32().crc_2x(byte_slice);

            di.state(slice[0], slice[1])
        })? {
            DIState::Valid => {}
            DIState::Uninitialized => {
                let _ = mmap.write_sync(0, |di| di.bootstrap())?;
            }
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
    #[inline(always)]
    fn state(&self, crc_a: u32, crc_b: u32) -> DIState {
        if self.hash == 0 {
            return DIState::Uninitialized;
        }

        if self.hash != T::HASH {
            return DIState::HashMismatch;
        }

        if self.obja.valid(crc_a) || self.objb.valid(crc_b) {
            return DIState::Valid;
        }

        DIState::Corrupted
    }

    fn bootstrap(&mut self) {
        let default = T::default();
        let crc = crc32().crc(to_bytes(&default));

        self.hash = T::HASH;

        // primary copy (valid)
        self.obja.obj = default;
        self.obja.ver = 1;
        self.obja.crc = crc;

        // secondary copy (invalid)
        self.objb.ver = 0;
        self.objb.crc = 0;
    }

    #[inline(always)]
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
    fn valid(&self, crc: u32) -> bool {
        self.crc == crc
    }
}

#[inline]
const fn to_bytes<T: RTA>(t: &T) -> &[u8] {
    unsafe { slice::from_raw_parts(t as *const T as *const u8, T::SIZE) }
}
