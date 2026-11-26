//! Windows DLL loading primitives used by the native launcher.

use crate::redirect::{RUNTIME_DLLS, prefer_tombox};
use std::ffi::{CStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::{self, NonNull};

pub const LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR: u32 = 0x100;
pub const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// A module kept loaded for the process lifetime, as in the C++ launcher.
#[derive(Clone, Copy, Debug)]
pub struct Module(NonNull<c_void>);

impl Module {
    pub fn handle(self) -> *mut c_void {
        self.0.as_ptr()
    }
    pub fn load(path: &Path, flags: u32) -> std::io::Result<Self> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is NUL-terminated and alive for the call; the reserved file handle is null.
        let result = unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), flags) };
        NonNull::new(result)
            .map(Self)
            .ok_or_else(std::io::Error::last_os_error)
    }

    pub fn export(self, name: &CStr) -> Option<NonNull<c_void>> {
        // SAFETY: the module is kept loaded and the C string is NUL-terminated.
        NonNull::new(unsafe { GetProcAddress(self.0.as_ptr(), name.as_ptr().cast()) })
    }

    pub fn ordinal(self, number: u16) -> Option<NonNull<c_void>> {
        // SAFETY: Windows interprets integer resources at most 0xffff as import ordinals.
        NonNull::new(unsafe { GetProcAddress(self.0.as_ptr(), usize::from(number) as *const u8) })
    }
}

/// Select runtime paths in load order, allowing a caller to supply the existence check.
pub fn runtime_paths(
    runtime: &Path,
    tombox: &Path,
    d3d12_override: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> [PathBuf; 6] {
    std::array::from_fn(|index| {
        if index == 2
            && let Some(path) = d3d12_override
        {
            return path.to_owned();
        }
        let candidate = tombox.join(RUNTIME_DLLS[index]);
        if prefer_tombox(index) && exists(&candidate) {
            candidate
        } else {
            runtime.join(RUNTIME_DLLS[index])
        }
    })
}

/// Load all six required runtime DLLs; a failure names the path and preserves the OS error.
pub fn load_runtime(paths: &[PathBuf; 6]) -> Result<[Module; 6], (PathBuf, std::io::Error)> {
    let mut modules = Vec::with_capacity(paths.len());
    for path in paths {
        let module = Module::load(
            path,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .map_err(|error| (path.clone(), error))?;
        modules.push(module);
    }
    Ok(modules.try_into().expect("six paths produce six modules"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_priority_matches_native_launcher() {
        let runtime = Path::new(r"C:\runtime");
        let tombox = Path::new(r"C:\tombox");
        let override_path = Path::new(r"C:\override\d3d12_xs.dll");
        let paths = runtime_paths(runtime, tombox, Some(override_path), |_| true);
        assert_eq!(paths[0], runtime.join("xgameruntime.dll"));
        assert_eq!(paths[1], tombox.join("xmem.dll"));
        assert_eq!(paths[2], override_path);
        assert_eq!(paths[5], tombox.join("XCRDAPI.dll"));
        let absent = runtime_paths(runtime, tombox, None, |_| false);
        assert_eq!(absent[1], runtime.join("xmem.dll"));
    }

    #[test]
    fn loads_system_library_and_resolves_named_exports() {
        let module = Module::load(Path::new("kernel32.dll"), LOAD_LIBRARY_SEARCH_SYSTEM32).unwrap();
        assert!(module.export(c"GetCurrentProcessId").is_some());
        assert!(module.export(c"AnyXboxXMissingExportForTest").is_none());
        assert!(
            Module::load(
                Path::new("AnyXboxXMissingLibraryForTest.dll"),
                LOAD_LIBRARY_SEARCH_SYSTEM32
            )
            .is_err()
        );
    }
}
