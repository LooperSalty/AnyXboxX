//! Register x64 exception metadata and install the image's implicit TLS.

use crate::{image::Image, libraries::Module, memory::Allocation};
use std::ffi::c_void;
use std::fmt;

const EXCEPTION_DIRECTORY: usize = 3;
const TLS_DIRECTORY: usize = 9;
const RUNTIME_FUNCTION_SIZE: u32 = 12;
const TLS_DIRECTORY_SIZE: usize = 40;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn RtlAddFunctionTable(table: *mut c_void, count: u32, base: u64) -> u8;
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegistrationError {
    InvalidExceptionDirectory,
    InvalidTlsDirectory,
    TlsInstallation,
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidExceptionDirectory => "table d'exceptions invalide",
            Self::InvalidTlsDirectory => "répertoire TLS invalide",
            Self::TlsInstallation => "installation de la TLS implicite impossible",
        })
    }
}
impl std::error::Error for RegistrationError {}

fn valid_range(address: u32, length: usize, image_size: usize) -> bool {
    (address as usize)
        .checked_add(length)
        .is_some_and(|end| end <= image_size)
}

/// Register the exception table and invoke the TLS carrier export when the image has TLS.
///
/// # Safety
/// The allocation must contain this image and remain mapped for the process lifetime;
/// the TLS callback may execute code from the mapped image.
pub unsafe fn register(
    image: &Image,
    allocation: &Allocation,
    tls_module: Module,
) -> Result<(), RegistrationError> {
    let (address, size) = image
        .directory(EXCEPTION_DIRECTORY)
        .expect("known directory");
    if address != 0 && size >= RUNTIME_FUNCTION_SIZE {
        if !valid_range(address, size as usize, image.bytes.len()) {
            return Err(RegistrationError::InvalidExceptionDirectory);
        }
        // SAFETY: caller guarantees the image is mapped and retained; the table was bounds checked.
        unsafe {
            RtlAddFunctionTable(
                allocation.base().add(address as usize).cast(),
                size / RUNTIME_FUNCTION_SIZE,
                allocation.base() as u64,
            );
        }
    }
    let (address, _) = image.directory(TLS_DIRECTORY).expect("known directory");
    if address == 0 {
        return Ok(());
    }
    if !valid_range(address, TLS_DIRECTORY_SIZE, image.bytes.len()) {
        return Err(RegistrationError::InvalidTlsDirectory);
    }
    let install = tls_module
        .export(c"AnyXboxX_InstallImageTls")
        .ok_or(RegistrationError::TlsInstallation)?;
    type Install = unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32;
    // SAFETY: the TLS carrier exports this exact ABI; the image directory is in bounds.
    let install: Install = unsafe { std::mem::transmute(install.as_ptr()) };
    // SAFETY: caller guarantees image lifetime and permits callbacks from the loaded image.
    let result = unsafe {
        install(
            allocation.base().cast(),
            allocation.base().add(address as usize).cast(),
        )
    };
    if result == 0 {
        Err(RegistrationError::TlsInstallation)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_directory_bounds_without_integer_overflow() {
        assert!(valid_range(0x1000, TLS_DIRECTORY_SIZE, 0x2000));
        assert!(!valid_range(0x1fff, TLS_DIRECTORY_SIZE, 0x2000));
        assert!(!valid_range(u32::MAX, usize::MAX, usize::MAX));
    }
}
