//! Import and runtime DLL routing rules of the native launcher.

pub const RUNTIME_DLLS: [&str; 6] = [
    "xgameruntime.dll",
    "xmem.dll",
    "d3d12_xs.dll",
    "kernelx.dll",
    "anyxboxx_tls.dll",
    "XCRDAPI.dll",
];

const REGISTRY: [&[u8]; 4] = [
    b"RegOpenKeyW",
    b"RegOpenKeyExW",
    b"RegQueryValueExW",
    b"RegCloseKey",
];
const DXGI: [&[u8]; 3] = [
    b"CreateDXGIFactory",
    b"CreateDXGIFactory1",
    b"CreateDXGIFactory2",
];
const SHIMS: [&[u8]; 9] = [
    b"LoadLibraryExW",
    b"LoadLibraryW",
    b"LoadLibraryExA",
    b"LoadLibraryA",
    b"GetProcAddress",
    b"GetModuleHandleW",
    b"GetModuleHandleA",
    b"GetModuleFileNameW",
    b"GetModuleFileNameA",
];

fn basename(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase()
}

/// The runtime array index used for a DLL name, if intercepted by the launcher.
pub fn runtime_for(path: &str) -> Option<usize> {
    let mut name = basename(path);
    if !name.contains('.') {
        name.push_str(".dll");
    }
    if name.starts_with("xgameruntime") {
        return Some(0);
    }
    (1..4).find(|&index| name == RUNTIME_DLLS[index])
}

/// A replacement is an export of xgameruntime, or one of the launcher's own shims.
#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    RuntimeExport(Vec<u8>),
    Shim,
}

pub fn redirect(dll: &str, name: &[u8], game_dll: bool, measuring: bool) -> Option<Redirect> {
    let module = basename(dll);
    if ((module == "advapi32.dll" || module.starts_with("api-ms-win-core-registry-"))
        && REGISTRY.contains(&name))
        || (measuring && module == "dxgi.dll" && DXGI.contains(&name))
    {
        let mut export = b"AnyXboxX_".to_vec();
        export.extend_from_slice(name);
        return Some(Redirect::RuntimeExport(export));
    }
    let kernel = matches!(
        module.as_str(),
        "kernel32.dll" | "kernelbase.dll" | "kernelx.dll"
    ) || module.starts_with("api-ms-win-core-");
    if kernel && SHIMS.contains(&name) && (!game_dll || name == b"GetProcAddress") {
        return Some(Redirect::Shim);
    }
    None
}

/// TomBox is preferred for these runtime DLLs, subject to an explicit D3D12 override.
pub fn prefer_tombox(index: usize) -> bool {
    matches!(index, 1 | 2 | 5)
}
