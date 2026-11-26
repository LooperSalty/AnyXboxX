//! Enumeration of the one locally installed game package.
use crate::object::{E_INVALIDARG, HResult, S_OK};
use std::ffi::{CString, c_char, c_void};

#[repr(C)]
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
    let mut parts = [0u16; 4];
    for (out, part) in parts.iter_mut().zip(version.split('.')) {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(value) = digits.parse::<u32>() else {
            break;
        };
        *out = value as u16;
        if digits.len() != part.len() {
            break;
        }
    }
    let display_name = text(display_name.as_bytes());
    let publisher = text(publisher.as_bytes());
    let store_id = text(store_id.as_bytes());
    let title_id = text(format!("{:08X}", crate::game_config::title_id()).as_bytes());
    let details = Details {
        identifier: identifier.as_ptr(),
        version: Version {
            major: parts[0],
            minor: parts[1],
            build: parts[2],
            revision: parts[3],
        },
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
