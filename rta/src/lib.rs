// #![deny(missing_docs)]
#![allow(unsafe_op_in_unsafe_fn)]

use frozen_core::{ack, crc32, error, fmmap};
use std::{slice, sync::atomic, time};

/// Procedural macro implementation for `#[derive(RTA)]`
pub use rta_derive::RTA;

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

#[derive(Debug, Clone)]
pub struct RtaCfg {
    pub module_id: u8,
    pub copies_on_disk: usize,
    pub path: std::path::PathBuf,
}

pub struct Rta<T: RTA + Send + Sync + Clone + 'static> {
    cfg: RtaCfg,
    crc32c: crc32::Crc32C,
    version: atomic::AtomicU32,
    published_version: atomic::AtomicU32,
    mmap: fmmap::FrozenMMap<DiskObject<T>>,
}

unsafe impl<T> Send for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}
unsafe impl<T> Sync for Rta<T> where T: RTA + Send + Sync + Clone + 'static {}

impl<T> Rta<T>
where
    T: RTA + Send + Sync + Clone + 'static,
{
    pub fn new(cfg: RtaCfg) -> error::FrozenResult<Self> {
        // sanity check
        assert!(cfg.copies_on_disk > 1, "Copies on disk must be greater then 1");
        assert!(
            cfg.copies_on_disk < u32::MAX as usize,
            "Copies on disk must be smaller then u32::MAX"
        );

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
        let version = Self::init(cfg.copies_on_disk, &crc32c, &mmap)?;

        Ok(Self {
            cfg,
            mmap,
            crc32c,
            version: atomic::AtomicU32::new(version),
            published_version: atomic::AtomicU32::new(version),
        })
    }

    #[inline(always)]
    pub unsafe fn write(&self, f: impl FnOnce(&mut T)) -> error::FrozenResult<ack::AckTicket> {
        let new_version = self.version.fetch_add(1, atomic::Ordering::AcqRel).wrapping_add(1);
        let live_index = new_version as usize % self.cfg.copies_on_disk;

        let ticket = self.mmap.write(live_index, |entry| {
            let di = &mut (*entry);

            di.hsh = T::HASH;
            di.ver = new_version;

            f(&mut di.obj);
            di.crc = self.crc32c.crc(to_bytes(&di.obj));
        })?;

        self.published_version.store(new_version, atomic::Ordering::Release);
        Ok(ticket)
    }

    #[inline]
    pub unsafe fn read(&self) -> T {
        let live_version = self.published_version.load(atomic::Ordering::Acquire);
        let live_index = live_version as usize % self.cfg.copies_on_disk;

        self.mmap.read(live_index, |entry| {
            let di = &*entry;
            di.obj.clone()
        })
    }

    #[inline]
    pub fn delete(&mut self) -> error::FrozenResult<()> {
        self.mmap.delete()
    }

    fn init(
        copies_on_disk: usize,
        crc32c: &crc32::Crc32C,
        mmap: &fmmap::FrozenMMap<DiskObject<T>>,
    ) -> error::FrozenResult<u32> {
        if let Some(best) = Self::init_checks(copies_on_disk, crc32c, mmap)? {
            return Ok(best);
        }

        Self::init_copies_on_disk(copies_on_disk, crc32c, mmap)
    }

    fn init_checks(
        copies_on_disk: usize,
        crc32c: &crc32::Crc32C,
        mmap: &fmmap::FrozenMMap<DiskObject<T>>,
    ) -> error::FrozenResult<Option<u32>> {
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
                        match best {
                            Some(version) if !is_newer_version(di.ver, version) => {}
                            _ => best = Some(di.ver),
                        }
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
    ) -> error::FrozenResult<u32> {
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

        // NOTE: On new init, we start version from `1` instead of `0` cause we seed the very first
        // on-disk copy (at index 0) w/ version `1`. Returning `0` would cause the first user write
        // to also allocate the version `1`, resulting in two valid copies w/ same version, i.e.
        // entry at index 0 and also at index 1 would have `ver` set to 1.
        //
        // During recovery, this makes it impossible to deterministically identify the latest copy,
        // as the both the entries copare equal. By starting w/ `1`, we elimiate this scenerio,
        // where the first user write starts from `ver` set to `2`.
        Ok(1u32)
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

#[inline]
fn is_newer_version(a: u32, b: u32) -> bool {
    let diff = a.wrapping_sub(b);
    diff != 0 && diff < (1 << 0x1F)
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
    pub const HSH: ErrCode =
        ErrCode::new(0x04, "`T` has HASH mismatch as it may be updated after being stored");

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_forward() {
        assert!(is_newer_version(2, 1));
        assert!(is_newer_version(0x64, 0x63));
    }

    #[test]
    fn err_equal() {
        assert!(!is_newer_version(0x0A, 0x0A));
    }

    #[test]
    fn err_older() {
        assert!(!is_newer_version(5, 6));
    }

    #[test]
    fn ok_wraparound() {
        assert!(is_newer_version(0, u32::MAX));
        assert!(is_newer_version(1, u32::MAX));
    }

    #[test]
    fn err_half_range() {
        assert!(!is_newer_version(1 << 0x1F, 0));
    }
}
