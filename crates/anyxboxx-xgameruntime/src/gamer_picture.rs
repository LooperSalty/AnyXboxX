//! The gamer picture of the offline profile: a generated RGB PNG of the requested size (C++
//! `gamer_picture.cpp`). The PNG uses stored (uncompressed) zlib blocks, with CRC-32 per chunk.

/// Sides of XUserGamerPictureSize (Small, Medium, Large, ExtraLarge), in pixels.
const SIDES: [u32; 4] = [64, 208, 424, 1080];
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
/// Xbox green background and light disc.
const BACKGROUND: [u8; 3] = [0x10, 0x7C, 0x10];
const DISC: [u8; 3] = [0xE6, 0xE6, 0xE6];
const CHANNELS: usize = 3;
const STORED_BLOCK_MAX: usize = 65535;
const ADLER_MODULO: u32 = 65521;

/// The side in pixels of a size code, or `None` for an unknown code.
pub fn side(size_code: u32) -> Option<u32> {
    SIDES.get(size_code as usize).copied()
}

/// CRC-32 (ISO 3309, as PNG uses it) of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Adler-32 of `data` (the zlib trailer).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % ADLER_MODULO;
        b = (b + a) % ADLER_MODULO;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

/// The filtered scanlines (filter byte 0): a disc centred on a flat background.
fn scanlines(side: u32) -> Vec<u8> {
    let side_len = side as usize;
    let mut raw = Vec::with_capacity(side_len * (1 + CHANNELS * side_len));
    let centre = i64::from(side / 2);
    let radius = i64::from(side) * 3 / 10;
    for y in 0..i64::from(side) {
        raw.push(0);
        for x in 0..i64::from(side) {
            let (dx, dy) = (x - centre, y - centre);
            raw.extend(if dx * dx + dy * dy <= radius * radius {
                DISC
            } else {
                BACKGROUND
            });
        }
    }
    raw
}

/// A zlib stream of `raw` made of stored blocks.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut blocks = raw.chunks(STORED_BLOCK_MAX).peekable();
    if blocks.peek().is_none() {
        out.extend([1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(block);
    }
    out.extend(adler32(raw).to_be_bytes());
    out
}

/// An RGB PNG of `side` × `side` pixels.
pub fn png(side: u32) -> Vec<u8> {
    let mut header = Vec::with_capacity(13);
    header.extend(side.to_be_bytes());
    header.extend(side.to_be_bytes());
    // 8 bits, RGB, deflate, filter 0, no interlacing.
    header.extend([8, 2, 0, 0, 0]);
    let mut out = SIGNATURE.to_vec();
    push_chunk(&mut out, b"IHDR", &header);
    push_chunk(&mut out, b"IDAT", &zlib_stored(&scanlines(side)));
    push_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_codes_map_to_the_four_sides() {
        assert_eq!(side(0), Some(64));
        assert_eq!(side(3), Some(1080));
        assert_eq!(side(4), None);
    }

    #[test]
    fn the_png_has_the_signature_and_the_checksums_of_its_chunks() {
        let image = png(8);
        assert_eq!(&image[..8], &SIGNATURE);
        // IHDR: length 13, then 8 × 8, 8 bits, RGB.
        assert_eq!(&image[8..12], &13u32.to_be_bytes());
        assert_eq!(&image[16..24], &[0, 0, 0, 8, 0, 0, 0, 8]);
        assert_eq!(image[24..26], [8, 2]);
        // The IEND chunk is last, with the standard CRC.
        assert_eq!(
            &image[image.len() - 8..],
            &[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
        );
    }

    #[test]
    fn the_zlib_stream_round_trips_the_scanlines() {
        let raw = scanlines(3);
        let stream = zlib_stored(&raw);
        assert_eq!(&stream[..2], &[0x78, 0x01]);
        assert_eq!(&stream[stream.len() - 4..], &adler32(&raw).to_be_bytes());
    }
}
