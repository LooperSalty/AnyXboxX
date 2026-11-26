//! VINTRP, the parameter interpolation of a pixel shader (port of `_interpolate` in
//! `emu/rdna2_exec_graphics.py`; layout of `emu/rdna2_graphics.py`).
//!
//! The 32-bit word: bits 0-7 VSRC, 8-9 CHAN, 10-15 ATTR, 16-17 OP, 18-25 VDST. Semantics (AMD, "RDNA 2
//! Instruction Set Architecture", Parameter Interpolation): v_interp_p1_f32 D = P10 × S + P0;
//! v_interp_p2_f32 D = P20 × S + D; v_interp_mov_f32 D = P10, P20 or P0 for VSRC 0, 1, 2 (0 for any
//! other VSRC, as the Python's `dict.get`). The product and the sum are each rounded to single
//! precision, as in the Python and its generated HLSL (`precise`), not fused. The NaN results are the
//! ones the Python's x86-64 arithmetic gives (`sse`, `narrow`): a property of the model's host, not a
//! documented RDNA 2 result, carried over as an unverified assumption.
//!
//! On a console P0, P10 = P1 − P0 and P20 = P2 − P0 come from the LDS; in this model they are three
//! words in memory at `PARAMETERS + primitive × PRIMITIVE_STRIDE + (attribute × 4 + channel) × 12`. The
//! primitive of a lane is `Wave::primitives[lane]` when that map is present and holds the lane, else
//! `Wave::primitive`. Attributes past param7 are refused before any lane, as in the Python. The address
//! wraps at 64 bits where the Python's integers do not, which only differs past 2^64 / 384 primitives.

use crate::error::Error;
use crate::float::{bits_float, float_bits};
use crate::wave::Wave;

/// Attributes param0 to param7 (`PARAMETER_COUNT`).
pub const PARAMETER_COUNT: u32 = 8;
const CHANNELS: u64 = 4;
const DWORD: u64 = 4;
/// Start of the parameter area (`PARAMETERS`).
pub const PARAMETERS: u64 = 0x54000;
/// P0, P10 and P20 of one channel (`PARAMETER_STRIDE`).
pub const PARAMETER_STRIDE: u64 = 3 * DWORD;
/// The parameters of one primitive (`PRIMITIVE_STRIDE`).
pub const PRIMITIVE_STRIDE: u64 = PARAMETER_COUNT as u64 * CHANNELS * PARAMETER_STRIDE;

/// The three VINTRP instructions.
const NAMES: [&str; 3] = ["v_interp_p1_f32", "v_interp_p2_f32", "v_interp_mov_f32"];

/// True for the VINTRP instructions this module executes.
pub fn supports(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The fields of a VINTRP word (`vintrp_fields`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VintrpFields {
    pub vsrc: usize,
    pub chan: u32,
    pub attr: u32,
    pub vdst: usize,
}

pub fn fields(word: u32) -> VintrpFields {
    VintrpFields {
        vsrc: (word & 0xFF) as usize,
        chan: (word >> 8) & 0x3,
        attr: (word >> 10) & 0x3F,
        vdst: ((word >> 18) & 0xFF) as usize,
    }
}

/// Address of P0 for a primitive, an attribute and a channel (`parameter_address`).
pub fn parameter_address(primitive: u64, attribute: u32, channel: u32) -> u64 {
    let slot = u64::from(attribute) * CHANNELS + u64::from(channel);
    PARAMETERS
        .wrapping_add(primitive.wrapping_mul(PRIMITIVE_STRIDE))
        .wrapping_add(slot * PARAMETER_STRIDE)
}

const QUIET64: u64 = 1 << 51;
const DEFAULT_NAN64: u64 = 0xFFF8_0000_0000_0000;
const SIGN32: u32 = 0x8000_0000;
const QUIET_NAN32: u32 = 0x7FC0_0000;
const FRACTION32: u32 = 0x003F_FFFF;
/// Bits dropped from a double's fraction to make a single's.
const NARROW_SHIFT: u32 = 29;

/// A double operation as the Python's x86-64 arithmetic does it (SSE2): a NaN operand comes out
/// quieted, the left one first; an invalid operation on non-NaN operands (∞ × 0, ∞ − ∞) gives the
/// default NaN, sign set. Written out because Rust may swap the operands of `*` and `+`.
fn sse(x: f64, y: f64, result: f64) -> f64 {
    if x.is_nan() {
        return f64::from_bits(x.to_bits() | QUIET64);
    }
    if y.is_nan() {
        return f64::from_bits(y.to_bits() | QUIET64);
    }
    if result.is_nan() {
        return f64::from_bits(DEFAULT_NAN64);
    }
    result
}

/// Single-precision bits of a double, a NaN keeping its sign and the top of its fraction, quiet (the
/// conversion `struct.pack("<f")` makes on x86-64).
fn narrow(value: f64) -> u32 {
    if !value.is_nan() {
        return float_bits(value);
    }
    let bits = value.to_bits();
    ((bits >> 32) as u32 & SIGN32) | QUIET_NAN32 | ((bits >> NARROW_SHIFT) as u32 & FRACTION32)
}

/// `a × b + c`, the product and the sum each rounded to single precision (`_multiply_add`), with the
/// Python's NaN results (`sse`, `narrow`).
pub fn multiply_add(a: u32, b: u32, c: u32) -> u32 {
    let (x, y) = (bits_float(a), bits_float(b));
    let product = bits_float(narrow(sse(x, y, x * y)));
    let addend = bits_float(c);
    narrow(sse(product, addend, product + addend))
}

/// P0, P10 and P20 of the lane's primitive.
fn parameters(wave: &Wave, lane: usize, attribute: u32, channel: u32) -> [u32; 3] {
    let primitive = wave
        .primitives
        .as_ref()
        .and_then(|primitives| primitives.get(&lane).copied())
        .unwrap_or(wave.primitive);
    let bytes = wave
        .memory
        .read(parameter_address(primitive, attribute, channel), 12);
    let word = |k: usize| u32::from_le_bytes([bytes[k], bytes[k + 1], bytes[k + 2], bytes[k + 3]]);
    [word(0), word(4), word(8)]
}

/// Executes one VINTRP instruction (`name` from `supports`) for every lane of EXEC.
pub fn execute(wave: &mut Wave, name: &str, word: u32) -> Result<(), Error> {
    let fields = fields(word);
    if fields.attr >= PARAMETER_COUNT {
        return Err(Error::Untranslated(format!(
            "{name} : attribut {} au-delà de param{}",
            fields.attr,
            PARAMETER_COUNT - 1
        )));
    }
    for lane in wave.active()? {
        let [p0, p10, p20] = parameters(wave, lane, fields.attr, fields.chan);
        let source = fields.vsrc;
        let value = match name {
            "v_interp_p1_f32" => multiply_add(p10, wave.v[source][lane], p0),
            "v_interp_p2_f32" => multiply_add(p20, wave.v[source][lane], wave.v[fields.vdst][lane]),
            _ => match source {
                0 => p10,
                1 => p20,
                2 => p0,
                _ => 0,
            },
        };
        wave.v[fields.vdst][lane] = value;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn vintrp(op: u32, vdst: u32, vsrc: u32, attr: u32, chan: u32) -> u32 {
        0x32 << 26 | vdst << 18 | op << 16 | attr << 10 | chan << 8 | vsrc
    }

    fn set_parameters(wave: &mut Wave, primitive: u64, attr: u32, chan: u32, p: [f32; 3]) {
        let words: Vec<u32> = p.iter().map(|value| value.to_bits()).collect();
        wave.memory
            .write_words(parameter_address(primitive, attr, chan), &words);
    }

    #[test]
    fn p1_then_p2_give_p0_plus_i_p10_plus_j_p20() {
        let mut wave = Wave::new(32).unwrap();
        set_parameters(&mut wave, 0, 3, 2, [1.0, 2.0, 4.0]);
        wave.v[1] = vec![0.5f32.to_bits(); 32];
        wave.v[2] = vec![0.25f32.to_bits(); 32];
        execute(&mut wave, "v_interp_p1_f32", vintrp(0, 5, 1, 3, 2)).unwrap();
        execute(&mut wave, "v_interp_p2_f32", vintrp(1, 5, 2, 3, 2)).unwrap();
        assert_eq!(f32::from_bits(wave.v[5][7]), 3.0);
    }

    #[test]
    fn the_product_is_rounded_before_the_sum() {
        // (1 + 2^-23)^2 = 1 + 2^-22 + 2^-46: rounded first, the 2^-46 is lost; fused, it survives.
        let a = 1.0f32 + f32::EPSILON;
        let c = -(1.0f32 + 2.0 * f32::EPSILON);
        let separate = multiply_add(a.to_bits(), a.to_bits(), c.to_bits());
        assert_eq!(f32::from_bits(separate), 0.0);
        assert_ne!(a.mul_add(a, c), 0.0);
    }

    #[test]
    fn nan_results_follow_the_python_x86_arithmetic() {
        let (one, infinity, zero) = (0x3F80_0000, 0x7F80_0000, 0);
        // ∞ × 0: the default NaN, sign set, and it is the left operand of the sum.
        assert_eq!(multiply_add(infinity, zero, 0x7FC0_0001), 0xFFC0_0000);
        // A NaN operand of the product wins over a NaN addend; a signaling one comes out quiet.
        assert_eq!(multiply_add(one, 0x7FC0_0002, 0xFFC0_0003), 0x7FC0_0002);
        assert_eq!(multiply_add(0xFF80_0004, one, zero), 0xFFC0_0004);
        // The addend's NaN, when the product is a number.
        assert_eq!(multiply_add(one, one, 0x7F80_0005), 0x7FC0_0005);
    }

    #[test]
    fn mov_picks_p10_p20_p0_and_any_other_source_gives_zero() {
        let mut wave = Wave::new(64).unwrap();
        set_parameters(&mut wave, 0, 0, 0, [1.0, 2.0, 3.0]);
        let picks: Vec<u32> = (0..4)
            .map(|vsrc| {
                execute(&mut wave, "v_interp_mov_f32", vintrp(2, 9, vsrc, 0, 0)).unwrap();
                wave.v[9][63]
            })
            .collect();
        assert_eq!(
            picks,
            [2.0f32.to_bits(), 3.0f32.to_bits(), 1.0f32.to_bits(), 0]
        );
    }

    #[test]
    fn each_lane_reads_its_own_primitive_or_the_default_one() {
        let mut wave = Wave::new(32).unwrap();
        set_parameters(&mut wave, 2, 7, 3, [0.0, 0.0, 5.0]);
        set_parameters(&mut wave, 4, 7, 3, [0.0, 0.0, 6.0]);
        wave.primitive = 2;
        wave.primitives = Some(BTreeMap::from([(1, 4)]));
        execute(&mut wave, "v_interp_mov_f32", vintrp(2, 1, 1, 7, 3)).unwrap();
        assert_eq!(f32::from_bits(wave.v[1][0]), 5.0);
        assert_eq!(f32::from_bits(wave.v[1][1]), 6.0);
    }

    #[test]
    fn an_attribute_past_param7_is_refused_even_with_an_empty_exec() {
        let mut wave = Wave::new(32).unwrap();
        wave.write_mask(crate::wave::EXEC_LO, 0).unwrap();
        match execute(&mut wave, "v_interp_p1_f32", vintrp(0, 1, 0, 8, 0)) {
            Err(Error::Untranslated(message)) => {
                assert!(message.contains("attribut 8"), "{message}")
            }
            other => panic!("expected untranslated, got {other:?}"),
        }
    }
}
