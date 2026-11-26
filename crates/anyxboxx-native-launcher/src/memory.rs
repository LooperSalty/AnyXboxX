//! Windows allocation and protection of a mapped native PE image.

use crate::image::{Image, ImageError};
use std::cell::Cell;
use std::ffi::c_void;
use std::fmt;
use std::ptr::{self, NonNull};

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READONLY: u32 = 0x02;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READ: u32 = 0x20;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualAlloc(
        address: *const c_void,
        size: usize,
        flags: u32,
        protection: u32,
    ) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, flags: u32) -> i32;
    fn VirtualProtect(address: *const c_void, size: usize, protection: u32, old: *mut u32) -> i32;
    fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

#[derive(Debug)]
pub enum MemoryError {
    Allocation(std::io::Error),
    Relocation(ImageError),
    Protection(std::io::Error),
    AlreadyProtected,
    WrongImage,
    InvalidEntry,
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation(error) => write!(f, "mémoire de l'image indisponible: {error}"),
            Self::Relocation(error) => error.fmt(f),
            Self::Protection(error) => write!(f, "protection de l'image impossible: {error}"),
            Self::AlreadyProtected => f.write_str("image déjà protégée"),
            Self::WrongImage => f.write_str("taille d'image incohérente"),
            Self::InvalidEntry => f.write_str("point d'entrée de l'image invalide"),
        }
    }
}

impl std::error::Error for MemoryError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Writable,
    ProtectionFailed,
    Protected,
}

pub struct Allocation {
    base: NonNull<u8>,
    size: usize,
    state: Cell<State>,
}

impl Allocation {
    /// Reserve at the preferred base when possible, then relocate and copy the image.
    pub fn map(image: &mut Image) -> Result<Self, MemoryError> {
        let size = image.bytes.len();
        // SAFETY: Windows receives either the preferred address or null and owns the allocation.
        let base = unsafe {
            let preferred = VirtualAlloc(
                image.preferred_base as *const c_void,
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );
            if preferred.is_null() {
                VirtualAlloc(ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
            } else {
                preferred
            }
        };
        let base = NonNull::new(base.cast::<u8>())
            .ok_or_else(|| MemoryError::Allocation(std::io::Error::last_os_error()))?;
        let allocation = Self {
            base,
            size,
            state: Cell::new(State::Writable),
        };
        image
            .relocate(base.as_ptr() as u64)
            .map_err(MemoryError::Relocation)?;
        // SAFETY: the reserved writable allocation is exactly `size` bytes long.
        unsafe { ptr::copy_nonoverlapping(image.bytes.as_ptr(), base.as_ptr(), size) };
        Ok(allocation)
    }

    pub fn base(&self) -> *mut u8 {
        self.base.as_ptr()
    }

    /// Copy the image after imports are bound, before applying page protections.
    pub fn sync(&self, image: &Image) -> Result<(), MemoryError> {
        if self.state.get() != State::Writable {
            return Err(MemoryError::AlreadyProtected);
        }
        if image.bytes.len() != self.size {
            return Err(MemoryError::WrongImage);
        }
        // SAFETY: the unprotected allocation is writable and matches the image length.
        unsafe { ptr::copy_nonoverlapping(image.bytes.as_ptr(), self.base.as_ptr(), self.size) };
        Ok(())
    }

    /// Invoke the image entry point after imports, protections, exceptions and TLS are installed.
    ///
    /// # Safety
    /// The image must be trusted executable x86-64 code and all its runtime state must be ready.
    pub unsafe fn call_entry(&self, image: &Image) -> Result<i32, MemoryError> {
        let offset = image.entry_rva as usize;
        if self.state.get() != State::Protected
            || image.bytes.len() != self.size
            || offset >= self.size
        {
            return Err(MemoryError::InvalidEntry);
        }
        // SAFETY: caller guarantees this address is a valid entry point with the Windows ABI.
        let entry: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(self.base.as_ptr().add(offset)) };
        // SAFETY: caller guarantees the mapped code is trusted and initialized.
        Ok(unsafe { entry() })
    }

    /// Apply the same section protection choices as the native launcher.
    pub fn protect(&self, image: &Image) -> Result<(), MemoryError> {
        // A partial failure can leave some pages read-only, so further sync must be refused.
        self.state.set(State::ProtectionFailed);
        self.protect_range(0, image.headers_size as usize, PAGE_READONLY)?;
        for section in &image.sections {
            let length = section.virtual_size.max(section.raw_size) as usize;
            if length == 0 {
                continue;
            }
            let execute = section.characteristics & IMAGE_SCN_MEM_EXECUTE != 0;
            let write = section.characteristics & IMAGE_SCN_MEM_WRITE != 0;
            let protection = match (execute, write) {
                (true, true) => PAGE_EXECUTE_READWRITE,
                (true, false) => PAGE_EXECUTE_READ,
                (false, true) => PAGE_READWRITE,
                (false, false) => PAGE_READONLY,
            };
            self.protect_range(section.address as usize, length, protection)?;
        }
        // SAFETY: the process pseudo-handle and allocation address are valid.
        if unsafe {
            FlushInstructionCache(GetCurrentProcess(), self.base.as_ptr().cast(), self.size)
        } == 0
        {
            return Err(MemoryError::Protection(std::io::Error::last_os_error()));
        }
        self.state.set(State::Protected);
        Ok(())
    }

    fn protect_range(
        &self,
        offset: usize,
        length: usize,
        protection: u32,
    ) -> Result<(), MemoryError> {
        if offset.checked_add(length).is_none_or(|end| end > self.size) {
            return Err(MemoryError::Protection(std::io::Error::from(
                std::io::ErrorKind::InvalidData,
            )));
        }
        let mut old = 0;
        // SAFETY: the range was checked against the live allocation.
        let result = unsafe {
            VirtualProtect(
                self.base.as_ptr().add(offset).cast(),
                length,
                protection,
                &mut old,
            )
        };
        if result == 0 {
            Err(MemoryError::Protection(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: this is the base of a live VirtualAlloc reservation owned by this object.
        unsafe {
            VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}
