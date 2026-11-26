//! The HDR state of the primary display (XDisplayTryEnableHdrMode), as Windows reports it through
//! DXGI (`IDXGIOutput6::GetDesc1`). On PC Windows switches HDR on, so the title gets the real state
//! of the screen, never a simulated one. Port of the display half of `platform_interface.cpp`.

use std::ffi::c_void;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_NOT_FOUND;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_OUTPUT_DESC1, IDXGIFactory1, IDXGIOutput6,
};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::core::Interface;

use crate::hdr::{HdrModeInfo, normalize};
use crate::log::trace;

/// XDisplayHdrModeResult values.
const HDR_UNKNOWN: u32 = 0;
const HDR_ENABLED: u32 = 1;
const HDR_DISABLED: u32 = 2;

/// The description of the output that holds the primary monitor, if DXGI has one.
fn primary_output_description() -> Option<DXGI_OUTPUT_DESC1> {
    // SAFETY: a point and flags as the call expects; the returned handle is only compared.
    let primary: HMONITOR =
        unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
    // SAFETY: no arguments beyond the interface id, which the crate derives.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    for adapter_index in 0u32.. {
        // SAFETY: an index; DXGI reports the end of the list with DXGI_ERROR_NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return None,
            Err(_) => return None,
        };
        for output_index in 0u32.. {
            // SAFETY: as above, for the outputs of this adapter.
            let Ok(output) = (unsafe { adapter.EnumOutputs(output_index) }) else {
                break;
            };
            let Ok(output6) = output.cast::<IDXGIOutput6>() else {
                continue;
            };
            // SAFETY: the interface is valid; the description is returned by value.
            if let Ok(description) = unsafe { output6.GetDesc1() }
                && description.Monitor == primary
            {
                return Some(description);
            }
        }
    }
    None
}

/// XDisplayTryEnableHdrMode: the primary display's state; with `info` non-null and HDR on, its
/// luminances (normalised, as the C++ does).
///
/// # Safety
/// `info` is null or points at one writable `HdrModeInfo`.
pub unsafe extern "system" fn try_enable_hdr_mode(
    _this: *mut c_void,
    preference: u32,
    info: *mut HdrModeInfo,
) -> u32 {
    let Some(description) = primary_output_description() else {
        return HDR_UNKNOWN;
    };
    let hdr = description.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
    if hdr && let Some(info) = unsafe { info.as_mut() } {
        *info = normalize(HdrModeInfo {
            min_tone_map_luminance: description.MinLuminance,
            max_tone_map_luminance: description.MaxLuminance,
            max_full_frame_tone_map_luminance: description.MaxFullFrameLuminance,
        });
    }
    trace!(
        "[runtime] XDisplayTryEnableHdrMode({preference}) : écran {} (DXGI : {:.4} / {:.1} / {:.1} nits)",
        if hdr { "HDR" } else { "SDR" },
        description.MinLuminance,
        description.MaxLuminance,
        description.MaxFullFrameLuminance
    );
    if hdr { HDR_ENABLED } else { HDR_DISABLED }
}

/// The method table of the HDR display object: slot 4 is XDisplayTryEnableHdrMode.
pub fn methods() -> Vec<(usize, *const c_void)> {
    vec![(4, try_enable_hdr_mode as *const c_void)]
}
