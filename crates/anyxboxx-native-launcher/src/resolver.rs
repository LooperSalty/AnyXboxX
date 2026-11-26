//! Resolve PE imports against runtime, local, TomBox and Windows DLLs.

use crate::hooks;
use crate::image::Image;
use crate::imports::{self, BindError, Import, Symbol};
use crate::libraries::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Module};
use crate::patch;
use crate::redirect::{Redirect, redirect, runtime_for};
use std::collections::HashMap;
use std::ffi::{CString, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn MultiByteToWideChar(
        code: u32,
        flags: u32,
        src: *const u8,
        len: i32,
        dst: *mut u16,
        cap: i32,
    ) -> i32;
}

pub fn widen(value: &[u8]) -> Option<OsString> {
    let length = i32::try_from(value.len()).ok()?;
    if length == 0 {
        return Some(OsString::new());
    }
    // SAFETY: input is a valid byte slice; a null output requests the required buffer size.
    let count = unsafe { MultiByteToWideChar(0, 0, value.as_ptr(), length, ptr::null_mut(), 0) };
    if count == 0 {
        return None;
    }
    let mut buffer = vec![0; count as usize];
    // SAFETY: the buffer has the size obtained from the preceding call.
    if unsafe { MultiByteToWideChar(0, 0, value.as_ptr(), length, buffer.as_mut_ptr(), count) } == 0
    {
        return None;
    }
    Some(OsString::from_wide(&buffer))
}

#[derive(Debug)]
pub enum ResolveError {
    InvalidTable,
    Missing(Vec<Import>),
    LocalModulePatching(PathBuf),
}

fn redirect_address(
    dll: &[u8],
    name: &[u8],
    runtime: &[Module; 6],
    game_dll: bool,
    measuring: bool,
) -> Option<usize> {
    let dll = widen(dll)?;
    match redirect(&dll.to_string_lossy(), name, game_dll, measuring) {
        Some(Redirect::RuntimeExport(name)) => runtime[0]
            .export(&CString::new(name).ok()?)
            .map(|p| p.as_ptr() as usize),
        Some(Redirect::Shim) => hooks::shim(name),
        None => None,
    }
}

/// Resolve imports into the writable image and patch local DLL imports.
pub fn bind(
    image: &mut Image,
    runtime: &[Module; 6],
    game_folder: &Path,
    tombox_folder: &Path,
    measuring: bool,
) -> Result<(), ResolveError> {
    let entries = imports::imports(image).map_err(|_| ResolveError::InvalidTable)?;
    let mut modules = HashMap::new();
    for entry in &entries {
        if modules.contains_key(&entry.dll) {
            continue;
        }
        let Some(name) = widen(&entry.dll) else {
            modules.insert(entry.dll.clone(), None);
            continue;
        };
        let display = name.to_string_lossy();
        let module = if let Some(index) = runtime_for(&display) {
            Some(runtime[index])
        } else {
            let local = game_folder.join(&name);
            if local.exists() {
                let module = Module::load(&local, 8).ok();
                if let Some(module) = module {
                    let base = module.handle().cast::<u8>();
                    // SAFETY: Windows has loaded this module as a valid PE image.
                    let nt = unsafe { base.add(60).cast::<u32>().read_unaligned() } as usize;
                    // SAFETY: Windows verified the NT headers when loading the DLL.
                    let size =
                        unsafe { base.add(nt + 24 + 56).cast::<u32>().read_unaligned() } as usize;
                    // SAFETY: this loaded module remains mapped; initialization is single-threaded here.
                    unsafe {
                        patch::patch_imports(base, size, |dll, symbol| {
                            redirect_address(dll, symbol, runtime, true, measuring)
                        })
                    }
                    .map_err(|_| ResolveError::LocalModulePatching(local.clone()))?;
                }
                modules.insert(entry.dll.clone(), module);
                continue;
            }
            let basename = display
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("")
                .to_lowercase();
            let tombox = tombox_folder.join(basename);
            if tombox.exists() {
                Module::load(
                    &tombox,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
                .ok()
            } else {
                Module::load(Path::new(&name), 0).ok()
            }
        };
        modules.insert(entry.dll.clone(), module);
    }
    match imports::bind(image, |entry| {
        let module = modules.get(&entry.dll).copied().flatten();
        match &entry.symbol {
            Symbol::Ordinal(number) => module?.ordinal(*number).map(|p| p.as_ptr() as u64),
            Symbol::Name(name) => {
                let replacement =
                    redirect_address(&entry.dll, name, runtime, false, measuring).map(|p| p as u64);
                replacement.or_else(|| {
                    module?
                        .export(&CString::new(name.as_slice()).ok()?)
                        .map(|p| p.as_ptr() as u64)
                })
            }
        }
    }) {
        Ok(()) => Ok(()),
        Err(BindError::InvalidTable) => Err(ResolveError::InvalidTable),
        Err(BindError::Missing(missing)) => Err(ResolveError::Missing(missing)),
    }
}
