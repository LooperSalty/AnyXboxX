//! Assemble the native image loader and call the mapped entry point.

use crate::{
    command_line, exceptions, hooks,
    image::Image,
    imports::Symbol,
    libraries::Module,
    memory::{Allocation, MemoryError},
    registration, resolver,
};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

fn resolution_error(error: resolver::ResolveError) -> String {
    match error {
        resolver::ResolveError::InvalidTable => "table d'imports invalide".to_owned(),
        resolver::ResolveError::LocalModulePatching(path) => {
            format!("interception de DLL impossible : {}", path.display())
        }
        resolver::ResolveError::Missing(imports) => {
            let mut result = "imports introuvables :".to_owned();
            for import in imports {
                let dll = String::from_utf8_lossy(&import.dll);
                let symbol = match import.symbol {
                    Symbol::Name(name) => String::from_utf8_lossy(&name).into_owned(),
                    Symbol::Ordinal(number) => format!("#{number}"),
                };
                result.push_str(&format!(" {dll}!{symbol}"));
            }
            result
        }
    }
}

/// Map and run a trusted executable with already loaded runtime DLLs.
///
/// # Safety
/// `bytes` must contain trusted executable code. Calling it and its TLS callbacks executes it
/// unsandboxed in this process. This function initializes process-wide hooks once. The caller
/// must also ensure no other thread reads the Windows command-line buffers while they are changed.
pub unsafe fn launch(
    bytes: &[u8],
    path: &Path,
    game_folder: &Path,
    tombox_folder: &Path,
    runtime: [Module; 6],
    measuring: bool,
) -> Result<i32, String> {
    let mut image = Image::parse(bytes).map_err(|error| format!("{error} (erreur 0)"))?;
    let allocation = Allocation::map(&mut image).map_err(|error| match error {
        MemoryError::Allocation(error) => format!(
            "mémoire de l'image indisponible (erreur {})",
            error.raw_os_error().unwrap_or(0)
        ),
        MemoryError::Relocation(error) => format!("{error} (erreur 0)"),
        error => error.to_string(),
    })?;
    let wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
    let context = hooks::HookContext {
        base: allocation.base() as usize,
        path: wide_path.clone(),
        runtime: runtime.map(|module| module.handle() as usize),
        measuring,
    };
    hooks::install(context).map_err(|_| "interceptions déjà installées".to_owned())?;
    resolver::bind(&mut image, &runtime, game_folder, tombox_folder, measuring)
        .map_err(resolution_error)?;
    allocation.sync(&image).map_err(|error| error.to_string())?;
    allocation
        .protect(&image)
        .map_err(|error| error.to_string())?;
    exceptions::install(allocation.base() as usize, image.bytes.len(), wide_path)
        .map_err(str::to_owned)?;
    // The handler and registered function table retain addresses into the mapping even when TLS
    // installation fails, so keep the image mapped for the rest of the process.
    let allocation = Box::leak(Box::new(allocation));
    // SAFETY: caller accepts execution of TLS callbacks and the leaked mapping stays alive.
    unsafe { registration::register(&image, allocation, runtime[4]) }
        .map_err(|error| format!("{error} (erreur 0)"))?;
    // SAFETY: caller ensures no other thread reads the process command-line buffers.
    unsafe { command_line::adopt(path) };
    // SAFETY: caller accepts execution of the trusted, initialized image.
    unsafe { allocation.call_entry(&image) }.map_err(|error| error.to_string())
}
