use core::{marker::PhantomData, slice};
use frozen_core::{
    fe::FRes,
    ff::{FFCfg, FF},
    fm::{FMCfg, FM},
};

const MOD_ID: u8 = 0x01;
const FLUSH_DURATION: std::time::Duration = std::time::Duration::from_millis(250);

pub use rta_derive::RTA;

pub unsafe trait RTA: Clone + Sized + Default {
    const HASH: u64;
}

pub struct Rta<T: RTA> {
    mmap: FM,
    _file: FF,
    _marker: PhantomData<T>,
    lock: std::sync::Mutex<()>,
}

impl<T> Rta<T>
where
    T: RTA + Clone + Sized + Default,
{
    const FILE_SIZE: usize = core::mem::size_of::<DiskInterface<T>>();

    pub fn new(path: std::path::PathBuf) -> FRes<Self> {
        if path.exists() {
            panic!("invalid path, path to already existing file");
        }

        if path.is_dir() {
            panic!("path must be of a file, not dir");
        }

        let file_cfg = FFCfg {
            path,
            auto_flush: false,
            module_id: MOD_ID,
            flush_duration: FLUSH_DURATION,
        };
        let mmap_cfg = FMCfg {
            module_id: MOD_ID,
            auto_flush: true,
            flush_duration: FLUSH_DURATION,
        };

        let _file = FF::new(file_cfg, Self::FILE_SIZE as u64)?;
        let mmap = FM::new(_file.fd(), Self::FILE_SIZE, mmap_cfg)?;

        {
            let writer = mmap.writer::<DiskInterface<T>>(0)?;
            writer.write(|di| {
                di.hash = T::HASH;

                di.obja.obj = T::default();
                di.obja.ver = 1;
                di.obja.crc = crc32(Self::to_bytes(&di.obja.obj));

                di.objb = di.obja.clone();
            })?;
        }

        Ok(Self {
            _file,
            mmap,
            _marker: PhantomData,
            lock: std::sync::Mutex::new(()),
        })
    }

    pub fn open(path: std::path::PathBuf) -> FRes<Self> {
        if !path.exists() {
            panic!("Rta does not exists");
        }

        if !path.is_file() {
            panic!("Path is not a file");
        }

        let file_cfg = FFCfg {
            path,
            auto_flush: false,
            module_id: MOD_ID,
            flush_duration: FLUSH_DURATION,
        };
        let mmap_cfg = FMCfg {
            module_id: MOD_ID,
            auto_flush: true,
            flush_duration: FLUSH_DURATION,
        };

        let _file = FF::open(file_cfg)?;
        let mmap = FM::new(_file.fd(), Self::FILE_SIZE, mmap_cfg)?;

        {
            let r = mmap.reader::<DiskInterface<T>>(0)?;
            r.read(|di| {
                if di.hash != T::HASH {
                    panic!("metadata hash mismatch");
                }

                let a = Self::valid(&di.obja);
                let b = Self::valid(&di.objb);

                if !a && !b {
                    panic!("both metadata copies corrupt");
                }
            });
        }

        Ok(Self {
            _file,
            mmap,
            _marker: PhantomData,
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
            target.crc = crc32(Self::to_bytes(&target.obj));
        })?;

        Ok(())
    }

    #[inline]
    fn to_bytes(t: &T) -> &[u8] {
        unsafe { slice::from_raw_parts(t as *const T as *const u8, Self::size()) }
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
        crc32(Self::to_bytes(&obj.obj)) == obj.crc
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

#[repr(C)]
#[derive(Clone)]
struct DiskObject<T: RTA> {
    obj: T,
    ver: u32,
    crc: u32,
}
