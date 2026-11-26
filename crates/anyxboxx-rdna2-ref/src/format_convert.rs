//! Conversions of typed buffer accesses between the bits of an element and the 32-bit VGPR values. Port
//! of the conversions of `emu/rdna2_buffer_format.py` (`ratio_bits`, `unorm_bits`, `snorm_bits`,
//! `half_bits`, `decode`, `encode`, `select_one`), with integer arithmetic only.
//!
//! Loads: UNORM on n bits is c / (2^n - 1); SNORM is max(c / (2^(n-1) - 1), -1), so -2^(n-1) gives -1.
//! Both round the exact quotient to the nearest float32, ties to even (the quotient is never halfway:
//! its binary expansion repeats forever, but the rule is kept as the Python writes it). USCALED and
//! SSCALED give the integer as a float32 (exact: 16 bits at most); UINT is zero-extended, SINT
//! sign-extended. 32-bit FLOAT keeps its bits; 16-bit FLOAT widens exactly (denormals included), a NaN
//! keeps its sign and its payload shifted, with no quiet bit added.
//! Stores: UINT saturates the 32-bit value to 2^n - 1, SINT clamps it to [-2^(n-1), 2^(n-1) - 1];
//! 32-bit components are written as they are.

use crate::buffer_format::{Format, Kind};

const MINUS_ONE: u32 = 0xBF80_0000;
const SIGN_BIT: u32 = 0x8000_0000;
const ONE_FLOAT: u32 = 0x3F80_0000;
const ONE_INTEGER: u32 = 1;
const MANTISSA_BITS: u32 = 23;
const MANTISSA_MASK: u32 = 0x7F_FFFF;
/// The biased exponent of 2^0 plus the 23 mantissa bits: a mantissa scaled by 2^shift has exponent
/// `EXPONENT_BASE - shift`.
const EXPONENT_BASE: i64 = 150;
const DWORD_BITS: u32 = 32;

/// `value` (its low `width` bits) as a signed number.
fn signed(value: u64, width: u32) -> i64 {
    let masked = value & ((1u64 << width) - 1);
    if masked >> (width - 1) & 1 == 1 {
        masked as i64 - (1i64 << width)
    } else {
        masked as i64
    }
}

/// The float32 (bits) nearest to numerator / denominator, ties to even, for 0 <= numerator <= denominator.
pub fn ratio_bits(numerator: u64, denominator: u64) -> u32 {
    if numerator == 0 {
        return 0;
    }
    let mut shift: u32 = 0;
    while numerator << shift < denominator << MANTISSA_BITS {
        shift += 1;
    }
    let scaled = numerator << shift;
    let (mut mantissa, remainder) = (scaled / denominator, scaled % denominator);
    if 2 * remainder > denominator || (2 * remainder == denominator && mantissa & 1 == 1) {
        mantissa += 1;
    }
    let mut exponent = EXPONENT_BASE - i64::from(shift);
    if mantissa >> (MANTISSA_BITS + 1) != 0 {
        mantissa >>= 1;
        exponent += 1;
    }
    (exponent as u32) << MANTISSA_BITS | (mantissa as u32 & MANTISSA_MASK)
}

/// UNORM on `width` bits: value / (2^width - 1).
pub fn unorm_bits(value: u64, width: u32) -> u32 {
    ratio_bits(value, (1u64 << width) - 1)
}

/// SNORM on `width` bits: value / (2^(width-1) - 1), at least -1.
pub fn snorm_bits(value: u64, width: u32) -> u32 {
    let number = signed(value, width);
    let largest = (1i64 << (width - 1)) - 1;
    if number <= -largest {
        return MINUS_ONE;
    }
    let magnitude = ratio_bits(number.unsigned_abs(), largest as u64);
    if number < 0 {
        magnitude | SIGN_BIT
    } else {
        magnitude
    }
}

/// float16 to float32, exact; infinities and NaNs keep their payload (shifted by 13 bits).
pub fn half_bits(half: u32) -> u32 {
    let sign = (half & 0x8000) << 16;
    let exponent = (half >> 10) & 0x1F;
    let mantissa = half & 0x3FF;
    if exponent == 0x1F {
        return sign | 0x7F80_0000 | mantissa << 13;
    }
    if exponent != 0 {
        return sign | (exponent + 112) << MANTISSA_BITS | mantissa << 13;
    }
    if mantissa == 0 {
        return sign;
    }
    let top = 31 - mantissa.leading_zeros();
    sign | (top + 103) << MANTISSA_BITS | (mantissa << (MANTISSA_BITS - top)) & MANTISSA_MASK
}

fn component(format: &Format, field: u64, width: u32) -> u32 {
    match format.kind {
        Kind::Unorm => unorm_bits(field, width),
        Kind::Snorm => snorm_bits(field, width),
        Kind::Uscaled => (field as f32).to_bits(),
        Kind::Sscaled => (signed(field, width) as f32).to_bits(),
        Kind::Sint => signed(field, width) as u32,
        Kind::Float if width == 16 => half_bits(field as u32),
        Kind::Uint | Kind::Float => field as u32,
    }
}

/// The components (32-bit values) of one element of `format.size` bytes.
pub fn decode(format: &Format, data: &[u8]) -> Vec<u32> {
    let packed = data
        .iter()
        .rev()
        .fold(0u128, |acc, byte| acc << 8 | u128::from(*byte));
    let mut position = 0;
    format
        .widths
        .iter()
        .map(|&width| {
            let field = (packed >> position) as u64 & ((1u64 << width) - 1);
            position += width;
            component(format, field, width)
        })
        .collect()
}

fn field(format: &Format, value: u32, width: u32) -> u64 {
    if format.kind == Kind::Float && width == 16 {
        return u64::from(value & 0xffff);
    }
    if width == DWORD_BITS {
        return u64::from(value);
    }
    let mask = (1u64 << width) - 1;
    if format.kind == Kind::Uint {
        return u64::from(value).min(mask);
    }
    let low = -(1i64 << (width - 1));
    let high = (1i64 << (width - 1)) - 1;
    (i64::from(value as i32).clamp(low, high) as u64) & mask
}

/// The bytes of one element from the 32-bit VGPR values (a storable format).
pub fn encode(format: &Format, values: &[u32]) -> Vec<u8> {
    let mut packed = 0u128;
    let mut position = 0;
    for (value, &width) in values.iter().zip(format.widths) {
        packed |= u128::from(field(format, *value, width)) << position;
        position += width;
    }
    packed.to_le_bytes()[..format.size].to_vec()
}

/// The value of DST_SEL = 1: 1.0 for the float, normalized and scaled formats, 1 for UINT and SINT.
pub fn select_one(format: &Format) -> u32 {
    if format.is_integer() {
        ONE_INTEGER
    } else {
        ONE_FLOAT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_format::format_for;

    #[test]
    fn unorm_and_snorm_round_to_the_nearest_float() {
        assert_eq!(unorm_bits(255, 8), 0x3F80_0000);
        assert_eq!(unorm_bits(1, 8), (1.0f32 / 255.0).to_bits());
        assert_eq!(unorm_bits(1, 16), (1.0f32 / 65535.0).to_bits());
        assert_eq!(snorm_bits(0x80, 8), MINUS_ONE);
        assert_eq!(snorm_bits(0x81, 8), MINUS_ONE);
        assert_eq!(snorm_bits(0x82, 8), (-126.0f32 / 127.0).to_bits());
        assert_eq!(snorm_bits(2, 2), MINUS_ONE);
        assert_eq!(snorm_bits(1, 2), 0x3F80_0000);
    }

    #[test]
    fn halves_widen_exactly() {
        assert_eq!(half_bits(0x3C00), 0x3F80_0000);
        assert_eq!(half_bits(0x0001), (2.0f32).powi(-24).to_bits());
        assert_eq!(half_bits(0xFC00), 0xFF80_0000);
        assert_eq!(half_bits(0x7E01), 0x7FC0_2000);
        assert_eq!(half_bits(0x8000), SIGN_BIT);
    }

    #[test]
    fn packed_formats_decode_from_bit_zero_and_stores_saturate() {
        let format = format_for(54, false).unwrap(); // 2_10_10_10_UINT
        let element = (3u32 << 30 | 0x155 << 20 | 0x2AA << 10 | 0x3FF).to_le_bytes();
        assert_eq!(decode(format, &element), vec![0x3FF, 0x2AA, 0x155, 3]);
        assert_eq!(encode(format, &[0x3FF, 0x2AA, 0x155, 3]), element.to_vec());
        let sint = format_for(6, true).unwrap(); // 8_SINT
        assert_eq!(encode(sint, &[0xFFFF_FF00]), vec![0x80]);
        assert_eq!(encode(sint, &[300]), vec![0x7F]);
        let uint = format_for(5, true).unwrap(); // 8_UINT
        assert_eq!(encode(uint, &[0xFFFF_FFFF]), vec![0xFF]);
    }
}
