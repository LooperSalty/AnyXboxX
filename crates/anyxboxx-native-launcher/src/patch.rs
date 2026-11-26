//! Patch the import address table of a DLL loaded by Windows.

use std::ffi::c_void;
use std::ptr;

const PAGE_READWRITE: u32 = 4;
const ORDINAL: u64 = 0x8000_0000_0000_0000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualProtect(address: *const c_void, size: usize, protection: u32, old: *mut u32) -> i32;
}

#[derive(Debug, PartialEq, Eq)]
pub enum PatchError {
    MalformedImports,
    Protection,
}

struct Mapped {
    base: *mut u8,
    size: usize,
}

impl Mapped {
    fn range(&self, at: usize, length: usize) -> Result<*mut u8, PatchError> {
        if at.checked_add(length).is_none_or(|end| end > self.size) {
            return Err(PatchError::MalformedImports);
        }
        // SAFETY: the requested range is inside the caller-provided mapping.
        Ok(unsafe { self.base.add(at) })
    }
    fn u32(&self, at: usize) -> Result<u32, PatchError> {
        // SAFETY: range checks precede this unaligned read.
        Ok(u32::from_le(unsafe {
            self.range(at, 4)?.cast::<u32>().read_unaligned()
        }))
    }
    fn u64(&self, at: usize) -> Result<u64, PatchError> {
        // SAFETY: range checks precede this unaligned read.
        Ok(u64::from_le(unsafe {
            self.range(at, 8)?.cast::<u64>().read_unaligned()
        }))
    }
    fn text(&self, at: usize) -> Result<Vec<u8>, PatchError> {
        let mut result = Vec::new();
        for offset in at..self.size {
            // SAFETY: each byte is checked against the mapping bounds.
            let byte = unsafe { self.range(offset, 1)?.read() };
            if byte == 0 {
                return Ok(result);
            }
            result.push(byte);
        }
        Err(PatchError::MalformedImports)
    }
    fn write_pointer(&self, at: usize, target: usize) -> Result<(), PatchError> {
        let slot = self.range(at, 8)?;
        let mut old = 0;
        // SAFETY: the slot belongs to the mapped image; Windows manages page protection.
        if unsafe { VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) } == 0 {
            return Err(PatchError::Protection);
        }
        // SAFETY: the slot is now writable and can be unaligned in malformed images.
        unsafe {
            ptr::write_unaligned(slot.cast::<u64>(), (target as u64).to_le());
        }
        let mut ignored = 0;
        // SAFETY: restores the previous protection for the same address and size.
        if unsafe { VirtualProtect(slot.cast(), 8, old, &mut ignored) } == 0 {
            return Err(PatchError::Protection);
        }
        Ok(())
    }
}

/// Patch named DLL imports selected by `redirect`, leaving ordinal imports untouched.
///
/// # Safety
/// `base` must refer to a live mapped PE image of at least `size` bytes. No other thread
/// may mutate its import tables during this call.
pub unsafe fn patch_imports(
    base: *mut u8,
    size: usize,
    mut redirect: impl FnMut(&[u8], &[u8]) -> Option<usize>,
) -> Result<(), PatchError> {
    let image = Mapped { base, size };
    let nt = image.u32(60)? as usize;
    let optional = nt.checked_add(24).ok_or(PatchError::MalformedImports)?;
    let directory = image.u32(
        optional
            .checked_add(120)
            .ok_or(PatchError::MalformedImports)?,
    )? as usize;
    if directory == 0 {
        return Ok(());
    }
    let mut descriptor = directory;
    loop {
        let original = image.u32(descriptor)? as usize;
        let dll_name = image.u32(
            descriptor
                .checked_add(12)
                .ok_or(PatchError::MalformedImports)?,
        )? as usize;
        let slots = image.u32(
            descriptor
                .checked_add(16)
                .ok_or(PatchError::MalformedImports)?,
        )? as usize;
        if dll_name == 0 {
            break;
        }
        let dll = image.text(dll_name)?;
        let mut names = if original == 0 { slots } else { original };
        let mut slot = slots;
        loop {
            let value = image.u64(names)?;
            if value == 0 {
                break;
            }
            if value & ORDINAL == 0 {
                let at = usize::try_from(value)
                    .ok()
                    .and_then(|v| v.checked_add(2))
                    .ok_or(PatchError::MalformedImports)?;
                let name = image.text(at)?;
                if let Some(target) = redirect(&dll, &name).filter(|&p| p != 0) {
                    image.write_pointer(slot, target)?;
                }
            }
            names = names.checked_add(8).ok_or(PatchError::MalformedImports)?;
            slot = slot.checked_add(8).ok_or(PatchError::MalformedImports)?;
        }
        descriptor = descriptor
            .checked_add(20)
            .ok_or(PatchError::MalformedImports)?;
    }
    Ok(())
}
