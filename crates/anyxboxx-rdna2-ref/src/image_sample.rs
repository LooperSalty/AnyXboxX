//! Sampling of the MIMG instructions: addressing modes, point and bilinear filtering, depth comparison
//! and gather4. Port of `clamp_index`, `texel_index`, `bilinear_axis`, `_lerp_bits`, `_compare_depth`,
//! `_sample_color`, `_sample_compare`, `_gather4`, `_unpack_offset` and `_use_linear` of
//! `emu/rdna2_exec_image.py`.
//!
//! Every step is a float32 operation, as the Python rounds each one to float32 (its float64 result
//! then rounds to the same float32: double rounding is harmless for +, - and x at these widths).
//! Coordinates are clamped to [-2147483520, 2147483520] before the floor, so the 6-bit offsets of the
//! _o forms cannot overflow 32 bits. A NaN coordinate gives the offset itself as the index.

use crate::image_desc::{
    CLAMP_LAST_TEXEL, CLAMP_MIRROR, CLAMP_MIRROR_ONCE_BORDER, CLAMP_MIRROR_ONCE_LAST_TEXEL,
    CLAMP_WRAP, FILTER_LINEAR, Image, Sampler,
};
use crate::image_texel::{ONE_BITS, fetch_texel, select, swizzled};
use crate::memory::Memory;

const INT_MIN: f32 = -2_147_483_520.0;
const INT_MAX: f32 = 2_147_483_520.0;
const OFFSET_BITS: u32 = 0x3F;
const OFFSET_SIGN: u32 = 0x20;
const OFFSET_RANGE: i64 = 64;

/// The texel index after the addressing mode, or `None` outside the image in a border mode.
pub fn clamp_index(index: i64, size: u32, mode: u32) -> Option<u64> {
    let size = i64::from(size);
    let found = match mode {
        CLAMP_WRAP => index.rem_euclid(size),
        CLAMP_MIRROR => {
            let period = 2 * size;
            let pos = index.rem_euclid(period);
            if pos < size { pos } else { period - 1 - pos }
        }
        CLAMP_LAST_TEXEL => index.clamp(0, size - 1),
        CLAMP_MIRROR_ONCE_LAST_TEXEL => mirror_once(index).min(size - 1),
        CLAMP_MIRROR_ONCE_BORDER => {
            let index = mirror_once(index);
            if index < size {
                index
            } else {
                return None;
            }
        }
        _ if (0..size).contains(&index) => index,
        _ => return None,
    };
    Some(found as u64)
}

fn mirror_once(index: i64) -> i64 {
    if index < 0 { -1 - index } else { index }
}

/// The coordinate in texels: scaled by the size unless unnormalized.
fn texel_coordinate(bits: u32, size: u32, unnormalized: bool) -> f32 {
    let value = f32::from_bits(bits);
    if unnormalized {
        value
    } else {
        value * size as f32
    }
}

/// The point-sampled texel index on one axis.
pub fn texel_index(
    bits: u32,
    size: u32,
    unnormalized: bool,
    mode: u32,
    offset: i64,
) -> Option<u64> {
    let value = texel_coordinate(bits, size, unnormalized);
    if value.is_nan() {
        return clamp_index(offset, size, mode);
    }
    let index = value.clamp(INT_MIN, INT_MAX).floor() as i64 + offset;
    clamp_index(index, size, mode)
}

/// (i0, i1, frac) on one axis for bilinear filtering and gather4; NaN gives (offset, offset, 0).
pub fn bilinear_axis(bits: u32, size: u32, unnormalized: bool, offset: i64) -> (i64, i64, f32) {
    let value = texel_coordinate(bits, size, unnormalized);
    if value.is_nan() {
        return (offset, offset, 0.0);
    }
    let clamped = (value - 0.5).clamp(INT_MIN, INT_MAX);
    let floor = clamped.floor();
    let first = floor as i64 + offset;
    (first, first + 1, clamped - floor)
}

fn lerp_bits(a_bits: u32, b_bits: u32, frac: f32) -> u32 {
    let a = f32::from_bits(a_bits);
    let b = f32::from_bits(b_bits);
    (a + (b - a) * frac).to_bits()
}

fn bilerp_bits(corners: [u32; 4], fx: f32, fy: f32) -> u32 {
    let [c00, c10, c01, c11] = corners;
    lerp_bits(lerp_bits(c00, c10, fx), lerp_bits(c01, c11, fx), fy)
}

/// 1.0 when `z <op> texel` passes (D3D12 SampleCmp), else 0.
fn compare_depth(z_bits: u32, texel_bits: u32, function: u32) -> u32 {
    let (z, t) = (f32::from_bits(z_bits), f32::from_bits(texel_bits));
    let passed = match function {
        0 => false,
        1 => z < t,
        2 => z == t,
        3 => z <= t,
        4 => z > t,
        5 => z != t,
        6 => z >= t,
        _ => true,
    };
    if passed { ONE_BITS } else { 0 }
}

/// The sampling inputs of one lane.
#[derive(Debug, Clone, Copy)]
pub struct Coordinates {
    pub s: u32,
    pub t: u32,
    pub unnormalized: bool,
    pub offset: (i64, i64),
    pub lod: Option<u32>,
}

/// The offsets (x, y) of an _o form: signed 6-bit fields in bits 5-0 and 13-8 (RDNA 2 ISA, Table 8.9).
pub fn unpack_offset(word: u32) -> (i64, i64) {
    let field = |value: u32| {
        let low = i64::from(value & OFFSET_BITS);
        if value & OFFSET_SIGN != 0 {
            low - OFFSET_RANGE
        } else {
            low
        }
    };
    (field(word), field(word >> 8))
}

/// MIN_FILTER when the lod is above 0 (_l forms), MAG_FILTER otherwise: true when it is bilinear.
fn use_linear(sampler: &Sampler, lod: Option<u32>) -> bool {
    match lod {
        Some(bits) if f32::from_bits(bits) > 0.0 => sampler.min_filter == FILTER_LINEAR,
        _ => sampler.mag_filter == FILTER_LINEAR,
    }
}

/// The four bilinear corners (x0 y0, x1 y0, x0 y1, x1 y1) after addressing, and the fractions.
type Corners = ([(Option<u64>, Option<u64>); 4], f32, f32);

fn corners(image: &Image, sampler: &Sampler, at: &Coordinates) -> Corners {
    let (x0, x1, fx) = bilinear_axis(at.s, image.width, at.unnormalized, at.offset.0);
    let (y0, y1, fy) = bilinear_axis(at.t, image.height, at.unnormalized, at.offset.1);
    let cx0 = clamp_index(x0, image.width, sampler.clamp_x);
    let cx1 = clamp_index(x1, image.width, sampler.clamp_x);
    let cy0 = clamp_index(y0, image.height, sampler.clamp_y);
    let cy1 = clamp_index(y1, image.height, sampler.clamp_y);
    ([(cx0, cy0), (cx1, cy0), (cx0, cy1), (cx1, cy1)], fx, fy)
}

fn point(image: &Image, sampler: &Sampler, at: &Coordinates) -> (Option<u64>, Option<u64>) {
    let x = texel_index(
        at.s,
        image.width,
        at.unnormalized,
        sampler.clamp_x,
        at.offset.0,
    );
    let y = texel_index(
        at.t,
        image.height,
        at.unnormalized,
        sampler.clamp_y,
        at.offset.1,
    );
    (x, y)
}

/// image_sample: the filtered texel, after the swizzle of the T#.
pub fn sample_color(
    memory: &Memory,
    image: &Image,
    sampler: &Sampler,
    at: &Coordinates,
) -> [u32; 4] {
    let border = sampler.border_color;
    let raw = if use_linear(sampler, at.lod) {
        let (spots, fx, fy) = corners(image, sampler, at);
        let texels = spots.map(|(x, y)| fetch_texel(memory, image, x, y, border));
        std::array::from_fn(|k| bilerp_bits(texels.map(|texel| texel[k]), fx, fy))
    } else {
        let (x, y) = point(image, sampler, at);
        fetch_texel(memory, image, x, y, border)
    };
    swizzled(&raw, &image.swizzle)
}

/// image_sample_c: the comparison of z with the red channel, filtered (percentage closer).
pub fn sample_compare(
    memory: &Memory,
    image: &Image,
    sampler: &Sampler,
    z: u32,
    at: &Coordinates,
) -> [u32; 4] {
    let border = sampler.border_color;
    let function = sampler.depth_compare;
    let result = if use_linear(sampler, at.lod) {
        let (spots, fx, fy) = corners(image, sampler, at);
        let passed = spots
            .map(|(x, y)| compare_depth(z, fetch_texel(memory, image, x, y, border)[0], function));
        bilerp_bits(passed, fx, fy)
    } else {
        let (x, y) = point(image, sampler, at);
        compare_depth(z, fetch_texel(memory, image, x, y, border)[0], function)
    };
    swizzled(&[result, result, result, ONE_BITS], &image.swizzle)
}

/// image_gather4: one channel (the single DMASK bit, through the swizzle) of the four bilinear texels,
/// in the order (x0, y1), (x1, y1), (x1, y0), (x0, y0).
pub fn gather4(
    memory: &Memory,
    image: &Image,
    sampler: &Sampler,
    dmask: u32,
    at: &Coordinates,
) -> [u32; 4] {
    let (spots, _, _) = corners(image, sampler, at);
    let [c00, c10, c01, c11] = spots;
    let channel = (0..4)
        .find(|channel| dmask >> channel & 1 == 1)
        .unwrap_or(0);
    let selector = image.swizzle[channel];
    [c01, c11, c10, c00].map(|(x, y)| {
        select(
            &fetch_texel(memory, image, x, y, sampler.border_color),
            selector,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addressing_modes_fold_indices_like_the_python() {
        assert_eq!(clamp_index(-1, 4, CLAMP_WRAP), Some(3));
        assert_eq!(clamp_index(5, 4, CLAMP_MIRROR), Some(2));
        assert_eq!(clamp_index(-1, 4, CLAMP_MIRROR), Some(0));
        assert_eq!(clamp_index(9, 4, CLAMP_LAST_TEXEL), Some(3));
        assert_eq!(clamp_index(-3, 4, CLAMP_MIRROR_ONCE_LAST_TEXEL), Some(2));
        assert_eq!(clamp_index(-5, 4, CLAMP_MIRROR_ONCE_BORDER), None);
        assert_eq!(clamp_index(4, 4, 6), None);
        assert_eq!(clamp_index(3, 4, 6), Some(3));
    }

    #[test]
    fn coordinates_scale_floor_and_split() {
        assert_eq!(
            texel_index(0.5f32.to_bits(), 4, false, CLAMP_WRAP, 0),
            Some(2)
        );
        assert_eq!(
            texel_index(f32::NAN.to_bits(), 4, false, CLAMP_WRAP, -1),
            Some(3)
        );
        assert_eq!(bilinear_axis(0.25f32.to_bits(), 4, false, 0), (0, 1, 0.5));
        assert_eq!(bilinear_axis(f32::NAN.to_bits(), 4, false, 2), (2, 2, 0.0));
        assert_eq!(unpack_offset(0x3F | 0x01 << 8), (-1, 1));
    }

    #[test]
    fn depth_comparisons_follow_the_function() {
        let (one, two) = (1.0f32.to_bits(), 2.0f32.to_bits());
        assert_eq!(compare_depth(one, two, 1), ONE_BITS);
        assert_eq!(compare_depth(two, one, 1), 0);
        assert_eq!(compare_depth(f32::NAN.to_bits(), one, 5), ONE_BITS);
        assert_eq!(lerp_bits(0, ONE_BITS, 0.25), 0.25f32.to_bits());
    }
}
