//! VOP3P integer dot products and packed integer operations, ported from the Python interpreter.
//!
//! D = S2 + Σ S0[i] × S1[i] over 32 bits, where S0 and S1 are read as 2 values of 16 bits, 4 of 8 bits or
//! 8 of 4 bits. Only the default form runs: op_sel 0, op_sel_hi all ones, no neg_lo, neg_hi or clamp. The
//! other forms are refused, naming the modifier. Packed integer operations support half selection
//! and the clamp forms implemented by the Python model.

use crate::error::Error;
use crate::wave::Wave;

const MASK32: u64 = 0xFFFF_FFFF;
pub(crate) const VDST_MASK: u64 = 0xFF;
pub(crate) const SOURCE_SHIFTS: [u32; 3] = [32, 41, 50];
pub(crate) const SOURCE_MASK: u64 = 0x1FF;
pub(crate) const NEG_HI_SHIFT: u32 = 8;
pub(crate) const OP_SEL_SHIFT: u32 = 11;
pub(crate) const NEG_LO_SHIFT: u32 = 61;
pub(crate) const OP_SEL_HI_SHIFTS: [u32; 3] = [59, 60, 14];
pub(crate) const CLAMP_BIT: u32 = 15;
pub(crate) const THREE_BITS: u64 = 0x7;

/// The dot products of this module: element width in bits and whether the elements are signed.
pub fn operation(name: &str) -> Option<(u32, bool)> {
    match name {
        "v_dot2_i32_i16" => Some((16, true)),
        "v_dot2_u32_u16" => Some((16, false)),
        "v_dot4_i32_i8" => Some((8, true)),
        "v_dot4_u32_u8" => Some((8, false)),
        "v_dot8_i32_i4" => Some((4, true)),
        "v_dot8_u32_u4" => Some((4, false)),
        _ => None,
    }
}

/// The modifiers present outside the default form, by name (empty for the default form).
fn unsupported_modifiers(word: u64) -> Vec<&'static str> {
    let mut found = Vec::new();
    if (word >> OP_SEL_SHIFT) & THREE_BITS != 0 {
        found.push("op_sel");
    }
    if OP_SEL_HI_SHIFTS
        .iter()
        .any(|shift| (word >> shift) & 1 == 0)
    {
        found.push("op_sel_hi");
    }
    if (word >> NEG_LO_SHIFT) & THREE_BITS != 0 {
        found.push("neg_lo");
    }
    if (word >> NEG_HI_SHIFT) & THREE_BITS != 0 {
        found.push("neg_hi");
    }
    if (word >> CLAMP_BIT) & 1 != 0 {
        found.push("clamp");
    }
    found
}

/// S2 + Σ S0[i] × S1[i] modulo 2^32, with the element width and signedness of the operation.
pub fn dot(width: u32, signed: bool, a: u32, b: u32, c: u32) -> u32 {
    let mask = (1u64 << width) - 1;
    let element = |value: u32, shift: u32| -> i64 {
        let raw = (u64::from(value) >> shift) & mask;
        if signed {
            let sign = 1u64 << (width - 1);
            if raw & sign != 0 {
                raw as i64 - (1i64 << width)
            } else {
                raw as i64
            }
        } else {
            raw as i64
        }
    };
    let mut total = i64::from(c);
    for shift in (0..32).step_by(width as usize) {
        total += element(a, shift) * element(b, shift);
    }
    (total as u64 & MASK32) as u32
}

/// Executes a dot product for every active lane. `Ok(false)` when `name` is not one of them.
pub fn execute(
    wave: &mut Wave,
    name: &str,
    word: u64,
    literal: Option<u32>,
) -> Result<bool, Error> {
    if crate::pkf16::execute_packed(wave, name, word, literal)?
        || crate::pkf16_mix::execute(wave, name, word, literal)?
    {
        return Ok(true);
    }
    if let Some(op) = name.strip_prefix("v_pk_")
        && packed(wave, op, word, literal)?
    {
        return Ok(true);
    }
    let Some((width, signed)) = operation(name) else {
        return Ok(false);
    };
    let modifiers = unsupported_modifiers(word);
    if !modifiers.is_empty() {
        return Err(Error::Untranslated(format!(
            "{name} avec {} : sens non documenté pour RDNA 2",
            modifiers.join(", ")
        )));
    }
    let codes: Vec<u32> = SOURCE_SHIFTS
        .iter()
        .map(|shift| ((word >> shift) & SOURCE_MASK) as u32)
        .collect();
    let destination = (word & VDST_MASK) as usize;
    for lane in wave.active()? {
        let a = wave.read_lane(codes[0], lane, literal)?;
        let b = wave.read_lane(codes[1], lane, literal)?;
        let c = wave.read_lane(codes[2], lane, literal)?;
        wave.v[destination][lane] = dot(width, signed, a, b, c);
    }
    Ok(true)
}

fn packed(wave: &mut Wave, op: &str, word: u64, literal: Option<u32>) -> Result<bool, Error> {
    let supported = matches!(
        op,
        "add_u16"
            | "sub_u16"
            | "add_i16"
            | "sub_i16"
            | "mad_u16"
            | "mad_i16"
            | "mul_lo_u16"
            | "max_u16"
            | "max_i16"
            | "min_u16"
            | "min_i16"
            | "lshlrev_b16"
            | "lshrrev_b16"
            | "ashrrev_i16"
    );
    if !supported {
        return Ok(false);
    }
    let clamp = word >> CLAMP_BIT & 1 != 0;
    if word >> NEG_LO_SHIFT & THREE_BITS != 0
        || word >> NEG_HI_SHIFT & THREE_BITS != 0
        || (clamp && (op.starts_with("mul") || op.contains("shr") || op.contains("shl")))
    {
        return Err(Error::Untranslated(format!(
            "v_pk_{op}: unsupported modifier"
        )));
    }
    let count = if op.starts_with("mad") { 3 } else { 2 };
    let dest = (word & VDST_MASK) as usize;
    for lane in wave.active()? {
        let mut values = [0u32; 3];
        for i in 0..count {
            values[i] = wave.read_lane(
                ((word >> SOURCE_SHIFTS[i]) & SOURCE_MASK) as u32,
                lane,
                literal,
            )?;
        }
        let mut result = 0;
        for half in 0..2 {
            let mut v = [0i64; 3];
            for i in 0..count {
                let select = if half == 0 {
                    (word >> (OP_SEL_SHIFT + i as u32)) & 1
                } else {
                    (word >> OP_SEL_HI_SHIFTS[i]) & 1
                };
                let raw = ((values[i] >> (select * 16)) & 0xffff) as i64;
                v[i] = if op.ends_with("i16") {
                    (raw as i16) as i64
                } else {
                    raw
                };
            }
            let [a, b, c] = v;
            let value = match op.split('_').next().unwrap_or("") {
                "add" => a + b,
                "sub" => a - b,
                "mad" => a * b + c,
                "mul" => a * b,
                "max" => a.max(b),
                "min" => a.min(b),
                "lshlrev" => b << (a & 15),
                "lshrrev" | "ashrrev" => b >> (a & 15),
                _ => unreachable!(),
            };
            let value = if clamp
                && (op.starts_with("add") || op.starts_with("sub") || op.starts_with("mad"))
            {
                if op.ends_with("i16") {
                    value.clamp(-32768, 32767)
                } else {
                    value.clamp(0, 65535)
                }
            } else {
                value
            };
            result |= ((value as u32) & 0xffff) << (half * 16);
        }
        wave.v[dest][lane] = result;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_and_signed_dot_products_follow_the_elements() {
        // 0x02 * 0x03 + 0x04 * 0x05 + 10 = 36 for four bytes (only the low two here: 0x0201 * 0x0403 ...).
        assert_eq!(
            dot(8, false, 0x0403_0201, 0x0605_0403, 10),
            3 + 8 + 15 + 24 + 10
        );
        // -1 * -1 for signed bytes.
        assert_eq!(dot(8, true, 0xFF, 0xFF, 0), 1);
    }

    #[test]
    fn default_form_has_no_modifier_and_op_sel_hi_must_be_set() {
        let default = 1u64 << 59 | 1u64 << 60 | 1u64 << 14;
        assert!(unsupported_modifiers(default).is_empty());
        assert_eq!(unsupported_modifiers(0), vec!["op_sel_hi"]);
    }
}
