//! The Windows D3D12 loader reads both exports from the process executable.
//!
//! `D3D12SDKVersion` has to be the exact Agility SDK version of the `D3D12\D3D12Core.dll` beside
//! `victory.exe`; any other value makes `D3D12CreateDevice` fail with 0x887E0003 ("the D3D12 SDK
//! version configuration of the host exe is invalid") instead of falling back to Windows' own
//! runtime. Measured on Windows 10 22H2 (19045) with Microsoft.Direct3D.D3D12 1.620.2: 620 loads
//! it; 618, 619 and 621 fail; 620 with no `D3D12` folder fails too. So the version is not fixed
//! at build time: [`check_runtime`] sets it from the file that is there, or to Windows' own
//! runtime's where there is none, before Direct3D 12 is first used (`build.rs` delay-loads
//! `d3d12.dll`, which reads the export when it is loaded).
//!
//! Ported from trustno1-coder/VIctory-NVIDIA (`apps/victory/src/main.rs`, GPL-3.0 like this
//! copy), where Windows 11 machines with an older own runtime, or another package version in the
//! folder, were refused Direct3D 12 the same way.

use std::ffi::c_char;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use anyhow::{Context, Result};

/// The newest Agility SDK version this program is tested with (Microsoft.Direct3D.D3D12 1.620.2).
const TESTED: u32 = 620;

/// Set by [`check_runtime`] before `d3d12.dll` is loaded. (An atomic: the same bytes as a `u32`,
/// in memory that can be written.)
#[unsafe(no_mangle)]
static D3D12SDKVersion: AtomicU32 = AtomicU32::new(TESTED);

#[repr(transparent)]
struct SdkPath(*const c_char);

// SAFETY: the exported pointer refers to an immutable static C string.
unsafe impl Sync for SdkPath {}

#[unsafe(no_mangle)]
static D3D12SDKPath: SdkPath = SdkPath(c".\\D3D12\\".as_ptr());

/// The Agility SDK version of a `D3D12Core.dll` from Microsoft's package: its file version is
/// `1.<sdk>.<patch>.0` (1.620.2.0 is 620), read from the file's VS_FIXEDFILEINFO.
fn agility_version(dll: &[u8]) -> Option<u32> {
    let signature = 0xFEEF_04BD_u32.to_le_bytes();
    let at = dll.windows(4).position(|w| w == signature)?;
    let most = u32::from_le_bytes(dll.get(at + 8..at + 12)?.try_into().ok()?);
    let (major, minor) = (most >> 16, most & 0xffff);
    (major == 1 && minor >= 600).then_some(minor)
}

/// The `D3D12SDKVersion` a `D3D12Core.dll` exports: Windows' own in System32 says which version
/// it is, as Microsoft's do.
fn exported_sdk_version(dll: &[u8]) -> Option<u32> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(dll.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(dll.get(at..at + 4)?.try_into().ok()?));
    let pe = u32_at(0x3c)? as usize;
    (dll.get(pe..pe + 4)? == b"PE\0\0").then_some(())?;
    let sections = usize::from(u16_at(pe + 6)?);
    let optional = pe + 24;
    let table = optional + usize::from(u16_at(pe + 20)?);
    // PE32+ only: the export directory is the first data directory, at 112.
    (u16_at(optional)? == 0x20b).then_some(())?;
    let offset = |rva: u32| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let (size, va, raw_size, raw) = (
                u32_at(s + 8)?,
                u32_at(s + 12)?,
                u32_at(s + 16)?,
                u32_at(s + 20)?,
            );
            (va <= rva && rva < va + size.max(raw_size)).then(|| (rva - va + raw) as usize)
        })
    };
    let exports = offset(u32_at(optional + 112)?)?;
    // NumberOfNames, AddressOfFunctions, AddressOfNames, AddressOfNameOrdinals
    let count = u32_at(exports + 24)? as usize;
    let functions_at = offset(u32_at(exports + 28)?)?;
    let names_at = offset(u32_at(exports + 32)?)?;
    let ordinals_at = offset(u32_at(exports + 36)?)?;
    (0..count).find_map(|i| {
        let name = offset(u32_at(names_at + 4 * i)?)?;
        if dll.get(name..name + 16)? != b"D3D12SDKVersion\0" {
            return None;
        }
        let ordinal = usize::from(u16_at(ordinals_at + 2 * i)?);
        u32_at(offset(u32_at(functions_at + 4 * ordinal)?)?)
    })
}

/// The version asked for where no usable `D3D12\D3D12Core.dll` is beside this program: Windows'
/// own runtime's, so Windows keeps it, at most [`TESTED`]. Windows 10 22H2's own is 2: asking it
/// for 3 or 4 there without the folder fails like a mismatch, 2 and 0 give its own runtime. So 0
/// where System32's file is missing or exports none.
fn os_runtime() -> u32 {
    let system = std::env::var_os("SystemRoot")
        .map_or_else(|| r"C:\Windows".into(), std::path::PathBuf::from);
    std::fs::read(system.join("System32").join("D3D12Core.dll"))
        .ok()
        .and_then(|dll| exported_sdk_version(&dll))
        .map_or(0, |v| v.min(TESTED))
}

/// Asks Windows for the Agility SDK version of the `D3D12\D3D12Core.dll` beside this program, or
/// for its own runtime where there is none. First thing in `main`: Windows reads
/// `D3D12SDKVersion` when Direct3D 12 is first used. A `VICTORY_AGILITY_SDK=1` build stops
/// instead when the file is missing.
pub fn check_runtime() -> Result<()> {
    let executable = std::env::current_exe().context("locate victory.exe for Agility SDK")?;
    let runtime = executable.with_file_name("D3D12").join("D3D12Core.dll");
    #[cfg(agility_sdk_required)]
    anyhow::ensure!(
        runtime.is_file(),
        "Agility SDK build requires {} from Microsoft.Direct3D.D3D12 1.620.2 (x64). See Getting started in README.md.",
        runtime.display()
    );
    let version = match std::fs::read(&runtime) {
        Ok(bytes) => match agility_version(&bytes) {
            Some(v) => {
                tracing::info!(path = %runtime.display(), version = v, "Agility SDK requested");
                v
            }
            None => {
                let own = os_runtime();
                tracing::warn!(
                    path = %runtime.display(),
                    own,
                    "not a D3D12 Agility SDK runtime Windows can use: Windows' own Direct3D 12 is used"
                );
                own
            }
        },
        Err(_) => {
            let own = os_runtime();
            tracing::info!(
                missing = %runtime.display(),
                own,
                "no Agility SDK runtime beside victory.exe: Windows' own Direct3D 12 is used (Windows 10 refuses the title's root signatures without it, see README.md)"
            );
            own
        }
    };
    D3D12SDKVersion.store(version, Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    /// A file holding only a VS_FIXEDFILEINFO with file version `major.minor.0.0`.
    fn core_bytes(major: u32, minor: u32) -> Vec<u8> {
        let mut v = vec![0u8; 16];
        v.extend_from_slice(&0xFEEF_04BD_u32.to_le_bytes());
        v.extend_from_slice(&0x0001_0000_u32.to_le_bytes());
        v.extend_from_slice(&((major << 16) | minor).to_le_bytes());
        v.extend_from_slice(&[0u8; 40]);
        v
    }

    #[test]
    fn agility_version_is_the_middle_number_of_the_file_version() {
        assert_eq!(super::agility_version(&core_bytes(1, 620)), Some(620));
        assert_eq!(super::agility_version(&core_bytes(1, 722)), Some(722));
        // Windows' own D3D12Core.dll (10.0.x) and files without the structure are not one.
        assert_eq!(super::agility_version(&core_bytes(10, 0)), None);
        assert_eq!(super::agility_version(&[0u8; 64]), None);
        assert_eq!(super::agility_version(&core_bytes(1, 620)[..24]), None);
    }

    #[test]
    fn windows_own_runtime_tells_its_version() {
        let system = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap();
        if let Ok(dll) = std::fs::read(system.join("System32").join("D3D12Core.dll")) {
            let v = super::exported_sdk_version(&dll)
                .expect("System32's D3D12Core.dll exports D3D12SDKVersion");
            // 2 on Windows 10 22H2 (19045).
            assert!(v >= 1, "{v}");
            assert_eq!(super::os_runtime(), v.min(super::TESTED));
        }
        assert_eq!(super::exported_sdk_version(&[0u8; 64]), None);
        assert_eq!(super::exported_sdk_version(b"MZ"), None);
    }
}
