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

/// Total number of version of `T` stored on disk
const VERSIONS_ON_DISK: usize = 4;

/// module id used for [`FrozenErr`]
static MODULE_ID: sync::OnceLock<u8> = sync::OnceLock::new();

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

/// Error codes for [`Rta`]
mod err {
    use super::ErrCode;

    /// (1280) all copies are corrupted
    pub const CRP: ErrCode = ErrCode::new(0x500, "All copies of `T` are corrupted");

    /// (1281) invalid hash for T
    pub const HSH: ErrCode = ErrCode::new(0x501, "`T` has HASH mismatch as it may be updated after being stored");

    /// (1282) type `T` implements drop
    pub const DRP: ErrCode = ErrCode::new(0x502, "T must not implement Drop");

    /// (1283) type `T` is not 8 byte aligned
    pub const ALN: ErrCode = ErrCode::new(0x503, "T must be 8-byte aligned");

    /// (1284) type `T` is zero sized
    pub const ZRO: ErrCode = ErrCode::new(0x504, "T must not be zero-sized");

    /// (1285) `size_of::<T>()` is not multiple of 8
    pub const SZE: ErrCode = ErrCode::new(0x505, "T size must be multiple of 8");
}

#[inline]
fn new_err<R>(code: ErrCode) -> RtaRes<R> {
    let err = FrozenErr::new(*mod_id(), ERRDOMAIN, code, "");
    Err(err.into())
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
pub struct Rta<T: RTA + Send + Sync + Clone> {
    mmap: FrozenMMap<DiskObject<T>>,
    cache: sync::Arc<(sync::Mutex<MemCache<T>>, sync::Condvar)>,
}

impl<T> Rta<T>
where
    T: RTA + Default + Send + Sync + Clone + 'static,
{
    pub fn new<P: AsRef<std::path::Path>, const MID: u8>(path: P, flush_duration: time::Duration) -> RtaRes<Self> {
        // NOTE: The value is used for error logging and is initialized only once, as `OnceLock` guarantees that the
        // first caller sets the value and all subsequent calls reuse it
        let _ = MODULE_ID.get_or_init(|| MID);

        // INFO: Crc32C selects the optimal backend (hardware or software) at runtime w/ respect to hardware
        //
        // NOTE: Since, the value is used across objects, we initialize it once and pin it in a global `OnceLock`
        // to avoid repeated setup and reference passing
        let _ = CRC32.get_or_init(|| Crc32C::default());

        // NOTE: we must validate `T` before mmap init, to avoid any UB errors
        validate_t::<T>()?;

        let mmap = FrozenMMap::<DiskObject<T>>::new(FMCfg {
            flush_duration,
            mid: MID,
            initial_count: VERSIONS_ON_DISK,
            path: path.as_ref().to_path_buf(),
        })?;

        let (obj, ver) = Self::init_or_create(&mmap)?;
        let cache = sync::Arc::new((
            sync::Mutex::new(MemCache { obj, ver, dir: false }),
            sync::Condvar::new(),
        ));

        Ok(Self { mmap, cache })
    }

    fn init_or_create(mmap: &FrozenMMap<DiskObject<T>>) -> RtaRes<(T, u32)> {
        let mut best: Option<DiskObject<T>> = None;
        for i in 0..VERSIONS_ON_DISK {
            let res = mmap.read(i, |di| {
                if di.hsh == 0 {
                    return None;
                }

                if di.hsh != T::HASH {
                    return Some(err::HSH);
                }

                let crc = crc32().crc(to_bytes(&di.obj));
                if di.iseq_crc(crc) {
                    if let Some(curr_best) = &best {
                        if di.ver >= curr_best.ver {
                            best = Some(di.clone());
                        }
                    }
                }

                None
            })?;

            if let Some(err_code) = res {
                return new_err(err_code);
            }
        }

        if let Some(b) = best {
            return Ok((b.obj, b.ver));
        }

        // NOTE: if no valid versions are found (in case of new creation), we init the first slot (0th idx)

        let def = T::default();
        let crc = crc32().crc(to_bytes(&def));

        mmap.write_sync(0, |di| {
            di.ver = 1;
            di.crc = crc;
            di.hsh = T::HASH;
            di.obj = def.clone();
        })?;

        Ok((def, 1))
    }
}

struct MemCache<T: RTA> {
    obj: T,
    ver: u32,
    dir: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DiskObject<T: RTA> {
    ver: u32,
    crc: u32,
    hsh: u64,
    obj: T,
}

impl<T: RTA> DiskObject<T> {
    #[inline]
    fn iseq_crc(&self, crc: u32) -> bool {
        self.crc == crc
    }

    #[inline]
    fn iseq_hsh(&self, hsh: u64) -> bool {
        self.hsh == hsh
    }
}

#[inline]
fn to_bytes<T: RTA>(t: &T) -> &[u8] {
    unsafe { slice::from_raw_parts(t as *const T as *const u8, T::SIZE) }
}

#[inline(always)]
fn validate_t<T: RTA>() -> RtaRes<()> {
    if std::mem::needs_drop::<T>() {
        return new_err(err::DRP);
    }

    let align = std::mem::align_of::<T>();
    if align != 8 {
        return new_err(err::ALN);
    }

    let size = std::mem::size_of::<T>();
    if size == 0 {
        return new_err(err::ZRO);
    }

    if size % 8 != 0 {
        return new_err(err::SZE);
    }

    Ok(())
}
