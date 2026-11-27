use super::{Field, bits_list, flag, range, source};
use crate::format_table::FORMAT_NAMES;

pub(super) fn operand(kind: &str, family: &str, word: u64, value: u64, width: u64) -> String {
    let saddr = word >> 48 & 127;
    match kind {
        "smem_offset" => {
            let offset = ((value as i32) << 11) >> 11;
            let register = word >> 57 & 127;
            let hex = format!(
                "{}0x{:x}",
                if offset < 0 { "-" } else { "" },
                offset.unsigned_abs()
            );
            if register == 125 {
                return hex;
            }
            let reg = source(register, 1, None);
            if offset == 0 {
                reg
            } else {
                format!("{reg} offset:{hex}")
            }
        }
        "buf_vdata" => range("v", value, width + (word >> 55 & 1)),
        "buf_vaddr" => match (word >> 12 & 3).count_ones() {
            0 => "off".into(),
            n => range("v", value, u64::from(n)),
        },
        "rsrc" => source(value * 4, width, None),
        "flat_vaddr" if family == "SCRATCH" && saddr != 125 => "off".into(),
        "flat_vaddr" => range(
            "v",
            value,
            if family == "FLAT" || (family == "GLOBAL" && saddr == 125) {
                2
            } else {
                1
            },
        ),
        "flat_saddr" if value == 125 || (family == "SCRATCH" && value == 127) => "off".into(),
        "flat_saddr" => source(value, if family == "GLOBAL" { 2 } else { 1 }, None),
        _ => unreachable!("unknown generated operand kind {kind}"),
    }
}

pub(super) fn adjust(family: &str, name: &str, word: u64, fields: &[Field]) -> Vec<Field> {
    let mut fields = fields.to_vec();
    if family == "MUBUF" && flag(word, 16) {
        fields.retain(|f| f.name != "vdata");
    }
    if matches!(family, "FLAT" | "GLOBAL" | "SCRATCH") {
        if flag(word, 13) || name.contains("_atomic_") {
            fields.retain(|f| f.name != "vdst");
        }
        if !flag(word, 13) && name.contains("_atomic_") && flag(word, 16) {
            let width = fields
                .iter()
                .find(|f| f.name == "vdata")
                .map_or(1, |f| f.width);
            let width = if name.contains("cmpswap") && width > 1 {
                width / 2
            } else {
                width
            };
            fields.insert(0, Field::new("vdst", "vgpr", 56, 255, width, ""));
        }
    }
    fields
}

fn swizzle(imm: u64) -> String {
    if imm >= 0xe000 {
        return format!("swizzle(FFT,{})", imm & 31);
    }
    if imm >= 0xc000 {
        return format!("swizzle(ROTATE,{},{})", imm >> 10 & 1, imm >> 5 & 31);
    }
    if imm & 0xff00 == 0x8000 {
        return format!(
            "swizzle(QUAD_PERM,{})",
            bits_list((0..4).map(|i| imm >> (2 * i) & 3))
        );
    }
    if imm & 0x8000 != 0 {
        return imm.to_string();
    }
    let (and, or, xor) = (imm & 31, imm >> 5 & 31, imm >> 10 & 31);
    if and == 31 && or == 0 {
        if xor.is_power_of_two() {
            return format!("swizzle(SWAP,{xor})");
        }
        if xor != 0 && (xor + 1).is_power_of_two() {
            return format!("swizzle(REVERSE,{})", xor + 1);
        }
    }
    let group = 32 - and;
    if group > 1 && group.is_power_of_two() && or < group && xor == 0 {
        return format!("swizzle(BROADCAST,{group},{or})");
    }
    let chars: String = (0..5)
        .rev()
        .map(
            |i| match (((or ^ xor) >> i) & 1, (((and | or) ^ xor) >> i) & 1) {
                (0, 0) => '0',
                (1, 1) => '1',
                (0, 1) => 'p',
                _ => 'i',
            },
        )
        .collect();
    format!("swizzle(BITMASK_PERM,\"{chars}\")")
}

pub(super) fn flags(word: u64, names: &[(u32, &str)]) -> Vec<String> {
    names
        .iter()
        .filter(|(bit, _)| flag(word, *bit))
        .map(|(_, s)| s.to_string())
        .collect()
}

pub(super) fn suffix(family: &str, name: &str, word: u64) -> Vec<String> {
    let mut parts = Vec::new();
    match family {
        "MUBUF" | "MTBUF" => {
            if family == "MTBUF" {
                let id = (word >> 19 & 127) as usize;
                match id {
                    1 => (),
                    0..=77 => parts.push(format!(
                        "format:[BUF_FMT_{}]",
                        FORMAT_NAMES[id].1.trim_start_matches("GFX10_FORMAT_")
                    )),
                    _ => parts.push(format!("format:{id}")),
                }
            }
            parts.extend(flags(word, &[(13, "idxen"), (12, "offen")]));
            if word & 4095 != 0 {
                parts.push(format!("offset:{}", word & 4095));
            }
            parts.extend(flags(word, &[(14, "glc"), (54, "slc"), (15, "dlc")]));
            if family == "MUBUF" && flag(word, 16) {
                parts.push("lds".into());
            }
            if flag(word, 55) {
                parts.push("tfe".into());
            }
        }
        "FLAT" | "GLOBAL" | "SCRATCH" => {
            let mut offset = (word & 4095) as i32;
            if family != "FLAT" && offset >= 2048 {
                offset -= 4096;
            }
            if offset != 0 {
                parts.push(format!("offset:{offset}"));
            }
            parts.extend(flags(
                word,
                &[(16, "glc"), (17, "slc"), (12, "dlc"), (13, "lds")],
            ));
        }
        "DS" => {
            if ["ds_read2", "ds_write2", "ds_wrxchg2"]
                .iter()
                .any(|prefix| {
                    name.strip_prefix(prefix)
                        .is_some_and(|s| s.starts_with('_') || s.starts_with("st64_"))
                })
            {
                for i in 0..2 {
                    let v = word >> (8 * i) & 255;
                    if v != 0 {
                        parts.push(format!("offset{i}:{v}"));
                    }
                }
            } else if word & 65535 != 0 {
                let v = word & 65535;
                parts.push(format!(
                    "offset:{}",
                    if name == "ds_swizzle_b32" {
                        swizzle(v)
                    } else {
                        v.to_string()
                    }
                ));
            }
            parts.extend(flags(word, &[(17, "gds")]));
        }
        "SMEM" => parts.extend(flags(word, &[(16, "glc"), (14, "dlc")])),
        _ => (),
    }
    parts
}
