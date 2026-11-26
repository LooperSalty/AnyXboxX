//! Connectivity hint of the PC (XNetworking), from Windows: Network List Manager for the level, the
//! default route's interface for its IANA type. Port of the networking half of `platform_interface.cpp`.
//! Other XNetworking methods (slots 3 to 14, 18 to 20) stay E_NOTIMPL.

use std::ffi::c_void;

use windows::Win32::NetworkManagement::IpHelper::{GetBestInterface, GetIfEntry, MIB_IFROW};
use windows::Win32::Networking::NetworkListManager::{
    INetworkListManager, NLM_CONNECTIVITY, NLM_CONNECTIVITY_IPV4_INTERNET,
    NLM_CONNECTIVITY_IPV4_LOCALNETWORK, NLM_CONNECTIVITY_IPV4_SUBNET,
    NLM_CONNECTIVITY_IPV6_INTERNET, NLM_CONNECTIVITY_IPV6_LOCALNETWORK,
    NLM_CONNECTIVITY_IPV6_SUBNET, NetworkListManager,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

use crate::events;
use crate::object::{E_INVALIDARG, HResult, S_OK};

/// XNetworkingConnectivityLevelHint values.
const LEVEL_UNKNOWN: u32 = 0;
const LEVEL_NONE: u32 = 1;
const LEVEL_LOCAL: u32 = 2;
const LEVEL_INTERNET: u32 = 3;

/// XNetworkingConnectivityHint (XNetworking.h): the structure the title passes by pointer.
#[repr(C)]
#[derive(Default)]
pub struct ConnectivityHint {
    pub connectivity_level: u32,
    /// 0: unknown (the cost comes from the WinRT API, not queried).
    pub connectivity_cost: u32,
    pub iana_interface_type: u32,
    pub network_initialized: bool,
    pub approaching_data_limit: bool,
    pub over_data_limit: bool,
    pub roaming: bool,
}

/// The level of connectivity Network List Manager reports; `LEVEL_UNKNOWN` if it cannot be asked.
fn level_from_windows() -> u32 {
    // SAFETY: the calling convention of COM initialisation; the result is paired below.
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let level = match unsafe {
        CoCreateInstance::<_, INetworkListManager>(&NetworkListManager, None, CLSCTX_ALL)
    } {
        Ok(manager) => match unsafe { manager.GetConnectivity() } {
            Ok(connectivity) => classify(connectivity),
            Err(_) => LEVEL_UNKNOWN,
        },
        Err(_) => LEVEL_UNKNOWN,
    };
    if init.is_ok() {
        // SAFETY: paired with the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    level
}

/// Maps Network List Manager's connectivity flags to the title's levels.
fn classify(connectivity: NLM_CONNECTIVITY) -> u32 {
    let internet = NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0;
    let local = NLM_CONNECTIVITY_IPV4_LOCALNETWORK.0
        | NLM_CONNECTIVITY_IPV6_LOCALNETWORK.0
        | NLM_CONNECTIVITY_IPV4_SUBNET.0
        | NLM_CONNECTIVITY_IPV6_SUBNET.0;
    if connectivity.0 & internet != 0 {
        LEVEL_INTERNET
    } else if connectivity.0 & local != 0 {
        LEVEL_LOCAL
    } else {
        LEVEL_NONE
    }
}

/// The IANA type (ifType) of the interface of the default route; 0 when there is none.
fn default_route_interface_type() -> u32 {
    let mut index = 0u32;
    // SAFETY: the destination 0 is the default route; `index` is writable.
    if unsafe { GetBestInterface(0, &mut index) } != 0 {
        return 0;
    }
    let mut row = MIB_IFROW {
        dwIndex: index,
        ..Default::default()
    };
    // SAFETY: the row is writable and its index is set, as GetIfEntry requires.
    if unsafe { GetIfEntry(&mut row) } == 0 {
        row.dwType
    } else {
        0
    }
}

/// XNetworkingGetConnectivityHint (slot 15).
///
/// # Safety
/// `hint` is null or points at one writable `ConnectivityHint`.
pub unsafe extern "system" fn get_connectivity_hint(
    _this: *mut c_void,
    hint: *mut ConnectivityHint,
) -> HResult {
    let Some(hint) = (unsafe { hint.as_mut() }) else {
        return E_INVALIDARG;
    };
    let mut level = level_from_windows();
    if level == LEVEL_UNKNOWN {
        level = LEVEL_NONE;
    }
    *hint = ConnectivityHint {
        connectivity_level: level,
        iana_interface_type: default_route_interface_type(),
        network_initialized: true,
        ..Default::default()
    };
    S_OK
}

/// The method table of the networking object: slots 15 to 17.
pub fn methods() -> Vec<(usize, *const c_void)> {
    vec![
        (15, get_connectivity_hint as *const c_void),
        (16, events::subscribe as *const c_void),
        (17, events::unsubscribe as *const c_void),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_list_flags_give_the_title_levels() {
        assert_eq!(classify(NLM_CONNECTIVITY(0)), LEVEL_NONE);
        assert_eq!(classify(NLM_CONNECTIVITY_IPV4_LOCALNETWORK), LEVEL_LOCAL);
        assert_eq!(classify(NLM_CONNECTIVITY_IPV6_INTERNET), LEVEL_INTERNET);
    }

    #[test]
    fn a_null_hint_is_refused() {
        // SAFETY: a null pointer is part of the contract.
        assert_eq!(
            unsafe { get_connectivity_hint(std::ptr::null_mut(), std::ptr::null_mut()) },
            E_INVALIDARG
        );
    }
}
