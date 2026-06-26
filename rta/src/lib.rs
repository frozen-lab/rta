// #![deny(missing_docs)]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(unused)]

use frozen_core::{error, fmmap, reservoir};
use std::{slice, time};

/// Default flush duration used for [`FrozenMMap`]
///
/// ## NOTE
///
/// The value is disregarded and meraly used as a placeholder by the system. As for [`Rta`] we use,
/// the [`FrozenMMapCfg::immediate_flush`] to notify the background thread to flush dirty pages
/// right after a successful [`Rta::write`] is completed.
const MMAP_FLUSH_DURATION: time::Duration = time::Duration::from_secs(1);

/// Derive the `rta::RTA` trait for the struct `T`
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

pub struct RtaCfg {
    pub module_id: u8,
    pub copies_on_disk: usize,
    pub path: std::path::PathBuf,
}

pub struct Rta<T: RTA + Send + Sync + Clone + 'static> {
    mmap: fmmap::FrozenMMap<DiskObject<T>>,
    reservoir: reservoir::Reservoir<usize>,
}

unsafe impl<T> Send for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}
unsafe impl<T> Sync for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}

impl<T> Rta<T>
where
    T: RTA + Default + Send + Sync + Clone + 'static,
{
    #[inline]
    pub fn new(cfg: RtaCfg) -> error::FrozenResult<Self> {
        let mmap = fmmap::FrozenMMap::new(
            cfg.path,
            fmmap::FrozenMMapCfg {
                module_id: cfg.module_id,
                immediate_durability: true,
                initial_count: cfg.copies_on_disk,
                flush_duration: MMAP_FLUSH_DURATION,
            },
        )?;
        let reservoir = reservoir::Reservoir::new((0..cfg.copies_on_disk).into_iter().collect());

        Ok(Self { mmap, reservoir })
    }

    #[inline]
    pub fn clear(&mut self) -> error::FrozenResult<()> {
        todo!()
    }

    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(*mut T)) -> error::FrozenResult<()> {
        let index = self.reservoir.acquire();
        // self.mmap.write(index, f);
        Ok(())
    }
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
fn validate_t<T: RTA>() -> error::FrozenResult<()> {
    if std::mem::needs_drop::<T>() {
        return err::new_err_default(err::DRP);
    }

    let align = std::mem::align_of::<T>();
    if align != 8 {
        return err::new_err_default(err::ALN);
    }

    let size = std::mem::size_of::<T>();
    if size == 0 {
        return err::new_err_default(err::ZRO);
    }

    if size % 8 != 0 {
        return err::new_err_default(err::SZE);
    }

    Ok(())
}

mod err {
    use frozen_core::error::{ErrCode, FrozenError, FrozenResult};

    /// Domain Id for [`FrozenMMap`] is **20**
    const ERRDOMAIN: u8 = 0x14;

    /// module id used for [`FrozenMMap`]
    pub static MID: std::sync::OnceLock<u8> = std::sync::OnceLock::new();

    #[cfg(not(test))]
    #[inline(always)]
    pub fn mid() -> &'static u8 {
        MID.get().unwrap()
    }

    #[cfg(test)]
    #[inline(always)]
    pub fn mid() -> &'static u8 {
        MID.get_or_init(|| 0)
    }

    /// all copies are corrupted
    pub const CRP: ErrCode = ErrCode::new(0x02, "All copies of `T` are corrupted");

    /// invalid hash for T
    pub const HSH: ErrCode = ErrCode::new(0x04, "`T` has HASH mismatch as it may be updated after being stored");

    /// type `T` implements drop
    pub const DRP: ErrCode = ErrCode::new(0x06, "T must not implement Drop");

    /// type `T` is not 8 byte aligned
    pub const ALN: ErrCode = ErrCode::new(0x08, "T must be 8-byte aligned");

    /// type `T` is zero sized
    pub const ZRO: ErrCode = ErrCode::new(0x0A, "T must not be zero-sized");

    /// `size_of::<T>()` is not multiple of 8
    pub const SZE: ErrCode = ErrCode::new(0x0C, "T size must be multiple of 8");

    #[inline]
    pub fn new_err<R, E: std::fmt::Display>(code: ErrCode, error: E) -> FrozenResult<R> {
        let err = FrozenError::new_raw(*mid(), ERRDOMAIN, code, error);
        Err(err)
    }

    #[inline]
    pub fn new_err_default<R>(code: ErrCode) -> FrozenResult<R> {
        let err = FrozenError::new_raw(*mid(), ERRDOMAIN, code, "");
        Err(err)
    }
}
