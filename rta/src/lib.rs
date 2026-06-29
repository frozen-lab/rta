// #![deny(missing_docs)]
#![allow(unsafe_op_in_unsafe_fn)]

use frozen_core::{crc32, error, fmmap};
use std::{slice, sync::atomic, time};

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
    cfg: RtaCfg,
    crc32c: crc32::Crc32C,
    version: atomic::AtomicU32,
    live_index: atomic::AtomicUsize,
    mmap: fmmap::FrozenMMap<DiskObject<T>>,
}

unsafe impl<T> Send for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}
unsafe impl<T> Sync for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}

impl<T> Rta<T>
where
    T: RTA + Send + Sync + Clone + 'static,
{
    pub fn new(cfg: RtaCfg) -> error::FrozenResult<Self> {
        // NOTE: The value is used for error logging and is initialized only once, as `OnceLock`
        // guarantees that the first caller sets the value and all subsequent calls reuse it
        let _ = err::MID.get_or_init(|| cfg.module_id);

        // NOTE: we must validate `T` before mmap init, to avoid any UB errors
        validate_t::<T>()?;

        let mmap = fmmap::FrozenMMap::new(
            cfg.path.clone(),
            fmmap::FrozenMMapCfg {
                module_id: cfg.module_id,
                immediate_durability: true,
                initial_count: cfg.copies_on_disk as usize,
                flush_duration: MMAP_FLUSH_DURATION,
            },
        )?;

        let crc32c = crc32::Crc32C::new();
        let (index, version) = Self::init(cfg.copies_on_disk, &crc32c, &mmap)?;

        Ok(Self {
            cfg,
            mmap,
            crc32c,
            version: atomic::AtomicU32::new(version),
            live_index: atomic::AtomicUsize::new(index),
        })
    }

    #[inline(always)]
    pub unsafe fn write(&self, f: impl FnOnce(&mut T)) -> error::FrozenResult<()> {
        let next_version = self.version.load(atomic::Ordering::Acquire).wrapping_add(1);
        let new_index = self.live_index.load(atomic::Ordering::Acquire) % self.cfg.copies_on_disk;

        let _ticket = self.mmap.write(new_index, |entry| {
            let di = &mut (*entry);

            di.hsh = T::HASH;
            di.ver = next_version;

            f(&mut di.obj);
            di.crc = self.crc32c.crc(to_bytes(&di.obj));
        })?;

        self.version.store(next_version, atomic::Ordering::Release);
        Ok(())
    }

    #[inline]
    pub unsafe fn read(&self) -> T {
        let live_index = self.live_index.load(atomic::Ordering::Acquire);

        // CRITICAL: The use of `unwrap` does no harm whatsoever as the `read` call does not return any
        // sort of error and the use of `FrozenResult` is result of a mishap in the impl. When the update
        // is published in `frozen_core` crate, this unwrap will be eliminated!!
        //
        // Issue in context => `https://github.com/frozen-lab/frozen-core/issues/76`

        self.mmap.read(live_index, |entry| {
            let di = &*entry;
            di.obj.clone()
        })
    }

    #[inline]
    pub fn delete(&mut self) -> error::FrozenResult<()> {
        todo!()
    }

    fn init(
        copies_on_disk: usize,
        crc32c: &crc32::Crc32C,
        mmap: &fmmap::FrozenMMap<DiskObject<T>>,
    ) -> error::FrozenResult<(usize, u32)> {
        if let Some(best) = Self::init_checks(copies_on_disk, crc32c, mmap)? {
            return Ok(best);
        }

        Self::init_copies_on_disk(copies_on_disk, crc32c, mmap)?;
        Ok((0usize, 0u32))
    }

    fn init_checks(
        copies_on_disk: usize,
        crc32c: &crc32::Crc32C,
        mmap: &fmmap::FrozenMMap<DiskObject<T>>,
    ) -> error::FrozenResult<Option<(usize, u32)>> {
        let mut best = None;
        let mut seen_any = false;
        let mut seen_compatible = false;

        let current_entries_on_disk = mmap.total_slots();
        if current_entries_on_disk < copies_on_disk {
            return Err(err::new_err_default(err::CRP));
        }

        for index in 0..copies_on_disk {
            unsafe {
                mmap.read(index, |entry| {
                    let di = &*entry;

                    if di.hsh != 0 {
                        seen_any = true;
                    }

                    if !di.iseq_hsh(T::HASH) {
                        return;
                    }

                    seen_compatible = true;

                    let crc = crc32c.crc(&to_bytes(&di.obj));
                    if di.iseq_crc(crc) {
                        best = Some((index, di.ver));
                    }
                })
            };
        }

        if best.is_some() {
            return Ok(best);
        }

        if seen_any && !seen_compatible {
            return Err(err::new_err_default(err::HSH));
        }

        if seen_any {
            return Err(err::new_err_default(err::CRP));
        }

        Ok(None)
    }

    fn init_copies_on_disk(
        copies_on_disk: usize,
        crc32c: &crc32::Crc32C,
        mmap: &fmmap::FrozenMMap<DiskObject<T>>,
    ) -> error::FrozenResult<()> {
        let def_obj = T::default();
        let crc = crc32c.crc(to_bytes(&def_obj));

        let mut transaction = mmap.new_tx();
        for i in 0..copies_on_disk {
            let index = i;
            let obj = def_obj.clone();

            unsafe {
                transaction.write(index, move |entry| {
                    let di = &mut (*entry);

                    di.crc = crc;
                    di.obj = obj;
                    di.hsh = T::HASH;
                    di.ver = if index == 0 { 1 } else { 0 };
                })
            }?;
        }

        let ticket = transaction.commit()?;
        let _ = ticket.wait()?;

        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DiskObject<T: RTA + Send + Sync + Clone + 'static> {
    ver: u32,
    crc: u32,
    hsh: u64,
    obj: T,
}

impl<T> DiskObject<T>
where
    T: RTA + Send + Sync + Clone + 'static,
{
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
fn to_bytes<T: RTA + 'static>(t: &T) -> &[u8] {
    unsafe { slice::from_raw_parts(t as *const T as *const u8, T::SIZE) }
}

#[inline(always)]
fn validate_t<T: RTA + 'static>() -> error::FrozenResult<()> {
    if std::mem::needs_drop::<T>() {
        return Err(err::new_err_default(err::DRP));
    }

    let align = std::mem::align_of::<T>();
    if align != 8 {
        return Err(err::new_err_default(err::ALN));
    }

    let size = std::mem::size_of::<T>();
    if size == 0 {
        return Err(err::new_err_default(err::ZRO));
    }

    if size % 8 != 0 {
        return Err(err::new_err_default(err::SZE));
    }

    Ok(())
}

mod err {
    use frozen_core::error::{ErrCode, FrozenError};

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
    pub fn new_err_default(code: ErrCode) -> FrozenError {
        FrozenError::new_raw(*mid(), ERRDOMAIN, code, "")
    }
}
