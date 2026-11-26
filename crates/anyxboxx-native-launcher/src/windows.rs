use anyxboxx_native_launcher::{execution, libraries, manifest_executable, resolver};
use std::ffi::OsString;
use std::fs::File;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr;

const MAX_MANIFEST: u64 = 1024 * 1024;
const MAX_PATH: usize = 260;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFullPathNameW(name: *const u16, size: u32, buffer: *mut u16, part: *mut *mut u16) -> u32;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
}

fn full_path(path: &Path) -> Option<PathBuf> {
    let input: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut output = [0u16; MAX_PATH];
    // SAFETY: the input is terminated and output has MAX_PATH UTF-16 units.
    let len = unsafe {
        GetFullPathNameW(
            input.as_ptr(),
            MAX_PATH as u32,
            output.as_mut_ptr(),
            ptr::null_mut(),
        )
    } as usize;
    if len == 0 || len >= output.len() {
        return None;
    }
    use std::os::windows::ffi::OsStringExt;
    Some(PathBuf::from(OsString::from_wide(&output[..len])))
}

fn target(path: &Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return Some(path.to_owned());
    }
    let file = File::open(path.join("MicrosoftGame.config")).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST).read_to_end(&mut bytes).ok()?;
    let name = resolver::widen(&manifest_executable(&bytes)?)?;
    Some(path.join(name))
}

fn set_env(name: &str, value: &std::ffi::OsStr) {
    let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = value.encode_wide().chain([0]).collect();
    // SAFETY: both arguments are NUL-terminated UTF-16 buffers.
    unsafe {
        SetEnvironmentVariableW(name.as_ptr(), value.as_ptr());
    }
}

fn fail(message: &str, code: u32) -> i32 {
    eprintln!("[anyxboxx] {message} (erreur {code})");
    1
}

pub fn run() -> i32 {
    let mut args = std::env::args_os();
    let _program = args.next();
    let Some(input) = args.next() else {
        eprintln!("Usage : anyxboxx.exe <jeu.exe | dossier du paquet> [--fps]");
        return 1;
    };
    if args.any(|arg| arg == "--fps") {
        set_env("ANYXBOXX_FPS", std::ffi::OsStr::new("1"));
    }
    let Some(path) = target(Path::new(&input)).and_then(|path| full_path(&path)) else {
        return fail("jeu introuvable", 0);
    };
    if !path.exists() {
        eprintln!("[anyxboxx] exécutable du jeu absent : {}", path.display());
        eprintln!(
            "[anyxboxx] le paquet ne contient que les données et les DLL ; le code du jeu est dans cet exécutable."
        );
        return 1;
    }
    let Some(folder) = path.parent() else {
        return fail("jeu introuvable", 0);
    };
    let measuring = std::env::var_os("ANYXBOXX_FPS").is_some_and(|v| !v.is_empty())
        || std::env::var_os("ANYXBOXX_FPS_CSV").is_some_and(|v| !v.is_empty());
    let Ok(self_path) = std::env::current_exe() else {
        return fail("lanceur introuvable", 0);
    };
    let Some(runtime_dir) = self_path.parent() else {
        return fail("lanceur introuvable", 0);
    };
    let tombox = std::env::var_os("ANYXBOXX_TOMBOX_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| runtime_dir.join("..").join("tombox"), PathBuf::from);
    let alternative = std::env::var_os("ANYXBOXX_D3D12_XS")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let paths =
        libraries::runtime_paths(runtime_dir, &tombox, alternative.as_deref(), Path::exists);
    let runtime = match libraries::load_runtime(&paths) {
        Ok(runtime) => runtime,
        Err((path, error)) => {
            return fail(
                &format!("runtime introuvable : {}", path.display()),
                error.raw_os_error().unwrap_or(0) as u32,
            );
        }
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return fail("lecture du jeu impossible", 0),
    };
    set_env("ANYXBOXX_GAME_DIR", folder.as_os_str());
    if std::env::set_current_dir(folder).is_err() {
        return fail("dossier du jeu inaccessible", 0);
    }
    // SAFETY: native mode is explicitly requested and runs the user's executable unsandboxed.
    match unsafe { execution::launch(&bytes, &path, folder, &tombox, runtime, measuring) } {
        Ok(code) => code,
        Err(error) => {
            eprintln!("[anyxboxx] {error}");
            1
        }
    }
}
