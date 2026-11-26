//! Packed f16 of VOP3P (`v_pk_fma/add/mul/min/max_f16`) and `v_pk_fmac_f16` (VOP2 opcode 0x3C, VOP3 0x13C),
//! ported from `emu/rdna2_exec_pkf16.py`. The mixed-precision and dot-product forms are in `pkf16_mix`.

use crate::error::Error;
use crate::half::{
    HALF_MASK, half_clamp, half_from_f64, half_max, half_min, half_to_f64, read_half, signed_half,
};
use crate::vop3::Fields;
use crate::vop3p::{
    CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT, SOURCE_MASK,
    SOURCE_SHIFTS, VDST_MASK,
};
use crate::wave::Wave;

/// Source codes of the DPP8, DPP8_FI, SDWA and DPP16 forms: refused for packed f16.
const EXTENSION_CODES: [u32; 4] = [0xE9, 0xEA, 0xF9, 0xFA];
/// First VGPR source code.
const VGPR_FIRST: u32 = 256;

/// A packed operation over the 16-bit arguments of one half: (source count, function).
type PackedOp = fn([u32; 3]) -> u32;

fn half(bits: u32) -> f64 {
    half_to_f64(bits)
}

/// The packed f16 operations of VOP3P and their source count.
fn packed_operation(name: &str) -> Option<(usize, PackedOp)> {
    Some(match name {
        "v_pk_fma_f16" => (3, |[a, b, c]| half_from_f64(half(a) * half(b) + half(c))),
        "v_pk_add_f16" => (2, |[a, b, _]| half_from_f64(half(a) + half(b))),
        "v_pk_mul_f16" => (2, |[a, b, _]| half_from_f64(half(a) * half(b))),
        "v_pk_min_f16" => (2, |[a, b, _]| half_min(a, b)),
        "v_pk_max_f16" => (2, |[a, b, _]| half_max(a, b)),
        _ => return None,
    })
}

fn bit(word: u64, shift: u32) -> bool {
    (word >> shift) & 1 == 1
}

/// Runs a packed f16 VOP3P instruction for every active lane. `Ok(false)` when `name` is not one of them.
pub fn execute_packed(
    wave: &mut Wave,
    name: &str,
    word: u64,
    literal: Option<u32>,
) -> Result<bool, Error> {
    let Some((count, operation)) = packed_operation(name) else {
        return Ok(false);
    };
    let clamp = bit(word, CLAMP_BIT);
    let destination = (word & VDST_MASK) as usize;
    let codes: Vec<u32> = (0..count)
        .map(|i| ((word >> SOURCE_SHIFTS[i]) & SOURCE_MASK) as u32)
        .collect();
    for lane in wave.active()? {
        let mut lo_args = [0u32; 3];
        let mut hi_args = [0u32; 3];
        for (i, &code) in codes.iter().enumerate() {
            let i = i as u32;
            lo_args[i as usize] = signed_half(
                wave,
                code,
                lane,
                literal,
                bit(word, OP_SEL_SHIFT + i),
                bit(word, NEG_LO_SHIFT + i),
            )?;
            hi_args[i as usize] = signed_half(
                wave,
                code,
                lane,
                literal,
                bit(word, OP_SEL_HI_SHIFTS[i as usize]),
                bit(word, NEG_HI_SHIFT + i),
            )?;
        }
        let mut lo = operation(lo_args);
        let mut hi = operation(hi_args);
        if clamp {
            lo = half_clamp(lo);
            hi = half_clamp(hi);
        }
        wave.v[destination][lane] = lo | (hi << 16);
    }
    Ok(true)
}

/// The refusal of a VOP3P `v_pk_fmac_f16` operand form: SDWA and DPP sources (VOP2), or any VOP3 modifier.
fn refuse_vop3_modifiers(word: u64) -> Result<(), Error> {
    let fields = Fields::new(word);
    let op_sel = (word >> 11) & 0xF;
    let mut present = Vec::new();
    for (label, value) in [
        ("op_sel", op_sel),
        ("abs", u64::from(fields.abs)),
        ("neg", u64::from(fields.neg)),
        ("omod", u64::from(fields.omod)),
    ] {
        if value != 0 {
            present.push(label);
        }
    }
    if present.is_empty() {
        Ok(())
    } else {
        Err(Error::Untranslated(format!(
            "v_pk_fmac_f16 (VOP3) avec {} : non supporté",
            present.join(", ")
        )))
    }
}

/// `v_pk_fmac_f16` in VOP2 (`word` is the 32-bit encoding): d = a × b + d for both halves.
pub fn execute_fmac_vop2(wave: &mut Wave, word: u32, literal: Option<u32>) -> Result<(), Error> {
    let src0 = word & 0x1FF;
    if EXTENSION_CODES.contains(&src0) {
        return Err(Error::Untranslated(format!(
            "v_pk_fmac_f16: forme SDWA/DPP (src0 = 0x{src0:X}) non supportée pour le f16 packé"
        )));
    }
    let src1 = VGPR_FIRST + ((word >> 9) & 0xFF);
    let destination = ((word >> 17) & 0xFF) as usize;
    fmac(wave, destination, src0, src1, literal, false)
}

/// `v_pk_fmac_f16` in VOP3 (`word` is the 64-bit encoding); the clamp bit applies.
pub fn execute_fmac_vop3(wave: &mut Wave, word: u64, literal: Option<u32>) -> Result<(), Error> {
    refuse_vop3_modifiers(word)?;
    let fields = Fields::new(word);
    fmac(
        wave,
        fields.vdst as usize,
        fields.sources[0],
        fields.sources[1],
        literal,
        fields.clamp,
    )
}

fn fmac(
    wave: &mut Wave,
    destination: usize,
    src0: u32,
    src1: u32,
    literal: Option<u32>,
    clamp: bool,
) -> Result<(), Error> {
    for lane in wave.active()? {
        let old = wave.v[destination][lane];
        let a_lo = read_half(wave, src0, lane, literal, false)?;
        let a_hi = read_half(wave, src0, lane, literal, true)?;
        let b_lo = read_half(wave, src1, lane, literal, false)?;
        let b_hi = read_half(wave, src1, lane, literal, true)?;
        let mut lo = half_from_f64(half(a_lo) * half(b_lo) + half(old & HALF_MASK));
        let mut hi = half_from_f64(half(a_hi) * half(b_hi) + half(old >> 16));
        if clamp {
            lo = half_clamp(lo);
            hi = half_clamp(hi);
        }
        wave.v[destination][lane] = lo | (hi << 16);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, args: [u32; 3]) -> u32 {
        let (_, operation) = packed_operation(name).unwrap();
        operation(args)
    }

    #[test]
    fn packed_arithmetic_rounds_each_half_once() {
        // 1.5 + 2.25 = 3.75 (0x4380), 1.5 × 2.25 = 3.375 (0x42C0), 1.5 × 2 + 0.5 = 3.5 (0x4300).
        assert_eq!(run("v_pk_add_f16", [0x3E00, 0x4080, 0]), 0x4380);
        assert_eq!(run("v_pk_mul_f16", [0x3E00, 0x4080, 0]), 0x42C0);
        assert_eq!(run("v_pk_fma_f16", [0x3E00, 0x4000, 0x3800]), 0x4300);
        // Overflow of the sum gives infinity, and a NaN operand gives the canonical NaN.
        assert_eq!(run("v_pk_add_f16", [0x7BFF, 0x7BFF, 0]), 0x7C00);
        assert_eq!(run("v_pk_mul_f16", [0x7E01, 0x3C00, 0]), 0x7E00);
    }

    #[test]
    fn packed_min_and_max_take_the_other_operand_for_a_nan() {
        assert_eq!(run("v_pk_min_f16", [0x7E01, 0x3C00, 0]), 0x3C00);
        assert_eq!(run("v_pk_max_f16", [0xC000, 0x7E01, 0]), 0xC000);
        assert_eq!(run("v_pk_min_f16", [0x8000, 0x0000, 0]), 0x8000);
    }

    #[test]
    fn names_outside_the_packed_set_are_not_handled() {
        assert!(packed_operation("v_pk_fmac_f16").is_none());
        assert!(packed_operation("v_pk_add_u16").is_none());
    }

    #[test]
    fn vop3_fmac_refuses_modifiers() {
        assert!(refuse_vop3_modifiers(0).is_ok());
        assert!(refuse_vop3_modifiers(1 << 11).is_err());
        assert!(refuse_vop3_modifiers(1 << 61).is_err());
        assert!(refuse_vop3_modifiers(1 << 15).is_ok());
    }
}
