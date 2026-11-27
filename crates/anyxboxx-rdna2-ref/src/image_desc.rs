//! MIMG instruction fields and the T# (image) and S# (sampler) descriptors. Port of
//! `emu/rdna2_image.py`: what is supported and what is refused, with the same rules.
//!
//! MIMG gfx10 (MIMGe_gfx10 of LLVM): bits 1-2 NSA (extra address words), 3-5 DIM, 8-11 DMASK, 12 UNORM,
//! 15 R128, 16 TFE, 17 LWE, 32-39 VADDR, 40-47 VDATA, 48-52 SRSRC and 53-57 SSAMP (in units of four
//! SGPRs), 62 A16, 63 D16; the NSA words hold the other address VGPRs, one byte each.
//! Supported: a 2D linear image of one mip level (BASE_LEVEL = LAST_LEVEL = 0, rows of width x texel
//! size) in 8_UNORM, 8_8_UNORM, 8_8_8_8_UNORM, 32_FLOAT, 32_32_FLOAT or 32_32_32_32_FLOAT; image_load
//! [_mip], image_store[_mip], image_get_resinfo; image_sample[_lz|_l][_o], image_sample_c[_lz|_l][_o]
//! and image_gather4[_lz|_l][_o], point or bilinear, with the wrap, mirror, clamp-to-edge,
//! mirror-once and border addressing modes and the standard border colours. The rest is refused.

use crate::buffer_format::format_name;
use crate::error::Error;

pub const FORMAT_8_UNORM: u32 = 1;
pub const FORMAT_8_8_UNORM: u32 = 14;
pub const FORMAT_32_UINT: u32 = 20;
pub const FORMAT_32_SINT: u32 = 21;
pub const FORMAT_32_FLOAT: u32 = 22;
pub const FORMAT_8_8_8_8_UNORM: u32 = 56;
pub const FORMAT_32_32_UINT: u32 = 62;
pub const FORMAT_32_32_SINT: u32 = 63;
pub const FORMAT_32_32_FLOAT: u32 = 64;
pub const FORMAT_32_32_32_32_FLOAT: u32 = 77;
const TYPE_2D: u32 = 9;
const DIM_2D: u32 = 1;
pub const CLAMP_WRAP: u32 = 0;
pub const CLAMP_MIRROR: u32 = 1;
pub const CLAMP_LAST_TEXEL: u32 = 2;
pub const CLAMP_MIRROR_ONCE_LAST_TEXEL: u32 = 3;
pub const CLAMP_MIRROR_ONCE_BORDER: u32 = 7;
const SUPPORTED_CLAMPS: [u32; 6] = [0, 1, 2, 3, 6, 7];
pub const FILTER_LINEAR: u32 = 1;
const SUPPORTED_BORDERS: u32 = 3;
const SAMPLER_SUFFIXES: [&str; 6] = ["", "_lz", "_l", "_o", "_lz_o", "_l_o"];
const DIRECT_OPS: [&str; 5] = [
    "image_load",
    "image_load_mip",
    "image_store",
    "image_store_mip",
    "image_get_resinfo",
];
const ATOMIC_OPS: [&str; 16] = [
    "image_atomic_swap",
    "image_atomic_cmpswap",
    "image_atomic_add",
    "image_atomic_sub",
    "image_atomic_smin",
    "image_atomic_umin",
    "image_atomic_smax",
    "image_atomic_umax",
    "image_atomic_and",
    "image_atomic_or",
    "image_atomic_xor",
    "image_atomic_inc",
    "image_atomic_dec",
    "image_atomic_fcmpswap",
    "image_atomic_fmin",
    "image_atomic_fmax",
];
const ATOMIC_CMPSWAP_OPS: [&str; 2] = ["image_atomic_cmpswap", "image_atomic_fcmpswap"];

/// The kind of a supported MIMG instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Direct,
    Sample,
    Compare,
    Gather,
    Atomic,
}

/// The kind of `name`, or `None` when the interpreter does not run it.
pub fn op(name: &str) -> Option<Op> {
    if DIRECT_OPS.contains(&name) {
        return Some(Op::Direct);
    }
    if ATOMIC_OPS.contains(&name) {
        return Some(Op::Atomic);
    }
    [
        ("image_sample_c", Op::Compare),
        ("image_sample", Op::Sample),
        ("image_gather4", Op::Gather),
    ]
    .iter()
    .find_map(|(prefix, op)| {
        name.strip_prefix(prefix)
            .filter(|rest| SAMPLER_SUFFIXES.contains(rest))
            .map(|_| *op)
    })
}

/// True for compare-and-swap image atomics (`image_atomic_cmpswap`, `image_atomic_fcmpswap`).
pub fn is_cmpswap(name: &str) -> bool {
    ATOMIC_CMPSWAP_OPS.contains(&name)
}

/// Width in bits (32 or 64) of an `image_atomic_*` from its DMASK; `Untranslated` otherwise.
pub fn atomic_width(name: &str, dmask: u32) -> Result<u32, Error> {
    if is_cmpswap(name) {
        match dmask {
            0x3 => Ok(32),
            0xF => Ok(64),
            _ => refuse(format!(
                "{name} : dmask 0x{dmask:x} non pris en charge (0x3 ou 0xf attendus)"
            )),
        }
    } else {
        match dmask {
            0x1 => Ok(32),
            0x3 => Ok(64),
            _ => refuse(format!(
                "{name} : dmask 0x{dmask:x} non pris en charge (0x1 ou 0x3 attendus)"
            )),
        }
    }
}

/// Whether `format` is a valid raw 32-bit or 64-bit format for an `image_atomic_*` of `width` bits.
pub fn atomic_format_ok(format: u32, width: u32) -> bool {
    match width {
        32 => matches!(format, FORMAT_32_UINT | FORMAT_32_SINT | FORMAT_32_FLOAT),
        64 => matches!(
            format,
            FORMAT_32_32_UINT | FORMAT_32_32_SINT | FORMAT_32_32_FLOAT
        ),
        _ => false,
    }
}

/// The fields of a MIMG instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub nsa: usize,
    pub dim: u32,
    pub dmask: u32,
    pub unorm: bool,
    pub glc: bool,
    flags: [(&'static str, bool); 5],
    /// VADDR, then the bytes of the NSA words.
    pub vaddr: Vec<usize>,
    pub vdata: usize,
    pub srsrc: usize,
    pub ssamp: usize,
}

/// The fields of the instruction `words` (its two dwords and the NSA words that follow).
pub fn instruction_fields(words: &[u32]) -> Fields {
    let low = words[0];
    let high = words[1];
    let bit = |word: u32, index: u32| word >> index & 1 == 1;
    let nsa = ((low >> 1) & 0x3) as usize;
    let mut vaddr = vec![(high & 0xFF) as usize];
    for k in 0..4 * nsa {
        let word = words.get(2 + k / 4).copied().unwrap_or(0);
        vaddr.push(((word >> (8 * (k % 4))) & 0xFF) as usize);
    }
    Fields {
        nsa,
        dim: (low >> 3) & 0x7,
        dmask: (low >> 8) & 0xF,
        unorm: bit(low, 12),
        glc: bit(low, 13),
        flags: [
            ("r128", bit(low, 15)),
            ("tfe", bit(low, 16)),
            ("lwe", bit(low, 17)),
            ("a16", bit(high, 30)),
            ("d16", bit(high, 31)),
        ],
        vaddr,
        vdata: ((high >> 8) & 0xFF) as usize,
        srsrc: ((high >> 16) & 0x1F) as usize * 4,
        ssamp: ((high >> 21) & 0x1F) as usize * 4,
    }
}

fn refuse<T>(message: String) -> Result<T, Error> {
    Err(Error::Untranslated(message))
}

/// The kind of `name`; `Untranslated` for the instructions and options the interpreter refuses.
pub fn check_instruction(name: &str, fields: &Fields) -> Result<Op, Error> {
    let Some(op) = op(name) else {
        return refuse(format!("instruction d'image non prise en charge : {name}"));
    };
    if fields.dim != DIM_2D {
        return refuse(format!(
            "{name} : dimension {} non prise en charge (2D seulement)",
            fields.dim
        ));
    }
    if let Some((flag, _)) = fields.flags.iter().find(|(_, set)| *set) {
        return refuse(format!("{name} : option {flag} non prise en charge"));
    }
    if op == Op::Gather && fields.dmask.count_ones() != 1 {
        return refuse(format!(
            "{name} : dmask 0x{:x} non pris en charge (un seul canal)",
            fields.dmask
        ));
    }
    if op == Op::Atomic {
        atomic_width(name, fields.dmask)?;
    }
    Ok(op)
}

fn suffix_has(name: &str, part: &str) -> bool {
    name.split('_').skip(2).any(|piece| piece == part)
}

/// The number of address VGPRs in 2D (RDNA 2 ISA, Table 8.9).
pub fn address_count(name: &str, op: Op) -> usize {
    match name {
        "image_get_resinfo" => 1,
        "image_load_mip" | "image_store_mip" => 3,
        _ if op == Op::Direct || op == Op::Atomic => 2,
        _ => {
            2 + usize::from(suffix_has(name, "o"))
                + usize::from(op == Op::Compare)
                + usize::from(suffix_has(name, "l"))
        }
    }
}

/// The address VGPRs: consecutive, or given by the NSA words.
pub fn address_registers(name: &str, op: Op, fields: &Fields) -> Vec<usize> {
    let count = address_count(name, op);
    if fields.nsa != 0 {
        return fields.vaddr.iter().take(count).copied().collect();
    }
    (0..count).map(|i| fields.vaddr[0] + i).collect()
}

/// The address VGPRs of a 2D sample or gather, in their order: [offset], [z], s, t, [lod].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplerAddresses {
    pub offset: Option<usize>,
    pub z: Option<usize>,
    pub s: usize,
    pub t: usize,
    pub lod: Option<usize>,
}

pub fn sampler_addresses(name: &str, op: Op, addrs: &[usize]) -> SamplerAddresses {
    let mut next = addrs.iter().copied();
    let offset = suffix_has(name, "o").then(|| next.next()).flatten();
    let z = (op == Op::Compare).then(|| next.next()).flatten();
    let s = next.next().unwrap_or(0);
    let t = next.next().unwrap_or(0);
    let lod = suffix_has(name, "l").then(|| next.next()).flatten();
    SamplerAddresses {
        offset,
        z,
        s,
        t,
        lod,
    }
}

fn get(word: u32, low: u32, width: u32) -> u32 {
    (word >> low) & ((1 << width) - 1)
}

/// The bytes of a texel of a supported format.
pub fn texel_bytes(format: u32) -> Option<u64> {
    match format {
        FORMAT_8_UNORM => Some(1),
        FORMAT_8_8_UNORM => Some(2),
        FORMAT_32_UINT | FORMAT_32_SINT | FORMAT_32_FLOAT | FORMAT_8_8_8_8_UNORM => Some(4),
        FORMAT_32_32_UINT | FORMAT_32_32_SINT | FORMAT_32_32_FLOAT => Some(8),
        FORMAT_32_32_32_32_FLOAT => Some(16),
        _ => None,
    }
}

/// A supported T#.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub address: u64,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub swizzle: [u32; 4],
}

/// The T# of 8 words; `Untranslated` outside the supported case.
pub fn image(words: &[u32]) -> Result<Image, Error> {
    let kind = get(words[3], 28, 4);
    let format = get(words[1], 20, 9);
    let base_level = get(words[3], 12, 4);
    let last_level = get(words[3], 16, 4);
    if kind != TYPE_2D {
        return refuse(format!(
            "T# de type {kind} non pris en charge (2D = {TYPE_2D})"
        ));
    }
    if texel_bytes(format).is_none() {
        return refuse(format!("format {} non pris en charge", format_name(format)));
    }
    if base_level != 0 || last_level != 0 {
        return refuse(format!(
            "T# : niveaux mip non pris en charge (base_level={base_level}, last_level={last_level}, niveau 0 seulement)"
        ));
    }
    Ok(Image {
        address: (u64::from(get(words[1], 0, 8)) << 32 | u64::from(words[0])) << 8,
        format,
        width: (get(words[1], 30, 2) | get(words[2], 0, 12) << 2) + 1,
        height: get(words[2], 14, 14) + 1,
        swizzle: std::array::from_fn(|i| get(words[3], 3 * i as u32, 3)),
    })
}

/// A supported S#.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    pub clamp_x: u32,
    pub clamp_y: u32,
    pub depth_compare: u32,
    pub unnormalized: bool,
    pub mag_filter: u32,
    pub min_filter: u32,
    pub border_color: u32,
}

/// The S# of 4 words; `Untranslated` outside the supported case.
pub fn sampler(words: &[u32]) -> Result<Sampler, Error> {
    let found = Sampler {
        clamp_x: get(words[0], 0, 3),
        clamp_y: get(words[0], 3, 3),
        depth_compare: get(words[0], 12, 3),
        unnormalized: get(words[0], 15, 1) == 1,
        mag_filter: get(words[2], 20, 2),
        min_filter: get(words[2], 22, 2),
        border_color: get(words[3], 30, 2),
    };
    for clamp in [found.clamp_x, found.clamp_y] {
        if !SUPPORTED_CLAMPS.contains(&clamp) {
            return refuse(format!("S# : adressage {clamp} non pris en charge"));
        }
    }
    if found.mag_filter > FILTER_LINEAR || found.min_filter > FILTER_LINEAR {
        return refuse(
            "S# : filtrage anisotrope non pris en charge (au plus proche ou bilinéaire seulement)"
                .into(),
        );
    }
    if found.border_color >= SUPPORTED_BORDERS {
        return refuse(format!(
            "S# : couleur de bordure {} non prise en charge (standard seulement)",
            found.border_color
        ));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_give_their_kind_and_address_count() {
        assert_eq!(op("image_sample_c_lz_o"), Some(Op::Compare));
        assert_eq!(op("image_gather4_l"), Some(Op::Gather));
        assert_eq!(op("image_sample_d"), None);
        assert_eq!(op("image_atomic_add"), Some(Op::Atomic));
        assert_eq!(op("image_atomic_fcmpswap"), Some(Op::Atomic));
        assert_eq!(address_count("image_atomic_add", Op::Atomic), 2);
        assert_eq!(atomic_width("image_atomic_add", 0x1).unwrap(), 32);
        assert_eq!(atomic_width("image_atomic_add", 0x3).unwrap(), 64);
        assert!(atomic_width("image_atomic_add", 0xF).is_err());
        assert_eq!(atomic_width("image_atomic_cmpswap", 0x3).unwrap(), 32);
        assert_eq!(atomic_width("image_atomic_fcmpswap", 0xF).unwrap(), 64);
        assert!(atomic_width("image_atomic_cmpswap", 0x1).is_err());
        assert!(atomic_format_ok(FORMAT_32_UINT, 32));
        assert!(atomic_format_ok(FORMAT_32_32_SINT, 64));
        assert!(!atomic_format_ok(FORMAT_8_8_8_8_UNORM, 32));
        assert_eq!(address_count("image_sample_c_l_o", Op::Compare), 5);
        assert_eq!(address_count("image_sample_lz", Op::Sample), 2);
        assert_eq!(address_count("image_load_mip", Op::Direct), 3);
        let addrs = sampler_addresses("image_sample_c_l_o", Op::Compare, &[1, 2, 3, 4, 5]);
        assert_eq!(
            (addrs.offset, addrs.z, addrs.s, addrs.t, addrs.lod),
            (Some(1), Some(2), 3, 4, Some(5))
        );
    }

    #[test]
    fn nsa_words_give_the_other_addresses() {
        let fields = instruction_fields(&[1 << 1 | 1 << 3 | 0xF << 8, 7, 0x0403_0209]);
        assert_eq!(fields.vaddr, vec![7, 9, 2, 3, 4]);
        assert_eq!(
            address_registers("image_sample", Op::Sample, &fields),
            vec![7, 9]
        );
    }

    #[test]
    fn unsupported_descriptors_are_refused() {
        let good = [0x100, 22 << 20 | 3 << 30, 1 | 7 << 14, 0xFAC | 9 << 28];
        let found = image(&good).unwrap();
        assert_eq!((found.address, found.width, found.height), (0x10000, 8, 8));
        assert!(image(&[0, 22 << 20, 0, 8 << 28]).is_err());
        assert!(image(&[0, 2 << 20, 0, 9 << 28]).is_err());
        assert!(image(&[0, 22 << 20, 0, 9 << 28 | 1 << 16]).is_err());
        assert!(sampler(&[4, 0, 0, 0]).is_err());
        assert!(sampler(&[0, 0, 2 << 20, 0]).is_err());
        assert!(sampler(&[0, 0, 0, 3 << 30]).is_err());
    }
}
