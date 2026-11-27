//! HDR tone-mapping luminances of the display (XDisplayHdrModeInfo), made usable.
//!
//! DXGI reads them from the display's static HDR metadata (EDID). A display that announces none
//! reports 0, which CTA-861.3 reads as "undefined", and a tone mapper that caps or divides by the
//! peak then has nothing to work with. The fallbacks are those of DXVK (`NormalizeDisplayMetadata`,
//! zlib), which match what Windows' DXGI reports for panels with a faulty EDID. Port of
//! `runtime/xgameruntime/hdr_luminance.h`.

/// The three values of XDisplayHdrModeInfo, in nits.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrModeInfo {
    pub min_tone_map_luminance: f32,
    pub max_tone_map_luminance: f32,
    pub max_full_frame_tone_map_luminance: f32,
}

const FALLBACK_MIN: f32 = 0.01;
const FALLBACK_MAX: f32 = 1499.0;
const FALLBACK_MAX_FULL_FRAME: f32 = 799.0;

/// Keeps the display's values when they are coherent (`0 <= min < max`); otherwise the whole block
/// is unusable and the three fallbacks replace it. A full-frame value that is absent or above the
/// peak becomes `min(fallback, max)`.
pub fn normalize(reported: HdrModeInfo) -> HdrModeInfo {
    let min = reported.min_tone_map_luminance;
    let max = reported.max_tone_map_luminance;
    // Non-finite values (NaN or infinity) make the block incoherent or unusable.
    if !min.is_finite() || !max.is_finite() || min < 0.0 || max <= min {
        return HdrModeInfo {
            min_tone_map_luminance: FALLBACK_MIN,
            max_tone_map_luminance: FALLBACK_MAX,
            max_full_frame_tone_map_luminance: FALLBACK_MAX_FULL_FRAME,
        };
    }
    let mut full_frame = reported.max_full_frame_tone_map_luminance;
    if !full_frame.is_finite() || full_frame <= 0.0 {
        full_frame = FALLBACK_MAX_FULL_FRAME.min(max);
    }
    HdrModeInfo {
        min_tone_map_luminance: min,
        max_tone_map_luminance: max,
        max_full_frame_tone_map_luminance: full_frame.min(max),
    }
}

/// `AnyXboxX_NormalizeHdrLuminance`: normalises `*info` in place; a null pointer is ignored.
///
/// # Safety
/// `info` is null or points at one writable `HdrModeInfo`.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn AnyXboxX_NormalizeHdrLuminance(info: *mut HdrModeInfo) {
    // SAFETY: non-null and writable, per this function's contract.
    if let Some(info) = unsafe { info.as_mut() } {
        *info = normalize(*info);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(min: f32, max: f32, full: f32) -> HdrModeInfo {
        HdrModeInfo {
            min_tone_map_luminance: min,
            max_tone_map_luminance: max,
            max_full_frame_tone_map_luminance: full,
        }
    }

    #[test]
    fn a_display_without_metadata_gets_the_fallbacks() {
        let fallback = block(0.01, 1499.0, 799.0);
        for reported in [
            block(0.0, 0.0, 0.0),
            block(0.5, 0.5, 0.0),
            block(0.05, 0.02, 300.0),
            block(-1.0, 600.0, 400.0),
            block(f32::NAN, 600.0, 400.0),
            block(0.01, f32::INFINITY, 400.0),
            block(f32::NEG_INFINITY, 600.0, 400.0),
        ] {
            assert_eq!(normalize(reported), fallback, "{reported:?}");
        }
    }

    #[test]
    fn coherent_values_are_kept_and_a_missing_full_frame_value_is_capped() {
        assert_eq!(
            normalize(block(0.01, 600.0, 400.0)),
            block(0.01, 600.0, 400.0)
        );
        assert_eq!(
            normalize(block(0.01, 600.0, 0.0)),
            block(0.01, 600.0, 600.0)
        );
        assert_eq!(
            normalize(block(0.01, 400.0, 800.0)),
            block(0.01, 400.0, 400.0)
        );
        assert_eq!(
            normalize(block(0.01, 1000.0, f32::INFINITY)),
            block(0.01, 1000.0, FALLBACK_MAX_FULL_FRAME)
        );
    }

    #[test]
    fn the_export_normalises_in_place_and_ignores_null() {
        let mut info = block(0.0, 0.0, 0.0);
        // SAFETY: `info` is a valid, writable HdrModeInfo.
        unsafe { AnyXboxX_NormalizeHdrLuminance(&mut info) };
        assert_eq!(info, block(0.01, 1499.0, 799.0));
        // SAFETY: a null pointer is part of the contract.
        unsafe { AnyXboxX_NormalizeHdrLuminance(std::ptr::null_mut()) };
    }
}
