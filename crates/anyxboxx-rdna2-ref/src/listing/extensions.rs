use super::{Field, OUTPUT, bits_list, decorate, flag, join, render, source};

fn control(value: u64) -> Option<String> {
    Some(match value {
        0..=255 => format!(
            "quad_perm:[{}]",
            bits_list((0..4).map(|i| value >> (2 * i) & 3))
        ),
        0x101..=0x10f => format!("row_shl:{}", value & 15),
        0x111..=0x11f => format!("row_shr:{}", value & 15),
        0x121..=0x12f => format!("row_ror:{}", value & 15),
        0x140 => "row_mirror".into(),
        0x141 => "row_half_mirror".into(),
        0x150..=0x15f => format!("row_share:{}", value & 15),
        0x160..=0x16f => format!("row_xmask:{}", value & 15),
        _ => return None,
    })
}

pub(super) fn text(
    family: &str,
    name: &str,
    word: u64,
    fields: &[Field],
) -> Result<Option<String>, String> {
    let kind = word & 511;
    let ext = word >> 32;
    let sdwa = kind == 0xf9;
    if !sdwa && family == "VOPC" {
        return Ok(None);
    }
    let mut flags = Vec::new();
    if sdwa {
        let selections = [
            "BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD",
        ];
        let reserved = || format!("{name}_sdwa : sélecteur réservé");
        let select = |shift: u32| {
            selections
                .get((ext >> shift & 7) as usize)
                .ok_or_else(reserved)
        };
        if family != "VOPC" {
            if flag(ext, 13) {
                flags.push("clamp".into());
            }
            if ext >> 14 & 3 != 0 {
                flags.push(OUTPUT[(ext >> 14 & 3) as usize].into());
            }
            flags.push(format!("dst_sel:{}", select(8)?));
            flags.push(format!(
                "dst_unused:{}",
                ["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE"]
                    .get((ext >> 11 & 3) as usize)
                    .ok_or_else(reserved)?
            ));
        }
        flags.push(format!("src0_sel:{}", select(16)?));
        if fields.iter().any(|f| f.name == "vsrc1") {
            flags.push(format!("src1_sel:{}", select(24)?));
        }
    } else if kind == 0xfa {
        let Some(ctrl) = control(ext >> 8 & 511) else {
            return Ok(None);
        };
        flags.push(ctrl);
        flags.push(format!("row_mask:0x{:x}", ext >> 28 & 15));
        flags.push(format!("bank_mask:0x{:x}", ext >> 24 & 15));
        if flag(ext, 19) {
            flags.push("bound_ctrl:1".into());
        }
        if flag(ext, 18) {
            flags.push("fi:1".into());
        }
    } else {
        flags.push(format!(
            "dpp8:[{}]",
            bits_list((0..8).map(|i| ext >> (8 + 3 * i) & 7))
        ));
        if kind == 0xea {
            flags.push("fi:1".into());
        }
    }
    let operands = fields
        .iter()
        .enumerate()
        .map(|(pos, &f)| {
            if matches!(f.name, "src0" | "vsrc1") {
                let second = f.name == "vsrc1";
                let shift = if second { 8 } else { 0 };
                let reg = if second { word >> 9 & 255 } else { ext & 255 };
                let scalar = sdwa && flag(ext, 23 + shift);
                let mut s = source(if scalar { reg } else { reg + 256 }, f.width, None);
                if sdwa || kind == 0xfa {
                    let neg = 20
                        + if sdwa {
                            shift
                        } else if second {
                            2
                        } else {
                            0
                        };
                    s = decorate(s, flag(ext, neg), flag(ext, neg + 1));
                    if sdwa && flag(ext, 19 + shift) {
                        s = format!("sext({s})");
                    }
                }
                s
            } else if family == "VOPC" && pos == 0 && f.name == "=" && sdwa && flag(ext, 15) {
                source(ext >> 8 & 127, 1, None)
            } else {
                render(f, family, word, None)
            }
        })
        .collect();
    Ok(Some(join(
        &format!("{name}_{}", if sdwa { "sdwa" } else { "dpp" }),
        operands,
        flags,
    )))
}
