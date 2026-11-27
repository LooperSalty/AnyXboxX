//! Enumeration of the one locally installed game package.
use crate::object::{E_INVALIDARG, HResult, S_OK};
use std::ffi::{CString, c_char, c_void};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Version {
    major: u16,
    minor: u16,
    build: u16,
    revision: u16,
}
#[repr(C)]
pub(crate) struct Details {
    identifier: *const c_char,
    version: Version,
    kind: u32,
    display_name: *const c_char,
    description: *const c_char,
    publisher: *const c_char,
    store_id: *const c_char,
    installing: bool,
    index: u32,
    count: u32,
    age_restricted: bool,
    title_id: *const c_char,
}
type Callback = unsafe extern "system" fn(*mut c_void, *const Details) -> bool;
fn text(bytes: &[u8]) -> CString {
    // C++ treats the first NUL as the end of a config field.
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    CString::new(&bytes[..end]).expect("the prefix has no NUL")
}

fn parse_version(version: &str) -> Version {
    let mut parts = [0u16; 4];
    for (out, part) in parts.iter_mut().zip(version.trim().split('.')) {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(value) = digits.parse::<u16>() else {
            break;
        };
        *out = value;
        if digits.len() != part.len() {
            break;
        }
    }
    Version {
        major: parts[0],
        minor: parts[1],
        build: parts[2],
        revision: parts[3],
    }
}

pub unsafe extern "system" fn enumerate(
    _this: *mut c_void,
    kind: u32,
    _filter: u32,
    context: *mut c_void,
    callback: Option<Callback>,
) -> HResult {
    let Some(callback) = callback else {
        return E_INVALIDARG;
    };
    if kind != 0 {
        return S_OK;
    }
    let name = crate::game_config::package_name();
    let name = if name.is_empty() {
        b"AnyXboxX.Game".as_slice()
    } else {
        name.as_bytes()
    };
    let identifier = text(&name[..name.len().min(32)]);
    let (publisher, version, display_name, store_id) = crate::game_config::package_details();
    let version = parse_version(&version);
    let display_name = text(display_name.as_bytes());
    let publisher = text(publisher.as_bytes());
    let store_id = text(store_id.as_bytes());
    let title_id = text(format!("{:08X}", crate::game_config::title_id()).as_bytes());
    let details = Details {
        identifier: identifier.as_ptr(),
        version,
        kind: 0,
        display_name: display_name.as_ptr(),
        description: c"".as_ptr(),
        publisher: publisher.as_ptr(),
        store_id: store_id.as_ptr(),
        installing: false,
        index: 0,
        count: 1,
        age_restricted: false,
        title_id: title_id.as_ptr(),
    };
    // SAFETY: the title supplied the callback and all details remain alive until it returns.
    unsafe { callback(context, &details) };
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ver(major: u16, minor: u16, build: u16, revision: u16) -> Version {
        Version {
            major,
            minor,
            build,
            revision,
        }
    }

    #[test]
    fn version_parsing_trims_whitespace_and_rejects_u16_overflow() {
        assert_eq!(parse_version("1.2.3.4"), ver(1, 2, 3, 4));
        assert_eq!(parse_version("  10.0.26100.1 \n"), ver(10, 0, 26100, 1));
        assert_eq!(parse_version("2.5-beta.9"), ver(2, 5, 0, 0));
        assert_eq!(parse_version("1.70000.3.4"), ver(1, 0, 0, 0));
        assert_eq!(parse_version(""), ver(0, 0, 0, 0));
    }

    #[test]
    fn enumerate_refuses_null_callback_and_skips_non_zero_kind() {
        unsafe extern "system" fn fail_if_called(_: *mut c_void, _: *const Details) -> bool {
            panic!("callback must not be invoked for non-zero kind");
        }
        // SAFETY: null pointers are part of the FFI validation contract.
        assert_eq!(
            unsafe { enumerate(std::ptr::null_mut(), 0, 0, std::ptr::null_mut(), None) },
            E_INVALIDARG
        );
        // SAFETY: valid callback pointer; kind != 0 returns before invoking it.
        assert_eq!(
            unsafe {
                enumerate(
                    std::ptr::null_mut(),
                    1,
                    0,
                    std::ptr::null_mut(),
                    Some(fail_if_called),
                )
            },
            S_OK
        );
    }
}
