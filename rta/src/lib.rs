use frozen_core::{
    crc32::Crc32C,
    error::{ErrCode, FrozenErr},
    fmmap::{FMCfg, FrozenMMap},
};
use std::{slice, sync, time};

/// Procedural macro implementation for `#[derive(RTA)]`
pub use rta_derive::RTA;

/// Domain Id for [`Rta`] is **20**
const ERRDOMAIN: u8 = 0x14;

/// module id used for [`FrozenErr`]
static MODULE_ID: std::sync::OnceLock<u8> = std::sync::OnceLock::new();

#[inline(always)]
fn mod_id() -> &'static u8 {
    MODULE_ID.get_or_init(|| 0)
}

/// crc backend used for calculating checksum
static CRC32: sync::OnceLock<Crc32C> = sync::OnceLock::new();

#[inline(always)]
fn crc32() -> &'static Crc32C {
    CRC32.get().expect("CRC32 OnceLock not initialized")
}

/// Error codes for [`FrozenPipe`]
mod err {
    use super::ErrCode;

    /// (1280) all copies are corrupted
    pub const CRP: ErrCode = ErrCode::new(0x500, "All copies of `T` are corrupted");

    /// (1281) invalid hash for T
    pub const HSH: ErrCode = ErrCode::new(0x500, "`T` has HASH mismatch as it may be updated after being stored");
}

#[inline]
fn new_err<R>(code: ErrCode) -> RtaRes<R> {
    let err = FrozenErr::new(*mod_id(), ERRDOMAIN, code, "");
    Err(err.into())
}

#[inline]
fn new_err_raw(code: ErrCode) -> RtaErr {
    FrozenErr::new(*mod_id(), ERRDOMAIN, code, "").into()
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

/// Custom result type w/ [`RtaErr`] as error type
pub type RtaRes<T> = Result<T, RtaErr>;

/// Utility for error propagation used for [`Rta`]
#[derive(Debug, Clone)]
pub struct RtaErr {
    /// Encoded 32-bit unique identifier
    pub id: u32,

    /// Formated string w/ added context describing the error
    pub context: String,
}

impl From<FrozenErr> for RtaErr {
    fn from(value: FrozenErr) -> Self {
        Self {
            id: value.id,
            context: value.context,
        }
    }
}

/// Ṛta (ऋत) is a minimal metadata store for durable system state
pub struct Rta<T: RTA + Send + Sync> {
    lock: std::sync::Mutex<()>,
    mmap: FrozenMMap<DiskInterface<T>>,
}

impl<T> Rta<T>
where
    T: Default + RTA + Send + Sized + Sync,
{
    /// Create a new instance of [`Rta`]
    pub fn new<P: AsRef<std::path::Path>, const MID: u8>(path: P, flush_duration: time::Duration) -> RtaRes<Self> {
        // NOTE: The value is used for error logging and is initialized only once, as `OnceLock` guarantees that the
        // first caller sets the value and all subsequent calls reuse it
        let _ = MODULE_ID.get_or_init(|| MID);

        // INFO: Crc32C selects the optimal backend (hardware or software) at runtime w/ respect to hardware
        //
        // NOTE: Since, the value is used across objects, we initialize it once and pin it in a global `OnceLock`
        // to avoid repeated setup and reference passing
        let _ = CRC32.get_or_init(|| Crc32C::default());

        let mmap = FrozenMMap::<DiskInterface<T>>::new(FMCfg {
            flush_duration,
            mid: MID,
            initial_count: 1,
            path: path.as_ref().to_path_buf(),
        })?;

        Self::init_if_new(&mmap)?;

        Ok(Self {
            mmap,
            lock: std::sync::Mutex::new(()),
        })
    }

    #[inline(always)]
    pub fn read<R>(&self, f: impl FnOnce(&T) -> R) -> RtaRes<R> {
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
            Ok(res) => res.ok_or_else(|| new_err_raw(err::CRP)),
            Err(e) => Err(e.into()),
        }
    }

    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(&mut T)) -> RtaRes<()> {
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

    fn init_if_new(mmap: &FrozenMMap<DiskInterface<T>>) -> RtaRes<()> {
        match mmap.read(0, |di| {
            let byte_slice = [to_bytes(&di.obja.obj), to_bytes(&di.objb.obj)];
            let slice = crc32().crc_2x(byte_slice);

            di.state(slice[0], slice[1])
        })? {
            DIState::Valid => {}
            DIState::Corrupted => return new_err(err::CRP),
            DIState::HashMismatch => return new_err(err::HSH),
            DIState::Uninitialized => {
                let _ = mmap.write_sync(0, |di| di.bootstrap())?;
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
