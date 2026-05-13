//! Ṛta (ऋत) is a minimal metadata store for durable system state.
//!
//! ## The type `T`
//!
//! T must satisfy following layout and safety constraints:
//!
//! - Implements `RTA`
//! - Uses `#[repr(C)]`
//! - 8 bytes alignment
//! - Does not implements `Drop`
//! - Size must be >0 and multiple of 8
//! - Implements `Default + Clone + Send + Sync + 'static`
//!
//! #### Limitations of `T`
//!
//! - No non-deterministic feilds
//! - No interior pointers or self-references
//! - Changes made in struct layout break compatibility
//!
//! ## Example
//!
//! ```
//! use rta::{Rta, RTA};
//!
//! #[repr(C)]
//! #[derive(Default, Clone, Copy, RTA)]
//! struct TestType {
//!     a: u64,
//!     b: u64,
//! }
//!
//! const MOD_ID: u8 = 0;
//!
//! let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
//! let rta = Rta::<TestType, MOD_ID>::new(&path).unwrap();
//!
//! rta.write(|t| {
//!   t.a = 0x20;
//!   t.b = 0x40;
//! }).unwrap();
//!
//! let val = rta.read().unwrap();
//! assert_eq!(val.a, 0x20);
//! assert_eq!(val.b, 0x40);
//! ```
//!
//! ## Writes
//!
//! The [`Rta::write()`] operation in `Rta` is designed as a lightweight _fire-and-forget_ metadata update primitive.
//! This call itself does **not** wait for disk synchronization on the fast path, allowing extremely low write latency
//! while still preserving crash-safe durability semantics by handing off durability responsibility to a background
//! thread.
//!
//! #### Guarantees
//!
//! Following guarantees are provided,
//!
//! - serialized metadata updates
//! - crash-safe durability via multiple on-disk copies
//! - automatic recovery from torn writes using latest valid version
//!
//! #### Benchmarks
//!
//! | Metric  | Latency |
//! |:--------|:--------|
//! | Average | 83 ns   |
//! | P50     | 101 ns  |
//! | P90     | 102 ns  |
//! | P99     | 102 ns  |
//! | P999    | 102 ns  |
//!
//! #### Notes
//!
//! - Most writes complete in ~100ns on the uncontended fast path.
//! - The benchmark primarily measures,
//!   - lock acquisition
//!   - in-memory mutation
//!   - dirty state propagation
//! - Disk durability is handled asynchronously by a dedicated background thread.
//! - Tail latency may increase if writes overlap with active durability synchronization.
//!
//! ## Reads
//!
//! The [`Rta::read()`] operation returns the latest available in-memory metadata state.
//!
//! The read path is optimistic and does not provide durability guarantees at read time. Instead, reads are served
//! directly from the synchronized in-memory cache, avoiding any disk IO overhead.
//!
//! #### Guarantees
//!
//! Following guarantees are provided,
//!
//! - lock-safe concurrent access
//! - latest available in-memory metadata view
//! - no torn or partially visible updates
//! - parallel read operations
//!
//! #### Notes
//!
//! - Reads are performed entirely from the in-memory cache.
//! - No mmap scan or disk synchronization occurs during the read path.
//! - Read latency generally remains stable even during background durability synchronization.
//!
//! ## Concurrency Model
//!
//! | Operation        | Parallelism | Blocks Reads | Blocks Writes |
//! |:-----------------|-------------|--------------|---------------|
//! | **Read**         | Yes         | No           | No            |
//! | **Write**        | Limited     | No           | Sometimes     |

#![deny(missing_docs)]
#![allow(unsafe_op_in_unsafe_fn)]

use frozen_core::{
    crc32::Crc32C,
    error::{ErrCode, FrozenErr},
    fmmap::{FMCfg, FrozenMMap},
    hints,
};
use std::{
    slice,
    sync::{self, atomic},
    thread, time,
};

/// Procedural macro implementation for `#[derive(RTA)]`
pub use rta_derive::RTA;

/// Domain Id for [`Rta`] is **20**
const ERRDOMAIN: u8 = 0x14;

/// Number of copies of `T` stored on disk
const COPIES_ON_DISK: usize = 4;

/// flush duration for mmap
///
/// ## NOTE
///
/// This is just a placeholder value, as `Rta` does and must not rely on [`FrozenMMap`] for the flush. [`Rta`],
/// by itself performs instant durable writes via background thread, eliminating the need to rely on [`FrozenMMap`]
/// to provide durability.
const MMAP_FLUSH_DURATION: time::Duration = time::Duration::from_secs(1);

/// mmap config used by [`FrozenMMap`]
const MMAP_CONFIG: FMCfg = FMCfg {
    initial_count: COPIES_ON_DISK,
    flush_duration: MMAP_FLUSH_DURATION,
};

static MODULE_ID: sync::OnceLock<u8> = sync::OnceLock::new();
static CRC32: sync::OnceLock<Crc32C> = sync::OnceLock::new();

#[inline(always)]
fn mod_id() -> &'static u8 {
    MODULE_ID.get().expect("MID OnceLock is not initialized")
}

#[inline(always)]
fn crc32() -> &'static Crc32C {
    CRC32.get().expect("CRC32 OnceLock is not initialized")
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

    /// (1286) lock poisoned
    pub const LPN: ErrCode = ErrCode::new(0x506, "lock poisoned internally");
}

#[inline]
fn new_err<R>(code: ErrCode) -> RtaRes<R> {
    let err = FrozenErr::new(*mod_id(), ERRDOMAIN, code, "");
    Err(err.into())
}

#[inline]
fn new_err_raw<E: std::fmt::Display>(code: ErrCode, error: E) -> RtaErr {
    let err = FrozenErr::new_raw(*mod_id(), ERRDOMAIN, code, error);
    err.into()
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
pub struct Rta<T: RTA + Send + Sync + Clone, const MOD_ID: u8> {
    core: sync::Arc<Core<T, MOD_ID>>,
    handle: Option<thread::JoinHandle<()>>,
}

unsafe impl<T, const MOD_ID: u8> Send for Rta<T, MOD_ID> where T: RTA + Default + Send + Sync + Clone + 'static {}
unsafe impl<T, const MOD_ID: u8> Sync for Rta<T, MOD_ID> where T: RTA + Default + Send + Sync + Clone + 'static {}

impl<T, const MOD_ID: u8> Rta<T, MOD_ID>
where
    T: RTA + Default + Send + Sync + Clone + 'static,
{
    /// Create a new instance of [`Rta`]
    ///
    /// Given `path` must point to a file and not a directory
    ///
    /// ## Example
    ///
    /// ```
    /// use rta::{Rta, RTA};
    ///
    /// #[repr(C)]
    /// #[derive(Default, Clone, Copy, RTA)]
    /// struct TestType(u64);
    ///
    /// let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
    /// let rta = Rta::<TestType, 0>::new(&path).unwrap();
    ///
    /// rta.write(|t| {
    ///   t.0 = 0x0A;
    /// }).unwrap();
    ///
    /// let val = rta.read().unwrap();
    /// assert_eq!(val.0, 0x0A);
    /// ```
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> RtaRes<Self> {
        // NOTE: The value is used for error logging and is initialized only once, as `OnceLock` guarantees
        // that the first caller sets the value and all subsequent calls reuse it
        let _ = MODULE_ID.get_or_init(|| MOD_ID);

        // NOTE: The value is used across objects, we initialize it once and pin it in a global `OnceLock`
        // to avoid repeated setup and reference passing
        let _ = CRC32.get_or_init(|| Crc32C::default());

        // NOTE: we must validate `T` before mmap init, to avoid any UB errors
        validate_t::<T>()?;

        let mmap = FrozenMMap::<DiskObject<T>, MOD_ID>::new(path, MMAP_CONFIG)?;

        let (obj, version) = Self::init_or_create(&mmap)?;
        let cache = MemCache::new(obj, version);

        let core = Core::new(cache, mmap);
        let handle = Some(Core::spawn_flush_tx(core.clone()));

        Ok(Self { handle, core })
    }

    /// Push a write into metadata store
    ///
    /// This call is designed as _fire-and-forget_, while avoiding IO overhead
    ///
    /// ## Example
    ///
    /// ```
    /// use rta::{Rta, RTA};
    ///
    /// #[repr(C)]
    /// #[derive(Default, Clone, Copy, RTA)]
    /// struct TestType(u64, u64);
    ///
    /// let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
    /// let rta = Rta::<TestType, 0>::new(&path).unwrap();
    ///
    /// rta.write(|t| {
    ///   t.0 = 0x0A;
    /// }).unwrap();
    ///
    /// rta.write(|t| {
    ///   t.1 = 0x1A;
    /// }).unwrap();
    ///
    /// let val = rta.read().unwrap();
    /// assert_eq!(val.0, 0x0A);
    /// assert_eq!(val.1, 0x1A);
    /// ```
    #[inline(always)]
    pub fn write(&self, f: impl FnOnce(&mut T)) -> RtaRes<()> {
        if let Some(err) = self.core.get_sync_error() {
            return Err(err);
        }

        let mut guard = match self.core.guard.lock() {
            Ok(g) => g,
            Err(e) => {
                return Err(new_err_raw(err::LPN, e));
            }
        };

        let mut write_lock = match self.core.cache.write() {
            Ok(cache) => cache,
            Err(e) => {
                return Err(new_err_raw(err::LPN, e));
            }
        };

        f(&mut write_lock.obj);
        write_lock.version = write_lock.version.wrapping_add(1);

        if !(*guard) {
            (*guard) = true;
            self.core.cv.notify_one();
        }

        Ok(())
    }

    /// Read data from the metadata store
    ///
    /// The read is optimistic and does not provide durability gurantee at read time
    ///
    /// ## Example
    ///
    /// ```
    /// use rta::{Rta, RTA};
    ///
    /// #[repr(C)]
    /// #[derive(Default, Clone, Copy, RTA)]
    /// struct TestType(u64);
    ///
    /// let path = tempfile::NamedTempFile::new().unwrap().into_temp_path().to_path_buf();
    /// let rta = Rta::<TestType, 0>::new(&path).unwrap();
    ///
    /// rta.write(|t| {
    ///   t.0 = 0x100;
    /// }).unwrap();
    ///
    /// let val = rta.read().unwrap();
    /// assert_eq!(val.0, 0x100);
    /// ```
    #[inline(always)]
    pub fn read(&self) -> RtaRes<T> {
        if let Some(err) = self.core.get_sync_error() {
            return Err(err);
        }

        let guard = match self.core.cache.read() {
            Ok(g) => g,
            Err(e) => {
                return Err(new_err_raw(err::LPN, e));
            }
        };

        Ok(guard.obj.clone())
    }

    fn init_or_create(mmap: &FrozenMMap<DiskObject<T>, MOD_ID>) -> RtaRes<(T, u32)> {
        let mut seen_any = false;
        let mut seen_compatible = false;
        let mut best: Option<DiskObject<T>> = None;

        for i in 0..COPIES_ON_DISK {
            unsafe {
                mmap.read(i, |disk_object| {
                    let di = &*disk_object;

                    if di.hsh != 0 {
                        seen_any = true;
                    }

                    if !di.iseq_hsh(T::HASH) {
                        return;
                    }

                    seen_compatible = true;

                    let crc = crc32().crc(to_bytes(&di.obj));
                    if di.iseq_crc(crc) {
                        match &best {
                            Some(curr) if curr.ver >= di.ver => {}
                            _ => best = Some(di.clone()),
                        }
                    }
                })
            }?;
        }

        if let Some(b) = best {
            return Ok((b.obj, b.ver));
        }

        if seen_any && !seen_compatible {
            return new_err(err::HSH);
        }

        if seen_any {
            return new_err(err::CRP);
        }

        let def = T::default();
        let crc = crc32().crc(to_bytes(&def));
        let mut tx = mmap.new_tx();

        for i in 0..COPIES_ON_DISK {
            let idx = i;
            let ver = if idx == 0 { 1 } else { 0 };
            let obj = def.clone();

            unsafe {
                tx.write(idx, move |disk_object| {
                    let di = &mut (*disk_object);
                    di.ver = ver;
                    di.crc = crc;
                    di.hsh = T::HASH;
                    di.obj = obj;
                })
            }?;
        }

        let _ = tx.commit()?;
        Ok((def, 1))
    }
}

impl<T, const MOD_ID: u8> Drop for Rta<T, MOD_ID>
where
    T: RTA + Default + Send + Sync + Clone,
{
    fn drop(&mut self) {
        self.core.shutdown.store(true, atomic::Ordering::Release);
        self.core.cv.notify_all();

        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Core<T: RTA + Send + Sync + Clone, const MOD_ID: u8> {
    cv: sync::Condvar,
    guard: sync::Mutex<bool>,
    shutdown: atomic::AtomicBool,
    cache: sync::RwLock<MemCache<T>>,
    mmap: FrozenMMap<DiskObject<T>, MOD_ID>,
    error: atomic::AtomicPtr<sync::Arc<RtaErr>>,
}

impl<T, const MOD_ID: u8> Core<T, MOD_ID>
where
    T: RTA + Default + Send + Sync + Clone + 'static,
{
    fn new(cache: MemCache<T>, mmap: FrozenMMap<DiskObject<T>, MOD_ID>) -> sync::Arc<Self> {
        sync::Arc::new(Self {
            mmap,
            cv: sync::Condvar::new(),
            guard: sync::Mutex::new(false),
            cache: sync::RwLock::new(cache),
            shutdown: atomic::AtomicBool::new(false),
            error: atomic::AtomicPtr::new(std::ptr::null_mut()),
        })
    }

    fn spawn_flush_tx(core: sync::Arc<Core<T, MOD_ID>>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut idx = 0;
            loop {
                let mut guard = match core.guard.lock() {
                    Ok(g) => g,
                    Err(e) => {
                        core.set_sync_error(new_err_raw(err::LPN, e));
                        return;
                    }
                };

                while !(*guard) && !core.shutdown.load(atomic::Ordering::Acquire) {
                    guard = match core.cv.wait(guard) {
                        Ok(g) => g,
                        Err(e) => {
                            core.set_sync_error(new_err_raw(err::LPN, e));
                            return;
                        }
                    }
                }

                let write_lock = match core.cache.write() {
                    Ok(cache) => cache,
                    Err(e) => {
                        core.set_sync_error(new_err_raw(err::LPN, e));
                        return;
                    }
                };

                // NOTE: upon receiving the shutdown signal, we must exit the flush tx

                if core.shutdown.load(atomic::Ordering::Acquire) && !(*guard) {
                    return;
                }

                // NOTE: we snapshot current state, so we could resume read/write ops w/o any worries

                let ver = write_lock.version;
                let obj = write_lock.obj.clone();
                let crc = crc32().crc(to_bytes(&obj));

                // NOTE: by dropping the write guard early, we could allow read ops to go through, but write
                // ops will have to wait to acquire the guard (to mark dirty)
                drop(write_lock);

                match unsafe {
                    core.mmap.write_sync(idx % COPIES_ON_DISK, |disk_object| {
                        let di = &mut (*disk_object);

                        di.obj = obj;
                        di.ver = ver;
                        di.crc = crc;
                        di.hsh = T::HASH;
                    })
                } {
                    Ok(()) => {
                        (*guard) = false;
                        core.clear_sync_error();
                    }
                    Err(e) => {
                        core.set_sync_error(e.into());
                    }
                }

                idx = idx.wrapping_add(1);
            }
        })
    }

    #[inline(always)]
    fn set_sync_error(&self, err: RtaErr) {
        let boxed = Box::into_raw(Box::new(sync::Arc::new(err)));
        let old = self.error.swap(boxed, atomic::Ordering::AcqRel);

        // NOTE: we must free the old error, if any, to avoid mem leaks
        if !old.is_null() {
            unsafe { drop(Box::from_raw(old)) };
        }
    }

    #[inline(always)]
    fn get_sync_error(&self) -> Option<RtaErr> {
        let ptr = self.error.load(atomic::Ordering::Acquire);
        if hints::likely(ptr.is_null()) {
            return None;
        }

        let arc = unsafe { &*ptr }.clone();
        Some((*arc).clone())
    }

    #[inline]
    fn clear_sync_error(&self) {
        let old = self.error.swap(std::ptr::null_mut(), atomic::Ordering::AcqRel);
        if hints::unlikely(!old.is_null()) {
            unsafe {
                drop(Box::from_raw(old));
            }
        }
    }
}

struct MemCache<T: RTA> {
    obj: T,
    version: u32,
}

impl<T: RTA> MemCache<T> {
    #[inline]
    fn new(obj: T, version: u32) -> Self {
        Self { obj, version }
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
