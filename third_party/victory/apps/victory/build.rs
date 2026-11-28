//! Exports `D3D12SDKVersion` and `D3D12SDKPath` (defined in `src/agility.rs`) from `victory.exe`.
//!
//! The D3D12 loader looks for these two exports in the executable: when they are there and a
//! `D3D12\D3D12Core.dll` of the D3D12 Agility SDK sits beside the executable, that runtime is
//! used instead of the one Windows ships. Windows 10's own runtime refuses every root signature
//! the title asks for, so the exports are in every x64 Windows MSVC build (the launcher's too).
//! `src/agility.rs` sets the version at startup: the folder's, or Windows' own without one.
//!
//! The loader reads `D3D12SDKVersion` when `d3d12.dll` is loaded. Imported the usual way it would
//! be loaded before `main`, so before that version is set: it is delay-loaded instead.
//!
//! `VICTORY_AGILITY_SDK=0` leaves all of this out. `VICTORY_AGILITY_SDK=1` also makes `victory`
//! stop at startup when `D3D12\D3D12Core.dll` is missing.
fn main() {
    println!("cargo:rerun-if-env-changed=VICTORY_AGILITY_SDK");
    println!("cargo:rustc-check-cfg=cfg(agility_sdk)");
    println!("cargo:rustc-check-cfg=cfg(agility_sdk_required)");
    let x64_msvc = std::env::var("TARGET").as_deref() == Ok("x86_64-pc-windows-msvc");
    match std::env::var("VICTORY_AGILITY_SDK").as_deref() {
        Ok("0") => return,
        Ok("1") => {
            assert!(
                x64_msvc,
                "VICTORY_AGILITY_SDK=1 requires the x86_64 Windows MSVC target"
            );
            println!("cargo:rustc-cfg=agility_sdk_required");
        }
        _ if !x64_msvc => return,
        _ => {}
    }
    println!("cargo:rustc-cfg=agility_sdk");
    println!("cargo:rustc-link-arg-bin=victory=/EXPORT:D3D12SDKVersion,DATA");
    println!("cargo:rustc-link-arg-bin=victory=/EXPORT:D3D12SDKPath,DATA");
    println!("cargo:rustc-link-arg-bin=victory=/DELAYLOAD:d3d12.dll");
    println!("cargo:rustc-link-arg-bin=victory=delayimp.lib");
}
