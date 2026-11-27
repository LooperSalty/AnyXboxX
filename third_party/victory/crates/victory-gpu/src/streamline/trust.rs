use super::{Path, wide};
use windows_sys::Win32::Security::{Cryptography::*, WinTrust::*};

/// The Authenticode signature of `path` (NVIDIA signs its DLLs), checked
/// without going online.
pub(super) fn verify_signature(path: &Path, locked: &std::fs::File) -> Result<(), String> {
    use windows_sys::Win32::Security::WinTrust::*;
    let file = wide(path);
    let mut info = WINTRUST_FILE_INFO {
        cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: file.as_ptr(),
        hFile: std::os::windows::io::AsRawHandle::as_raw_handle(locked),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: filled structures that live through both calls.
    let code = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    let result = if code == 0 {
        signer_is_nvidia(&data)
    } else {
        Err(format!("{} is not validly signed ({:#x})", path.display(), code as u32))
    };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: as above; closes the state the first call opened.
    unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    result
}

fn signer_is_nvidia(data: &WINTRUST_DATA) -> Result<(), String> {
    // SAFETY: WinVerifyTrust accepted this state. Read its primary signer's
    // leaf certificate before WTD_STATEACTION_CLOSE releases the chain.
    let organization = unsafe {
        let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
        let signer = provider.as_mut().and_then(|p| WTHelperGetProvSignerFromChain(p, 0, 0, 0).as_mut());
        let cert = signer.and_then(|s| WTHelperGetProvCertFromChain(s, 0).as_ref())
            .ok_or("Authenticode signer certificate unavailable")?;
        let mut name = [0u16; 256];
        let count = CertGetNameStringW(cert.pCert, CERT_NAME_ATTR_TYPE, 0, szOID_ORGANIZATION_NAME.cast(),
            name.as_mut_ptr(), name.len() as u32);
        String::from_utf16_lossy(&name[..count.saturating_sub(1) as usize])
    };
    if organization == "NVIDIA Corporation" { Ok(()) }
    else { Err(format!("expected Authenticode signer NVIDIA Corporation, got {organization:?}")) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_unsigned_dll_before_loading() {
        let dir = tempfile::tempdir().unwrap();
        let dll = dir.path().join("sl.interposer.dll");
        std::fs::write(&dll, b"unsigned test input").unwrap();
        assert!(super::super::load(&dll).err().unwrap().contains("not validly signed"));
    }
}
