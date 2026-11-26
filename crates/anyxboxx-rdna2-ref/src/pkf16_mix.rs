//! Mixed-precision FMA (`v_fma_mix_f32`, `v_fma_mixlo_f16`, `v_fma_mixhi_f16`) and `v_dot2_f32_f16` of VOP3P,
//! ported from `emu/rdna2_exec_pkf16.py`. Each source is f16 or f32 as its `op_sel_hi` bit says.

use crate::error::Error;
use crate::float::{bits_float, float_bits, fma};
use crate::half::{HALF_MASK, half_clamp, half_from_f64, half_to_f32_bits, read_half, signed_half};
use crate::vop3p::{
    CLAMP_BIT, NEG_HI_SHIFT, NEG_LO_SHIFT, OP_SEL_HI_SHIFTS, OP_SEL_SHIFT, SOURCE_MASK,
    SOURCE_SHIFTS, VDST_MASK,
};
use crate::wave::Wave;

const FLOAT_SIGN: u32 = 0x8000_0000;
const FLOAT_ABS: u32 = 0x7FFF_FFFF;
const FLOAT_ONE: u32 = 0x3F80_0000;
const FLOAT_INFINITY: u32 = 0x7F80_0000;

/// `clamp_f32`: the VOP3 clamp of an f32 result to [0, 1]; NaN and negatives (-0 included) give +0.
fn clamp_f32(bits: u32) -> u32 {
    if (bits & FLOAT_ABS) > FLOAT_INFINITY || bits & FLOAT_SIGN != 0 {
        0
    } else {
        bits.min(FLOAT_ONE)
    }
}

fn bit(word: u64, shift: u32) -> bool {
    (word >> shift) & 1 == 1
}

fn source_code(word: u64, index: usize) -> u32 {
    ((word >> SOURCE_SHIFTS[index]) & SOURCE_MASK) as u32
}

/// `vop3p_refusal` for the forms of this module: the operand selections the model does not define.
fn refusal(name: &str, word: u64) -> Option<String> {
    match name {
        "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" => (0..3).find_map(|i| {
            (!bit(word, OP_SEL_HI_SHIFTS[i]) && bit(word, OP_SEL_SHIFT + i as u32)).then(|| {
                format!("{name}: op_sel[{i}] sur une source f32 (op_sel_hi[{i}]=0) non défini")
            })
        }),
        "v_dot2_f32_f16" => {
            (bit(word, OP_SEL_SHIFT + 2) || bit(word, NEG_HI_SHIFT + 2)).then(|| {
                format!("{name}: op_sel[2] ou neg_hi[2] sur l'accumulateur f32 src2 non défini")
            })
        }
        _ => None,
    }
}

/// Runs one of the mixed-precision or dot-product forms. `Ok(false)` when `name` is not one of them.
pub fn execute(
    wave: &mut Wave,
    name: &str,
    word: u64,
    literal: Option<u32>,
) -> Result<bool, Error> {
    match name {
        "v_fma_mix_f32" | "v_fma_mixlo_f16" | "v_fma_mixhi_f16" => {
            refuse(name, word)?;
            execute_mix(wave, name, word, literal)?;
        }
        "v_dot2_f32_f16" => {
            refuse(name, word)?;
            execute_dot2(wave, word, literal)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn refuse(name: &str, word: u64) -> Result<(), Error> {
    match refusal(name, word) {
        Some(message) => Err(Error::Untranslated(message)),
        None => Ok(()),
    }
}

/// The f32 bits of source `index` for the mix form: an f16 half (`op_sel` picks it), or the f32 word,
/// then its absolute value (`neg_hi`) and negation (`neg_lo`).
fn mix_argument(
    wave: &Wave,
    word: u64,
    index: usize,
    lane: usize,
    literal: Option<u32>,
) -> Result<u32, Error> {
    let code = source_code(word, index);
    let is_f16 = bit(word, OP_SEL_HI_SHIFTS[index]);
    let mut value = if is_f16 {
        let half = read_half(
            wave,
            code,
            lane,
            literal,
            bit(word, OP_SEL_SHIFT + index as u32),
        )?;
        half_to_f32_bits(half)
    } else {
        wave.read_lane(code, lane, literal)?
    };
    if bit(word, NEG_HI_SHIFT + index as u32) {
        value &= FLOAT_ABS;
    }
    if bit(word, NEG_LO_SHIFT + index as u32) {
        value ^= FLOAT_SIGN;
    }
    Ok(value)
}

/// v_fma_mix_f32 (f32 result), v_fma_mixlo_f16 and v_fma_mixhi_f16 (half result, the other half kept).
fn execute_mix(wave: &mut Wave, name: &str, word: u64, literal: Option<u32>) -> Result<(), Error> {
    let clamp = bit(word, CLAMP_BIT);
    let destination = (word & VDST_MASK) as usize;
    for lane in wave.active()? {
        let a = mix_argument(wave, word, 0, lane, literal)?;
        let b = mix_argument(wave, word, 1, lane, literal)?;
        let c = mix_argument(wave, word, 2, lane, literal)?;
        let result = float_bits(fma(bits_float(a), bits_float(b), bits_float(c)));
        let old = wave.v[destination][lane];
        wave.v[destination][lane] = match name {
            "v_fma_mix_f32" => {
                if clamp {
                    clamp_f32(result)
                } else {
                    result
                }
            }
            _ => {
                let mut half = half_from_f64(bits_float(result));
                if clamp {
                    half = half_clamp(half);
                }
                if name == "v_fma_mixhi_f16" {
                    (old & HALF_MASK) | (half << 16)
                } else {
                    (old & 0xFFFF_0000) | half
                }
            }
        };
    }
    Ok(())
}

/// v_dot2_f32_f16: (a_lo × b_lo + a_hi × b_hi) + c, each product and sum rounded to f32, then the
/// accumulator c (f32, `neg_lo[2]` negates it). The clamp bit applies.
fn execute_dot2(wave: &mut Wave, word: u64, literal: Option<u32>) -> Result<(), Error> {
    let clamp = bit(word, CLAMP_BIT);
    let destination = (word & VDST_MASK) as usize;
    let (a, b) = (source_code(word, 0), source_code(word, 1));
    for lane in wave.active()? {
        let a_lo = signed_half(
            wave,
            a,
            lane,
            literal,
            bit(word, OP_SEL_SHIFT),
            bit(word, NEG_LO_SHIFT),
        )?;
        let a_hi = signed_half(
            wave,
            a,
            lane,
            literal,
            bit(word, OP_SEL_HI_SHIFTS[0]),
            bit(word, NEG_HI_SHIFT),
        )?;
        let b_lo = signed_half(
            wave,
            b,
            lane,
            literal,
            bit(word, OP_SEL_SHIFT + 1),
            bit(word, NEG_LO_SHIFT + 1),
        )?;
        let b_hi = signed_half(
            wave,
            b,
            lane,
            literal,
            bit(word, OP_SEL_HI_SHIFTS[1]),
            bit(word, NEG_HI_SHIFT + 1),
        )?;
        let c = wave.read_lane(source_code(word, 2), lane, literal)?
            ^ if bit(word, NEG_LO_SHIFT + 2) {
                FLOAT_SIGN
            } else {
                0
            };
        let p0 =
            float_bits(bits_float(half_to_f32_bits(a_lo)) * bits_float(half_to_f32_bits(b_lo)));
        let p1 =
            float_bits(bits_float(half_to_f32_bits(a_hi)) * bits_float(half_to_f32_bits(b_hi)));
        let sum = float_bits(bits_float(p0) + bits_float(p1));
        let result = float_bits(bits_float(sum) + bits_float(c));
        wave.v[destination][lane] = if clamp { clamp_f32(result) } else { result };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot2_refuses_op_sel_and_neg_hi_on_the_accumulator() {
        assert!(refusal("v_dot2_f32_f16", 1 << 13).is_some());
        assert!(refusal("v_dot2_f32_f16", 1 << 10).is_some());
        assert!(refusal("v_dot2_f32_f16", 1 << 11).is_none());
        assert!(refusal("v_dot2_f32_f16", 1 << 61).is_none());
    }

    #[test]
    fn mix_refuses_op_sel_on_an_f32_source() {
        // op_sel[0] set while op_sel_hi[0] (bit 59) is clear.
        assert!(refusal("v_fma_mix_f32", 1 << 11).is_some());
        assert!(refusal("v_fma_mix_f32", (1 << 11) | (1 << 59)).is_none());
    }

    #[test]
    fn clamp_f32_gives_zero_for_nan_and_negatives() {
        assert_eq!(clamp_f32(0x7FC0_0000), 0);
        assert_eq!(clamp_f32(0x8000_0000), 0);
        assert_eq!(clamp_f32(0x4000_0000), FLOAT_ONE);
        assert_eq!(clamp_f32(FLOAT_INFINITY), FLOAT_ONE);
    }

    #[test]
    fn names_outside_the_module_are_not_handled() {
        let mut wave = Wave::new(32).unwrap();
        assert_eq!(execute(&mut wave, "v_pk_add_f16", 0, None), Ok(false));
    }
}
