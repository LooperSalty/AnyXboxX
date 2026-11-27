//! Texel reads and writes of the MIMG instructions: port of `texel`, `fetch_texel`, `swizzled`,
//! `_select`, `float_to_unorm8` and `_store_texel` of `emu/rdna2_exec_image.py`.
//!
//! A texel gives four channels (float32 bits): the 8-bit UNORM channels are c / 255 rounded to the
//! nearest float32, the missing ones are 0 and alpha 1.0. A UNORM 8-bit write is
//! floor(v * 255 + 0.5) in float32 (D3D12 Functional Spec 3.2.3.6), 0 for NaN and v <= 0, 255 for
//! v >= 1. The texel at (x, y) is at address + (y x width + x) x texel size.

use crate::format_convert::unorm_bits;
use crate::image_desc::{
    FORMAT_8_8_8_8_UNORM, FORMAT_8_8_UNORM, FORMAT_32_32_32_32_FLOAT, FORMAT_32_32_FLOAT,
    FORMAT_32_32_SINT, FORMAT_32_32_UINT, FORMAT_32_FLOAT, FORMAT_32_SINT, FORMAT_32_UINT, Image,
    texel_bytes,
};
use crate::memory::{Memory, le_words};

pub const ONE_BITS: u32 = 0x3F80_0000;
const SELECT_ONE: u32 = 1;
const SELECT_X: u32 = 4;
const DWORD: u64 = 4;
const UNORM8_WIDTH: u32 = 8;
/// The border colours (transparent black, opaque black, opaque white).
const BORDER_COLORS: [[u32; 4]; 3] = [
    [0, 0, 0, 0],
    [0, 0, 0, ONE_BITS],
    [ONE_BITS, ONE_BITS, ONE_BITS, ONE_BITS],
];

fn unorm8(byte: u8) -> u32 {
    unorm_bits(u64::from(byte), UNORM8_WIDTH)
}

/// The address of the texel (x, y).
fn texel_address(image: &Image, x: u64, y: u64) -> u64 {
    let size = texel_bytes(image.format).unwrap_or(0);
    image.address.wrapping_add(
        y.wrapping_mul(u64::from(image.width))
            .wrapping_add(x)
            .wrapping_mul(size),
    )
}

/// The four channels (float32 bits) of the texel (x, y).
pub fn texel(memory: &Memory, image: &Image, x: u64, y: u64) -> [u32; 4] {
    let address = texel_address(image, x, y);
    let words = |count: usize| le_words(&memory.read(address, count * DWORD as usize));
    match image.format {
        FORMAT_32_32_32_32_FLOAT => {
            let w = words(4);
            [w[0], w[1], w[2], w[3]]
        }
        FORMAT_32_32_FLOAT => {
            let w = words(2);
            [w[0], w[1], 0, ONE_BITS]
        }
        FORMAT_32_32_UINT | FORMAT_32_32_SINT => {
            let w = words(2);
            [w[0], w[1], 0, 1]
        }
        FORMAT_32_FLOAT => [words(1)[0], 0, 0, ONE_BITS],
        FORMAT_32_UINT | FORMAT_32_SINT => [words(1)[0], 0, 0, 1],
        FORMAT_8_8_8_8_UNORM => {
            let raw = memory.read(address, 4);
            [
                unorm8(raw[0]),
                unorm8(raw[1]),
                unorm8(raw[2]),
                unorm8(raw[3]),
            ]
        }
        FORMAT_8_8_UNORM => {
            let raw = memory.read(address, 2);
            [unorm8(raw[0]), unorm8(raw[1]), 0, ONE_BITS]
        }
        _ => [unorm8(memory.read(address, 1)[0]), 0, 0, ONE_BITS],
    }
}

/// The texel (x, y), or the border colour when an index is outside the image (`None`).
pub fn fetch_texel(
    memory: &Memory,
    image: &Image,
    x: Option<u64>,
    y: Option<u64>,
    border: u32,
) -> [u32; 4] {
    match (x, y) {
        (Some(x), Some(y)) => texel(memory, image, x, y),
        _ => BORDER_COLORS[border as usize],
    }
}

/// One channel chosen by a selector: X to W, 1 (1.0) or 0 (the other values).
pub fn select(channels: &[u32; 4], selector: u32) -> u32 {
    if selector >= SELECT_X {
        channels[(selector - SELECT_X) as usize]
    } else if selector == SELECT_ONE {
        ONE_BITS
    } else {
        0
    }
}

/// The channels after the swizzle of the T#.
pub fn swizzled(channels: &[u32; 4], swizzle: &[u32; 4]) -> [u32; 4] {
    swizzle.map(|selector| select(channels, selector))
}

/// A float32 (bits) as an 8-bit UNORM integer.
pub fn float_to_unorm8(bits: u32) -> u8 {
    let value = f32::from_bits(bits);
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    if value >= 1.0 {
        return 255;
    }
    let scaled = value * 255.0;
    (scaled + 0.5).floor() as u8
}

/// The channels of DMASK filled, in order, from `values`; the others are 0.
fn unpack_dmask(dmask: u32, values: &[u32]) -> [u32; 4] {
    let mut packed = [0; 4];
    let mut next = values.iter();
    for (channel, slot) in packed.iter_mut().enumerate() {
        if dmask >> channel & 1 == 1 {
            *slot = next.next().copied().unwrap_or(0);
        }
    }
    packed
}

/// Writes the channels of DMASK of the texel (x, y); nothing outside the image or with DMASK 0.
pub fn store_texel(
    memory: &mut Memory,
    image: &Image,
    (x, y): (u32, u32),
    dmask: u32,
    values: &[u32],
) {
    if x >= image.width || y >= image.height || dmask == 0 {
        return;
    }
    let address = texel_address(image, u64::from(x), u64::from(y));
    let packed = unpack_dmask(dmask, values);
    let channels = match image.format {
        FORMAT_32_32_32_32_FLOAT => 4,
        FORMAT_32_32_FLOAT | FORMAT_32_32_UINT | FORMAT_32_32_SINT => 2,
        FORMAT_32_FLOAT | FORMAT_32_UINT | FORMAT_32_SINT => 1,
        _ => 0,
    };
    if channels != 0 {
        for (channel, value) in packed.iter().enumerate().take(channels) {
            if dmask >> channel & 1 == 1 {
                memory.write_words(address + channel as u64 * DWORD, &[*value]);
            }
        }
        return;
    }
    let size = texel_bytes(image.format).unwrap_or(1) as usize;
    if size == 1 && dmask & 1 == 0 {
        return; // 8_UNORM: nothing written (and no page created) without the red channel
    }
    let mut raw = if image.format == FORMAT_8_8_8_8_UNORM && dmask == 0xF {
        vec![0; size]
    } else {
        memory.read(address, size)
    };
    for (channel, byte) in raw.iter_mut().enumerate() {
        if dmask >> channel & 1 == 1 {
            *byte = float_to_unorm8(packed[channel]);
        }
    }
    memory.write(address, &raw);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unorm8_writes_round_half_up_and_saturate() {
        assert_eq!(float_to_unorm8(0.5f32.to_bits()), 128);
        assert_eq!(float_to_unorm8(f32::NAN.to_bits()), 0);
        assert_eq!(float_to_unorm8((-1.0f32).to_bits()), 0);
        assert_eq!(float_to_unorm8(2.0f32.to_bits()), 255);
        assert_eq!(unorm8(255), ONE_BITS);
    }

    #[test]
    fn selectors_pick_channels_one_or_zero() {
        let channels = [1, 2, 3, 4];
        assert_eq!(swizzled(&channels, &[7, 6, 1, 0]), [4, 3, ONE_BITS, 0]);
        assert_eq!(select(&channels, 2), 0);
    }

    #[test]
    fn partial_unorm_stores_keep_the_other_channels() {
        let image = Image {
            address: 0x100,
            format: FORMAT_8_8_8_8_UNORM,
            width: 2,
            height: 2,
            swizzle: [4, 5, 6, 7],
        };
        let mut memory = Memory::default();
        memory.write(0x104, &[9, 9, 9, 9]);
        store_texel(&mut memory, &image, (1, 0), 0b0101, &[ONE_BITS, 0]);
        assert_eq!(memory.read(0x104, 4), vec![255, 9, 0, 9]);
        store_texel(&mut memory, &image, (2, 0), 0xF, &[ONE_BITS; 4]);
        assert_eq!(memory.read(0x108, 4), vec![0; 4]);
    }
}
