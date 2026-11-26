//! Packed d16 buffer conversions, following `emu/rdna2_buffer_d16.py`.
use crate::buffer_format::{Format, Kind};
use crate::format_convert::{self, half_bits};

fn round_shift(value: u32, drop: u32, toward_zero: bool) -> u32 {
    let quotient = value >> drop;
    let remainder = value & ((1 << drop) - 1);
    let half = 1 << (drop - 1);
    quotient
        + u32::from(!toward_zero && (remainder > half || (remainder == half && quotient & 1 != 0)))
}

fn half_from_float(bits: u32, toward_zero: bool) -> u32 {
    let sign = (bits >> 16) & 0x8000;
    let exponent = (bits >> 23) & 0xff;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        let payload = mantissa >> 13;
        return sign
            | 0x7c00
            | if mantissa != 0 && payload == 0 {
                1
            } else {
                payload
            };
    }
    if exponent < 101 {
        return sign;
    }
    let full = mantissa | 1 << 23;
    let biased = exponent as i32 - 112;
    let rounded = if biased >= 1 {
        ((biased as u32 - 1) << 10) + round_shift(full, 13, toward_zero)
    } else {
        round_shift(full, (14 - biased) as u32, toward_zero)
    };
    sign | rounded.min(if toward_zero { 0x7bff } else { 0x7c00 })
}

fn half_from_ratio(numerator: u64, denominator: u64) -> u32 {
    if numerator == 0 {
        return 0;
    }
    let mut shift = 0;
    while numerator << shift < denominator {
        shift += 1;
    }
    let exponent_shift = shift.min(14);
    let scaled = numerator << (exponent_shift + 10);
    let (mut quotient, remainder) = (scaled / denominator, scaled % denominator);
    if 2 * remainder > denominator || (2 * remainder == denominator && quotient & 1 != 0) {
        quotient += 1;
    }
    ((14 - exponent_shift) << 10) + quotient as u32
}

pub fn decode(format: &Format, data: &[u8]) -> Vec<u32> {
    if !matches!(format.kind, Kind::Unorm | Kind::Snorm) {
        return format_convert::decode(format, data);
    }
    let packed = data
        .iter()
        .rev()
        .fold(0u128, |n, b| n << 8 | u128::from(*b));
    let mut position = 0;
    format
        .widths
        .iter()
        .map(|&width| {
            let field = ((packed >> position) & ((1u128 << width) - 1)) as u64;
            position += width;
            let half = if format.kind == Kind::Unorm {
                half_from_ratio(field, (1u64 << width) - 1)
            } else {
                let value = if field & (1 << (width - 1)) != 0 {
                    field as i64 - (1i64 << width)
                } else {
                    field as i64
                };
                let largest = (1i64 << (width - 1)) - 1;
                if value <= -largest {
                    0xbc00
                } else {
                    half_from_ratio(value.unsigned_abs(), largest as u64)
                        | if value < 0 { 0x8000 } else { 0 }
                }
            };
            half_bits(half)
        })
        .collect()
}

pub fn narrow(format: &Format, value: u32) -> u32 {
    if format.is_integer() {
        value & 0xffff
    } else {
        half_from_float(value, format.kind == Kind::Float)
    }
}

pub fn widen(format: &Format, half: u32, width: u32) -> u32 {
    match format.kind {
        Kind::Sint => (half as i16 as i32) as u32,
        Kind::Float if width != 16 => half_bits(half),
        _ => half,
    }
}
