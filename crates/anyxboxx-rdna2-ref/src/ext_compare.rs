//! SDWA form of the VOPC compares: port of the VOPC branch of `_exec_sdwa` in `emu/rdna2_exec_ext.py`.
//!
//! The compares of the Python `VECTOR_COMPARE` table read their two
//! sources through the SDWA selections of the extension word: source 0 is the register of bits 0-7
//! (an SGPR when bit 23 is set), source 1 the VSRC1 field (an SGPR when bit 31 is set), each with its
//! byte or word selection, sign extension (integer compares) or abs and neg (float compares). The mask
//! goes to EXEC for v_cmpx, else to the SGPR of bits 8-14 when bit 15 (SD) is set, else to VCC. The
//! clamp and output modifier fields of VOP1 and VOP2 do not exist in this form.
//!
//! The 64-bit and class compares do not reach this module: the Python runs them as plain compares
//! whose source 0 is the extension code, which no lane can read. The DPP16 and DPP8 forms of VOPC are
//! refused, as in the Python.

use crate::compare::compare32;
use crate::error::Error;
use crate::ext::{extract_selection, float_flags, float_modifiers};
use crate::wave::{EXEC_LO, VCC_LO, Wave};

const DESTINATION_SHIFT: u32 = 8;
const DESTINATION_MASK: u32 = 0x7F;
const SD_BIT: u32 = 15;

/// One SDWA source: its code, whether it is scalar, its selection, sign extension, neg and abs.
#[derive(Debug, Clone, Copy)]
struct Source {
    code: u32,
    scalar: bool,
    selection: u32,
    sext: bool,
    negated: bool,
    absolute: bool,
}

impl Source {
    /// The fields at `shift` (selection, then sext, neg, abs) and the scalar bit at `scalar_bit`.
    fn new(code: u32, ext: u32, shift: u32, scalar_bit: u32) -> Self {
        let bit = |offset: u32| (ext >> (shift + offset)) & 1 == 1;
        Self {
            code,
            scalar: (ext >> scalar_bit) & 1 == 1,
            selection: (ext >> shift) & 7,
            sext: bit(3),
            negated: bit(4),
            absolute: bit(5),
        }
    }

    fn read(&self, wave: &Wave, lane: usize, float: bool) -> Result<u32, Error> {
        let raw = if self.scalar {
            wave.read_scalar(self.code, None, 1)? as u32
        } else {
            wave.v[self.code as usize][lane]
        };
        let value = extract_selection(raw, self.selection, self.sext && !float);
        Ok(if float {
            float_modifiers(value, self.absolute, self.negated)
        } else {
            value
        })
    }
}

/// True when `name` is a `VECTOR_COMPARE` operation supported in SDWA.
pub fn supports(name: &str) -> bool {
    compare32(name).is_some()
}

/// The SGPR the mask goes to: EXEC for v_cmpx, the SD destination when bit 15 is set, else VCC.
fn destination(writes_exec: bool, ext: u32) -> u32 {
    if writes_exec {
        EXEC_LO
    } else if (ext >> SD_BIT) & 1 == 1 {
        (ext >> DESTINATION_SHIFT) & DESTINATION_MASK
    } else {
        VCC_LO
    }
}

/// Runs the SDWA compare `name`; `vsrc1` is the VSRC1 field, `ext` the extension word.
pub fn execute(wave: &mut Wave, name: &str, vsrc1: u32, ext: u32) -> Result<(), Error> {
    let (writes_exec, test) = compare32(name)
        .ok_or_else(|| Error::Untranslated(format!("instruction non traduite : {name} (SDWA)")))?;
    let (float, _) = float_flags(name);
    let s0 = Source::new(ext & 0xFF, ext, 16, 23);
    let s1 = Source::new(vsrc1, ext, 24, 31);
    let mut mask = 0u64;
    for lane in wave.active()? {
        let a = s0.read(wave, lane, float)?;
        let b = s1.read(wave, lane, float)?;
        mask |= u64::from(test(a, b)) << lane;
    }
    wave.write_mask(destination(writes_exec, ext), mask)
}

/// The refusal of a DPP16 or DPP8 compare, worded as the Python's.
pub fn refuse_dpp(name: &str, dpp16: bool) -> Error {
    let form = if dpp16 { "DPP16" } else { "DPP8" };
    Error::Untranslated(format!("VOPC non pris en charge en {form} : {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_goes_to_exec_then_sd_then_vcc() {
        assert_eq!(destination(true, 1 << SD_BIT | 4 << 8), EXEC_LO);
        assert_eq!(destination(false, 1 << SD_BIT | 4 << 8), 4);
        assert_eq!(destination(false, 4 << 8), VCC_LO);
    }

    #[test]
    fn source_fields_follow_the_sdwa_layout() {
        // source 1: WORD_1, sext, neg, abs, scalar
        let ext = 5 << 24 | 1 << 27 | 1 << 28 | 1 << 29 | 1 << 31;
        let s1 = Source::new(3, ext, 24, 31);
        assert_eq!(
            (s1.selection, s1.sext, s1.negated, s1.absolute, s1.scalar),
            (5, true, true, true, true)
        );
    }

    #[test]
    fn a_float_compare_ignores_sext_and_applies_abs() {
        let mut wave = Wave::new(32).unwrap();
        wave.v[1][0] = 0x0000_8000;
        let source = Source::new(1, 4 << 16 | 1 << 19 | 1 << 21, 16, 23);
        assert_eq!(source.read(&wave, 0, true).unwrap(), 0x8000);
        assert_eq!(source.read(&wave, 0, false).unwrap(), 0xFFFF_8000);
    }

    #[test]
    fn integer16_and_32_bit_compares_are_supported() {
        assert!(supports("v_cmp_lt_f32"));
        assert!(supports("v_cmpx_ge_i32"));
        assert!(supports("v_cmp_lt_i16"));
        assert!(!supports("v_cmp_lt_f64"));
        assert!(!supports("v_cmp_class_f32"));
    }
}
