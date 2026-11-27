//! Wave64 uses both scalar mask words and a 64-thread groupshared exchange.
use crate::{scalar::source, scalar_compare::high};
use anyxboxx_rdna2_ref::{decode::Instruction, vop3::Fields};

fn operand(code: u32, literal: Option<u32>) -> Result<String, String> {
    if code >= 256 {
        Ok(format!("v[{}]", code - 256))
    } else {
        source(code, literal)
    }
}

fn scalar_target(code: u32) -> String {
    if code == 125 {
        "s_null".into()
    } else {
        format!("s[{code}]")
    }
}

fn bit(code: u32) -> Result<String, String> {
    Ok(format!(
        "Bit64({}, {}, lane)",
        source(code, None)?,
        high(code)?
    ))
}

fn lane_statement(
    name: &str,
    family: &str,
    word: u64,
    literal: Option<u32>,
) -> Result<Option<String>, String> {
    let f = Fields::new(word);
    let (d, a, b, mask) = if family == "VOP3" {
        (f.vdst, f.sources[0], f.sources[1], f.sources[2])
    } else {
        (
            (word >> 17) as u32 & 255,
            word as u32 & 511,
            256 + ((word >> 9) as u32 & 255),
            106,
        )
    };
    if !matches!(
        name,
        "v_readlane_b32"
            | "v_readfirstlane_b32"
            | "v_writelane_b32"
            | "v_cndmask_b32"
            | "v_mbcnt_lo_u32_b32"
            | "v_mbcnt_hi_u32_b32"
    ) {
        return Ok(None);
    }
    let (a, b) = (operand(a, literal)?, operand(b, literal)?);
    Ok(Some(match name {
        "v_readlane_b32" => format!("{} = ReadLane64(lane, {a}, {b});", scalar_target(d)),
        "v_readfirstlane_b32" => format!(
            "{} = ReadLane64(lane, {a}, FirstLane64(s[126], s[127]));",
            scalar_target(d)
        ),
        "v_writelane_b32" => format!("if (lane == ({b} & 63u)) v[{d}] = {a};"),
        "v_cndmask_b32" => {
            if family == "VOP3" && (f.clamp || (f.abs | f.neg) & 4 != 0) {
                return Err(format!("{name}: unsupported mask modifier or clamp"));
            }
            format!("if (LANE_ON) v[{d}] = {} ? {b} : {a};", bit(mask)?)
        }
        _ => {
            let below = if name.starts_with("v_mbcnt_lo") {
                "(lane < 32u ? (1u << lane) - 1u : 0xFFFFFFFFu)"
            } else {
                "(lane < 32u ? 0u : (1u << (lane - 32u)) - 1u)"
            };
            format!("if (LANE_ON) v[{d}] = countbits({a} & {below}) + {b};")
        }
    }))
}

pub fn statements(name: &str, inst: &Instruction, bytes: &[u8]) -> Result<Vec<String>, String> {
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let next = bytes
        .get(4..8)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let pair = first as u64 | ((next.unwrap_or(0) as u64) << 32);
    let family = inst.family;
    if family == "VOP3" && crate::carry::supports(name) {
        let literal = bytes
            .get(8..12)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        return Ok(vec![crate::carry::statement(name, pair, literal, 64)?]);
    }
    if family == "VOP3" && name.starts_with("v_cmp") {
        let literal = bytes
            .get(8..12)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        return Ok(vec![crate::vector_compare::vop3(name, pair, literal, 64)?]);
    }
    if family == "VOPC" && matches!(first & 511, 0xe9 | 0xea | 0xf9 | 0xfa) {
        return Err(format!(
            "{name}: wave64 lane exchange in an extended comparison"
        ));
    }
    if matches!(family, "VOP3" | "VOP1" | "VOP2") {
        let literal = if family == "VOP3" {
            bytes
                .get(8..12)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        } else {
            next
        };
        let ext = family != "VOP3" && matches!(first & 511, 0xe9 | 0xea | 0xf9 | 0xfa);
        if !ext
            && let Some(line) =
                lane_statement(name, family, pair, literal).map_err(|e| format!("{name}: {e}"))?
        {
            return Ok(vec![line]);
        }
    }
    if name == "ds_bpermute_b32" {
        let (offset, addr, data, d) = (
            first & 65535,
            (pair >> 32) & 255,
            (pair >> 40) & 255,
            pair >> 56,
        );
        return Ok(vec![format!(
            "{{ uint value = ReadLane64(lane, v[{data}], (lane & 32u) | (((v[{addr}] + {offset}u) >> 2) & 31u)); if (LANE_ON) v[{d}] = value; }}"
        )]);
    }
    let mut lines = super::statements(name, inst, bytes)?;
    for line in &mut lines {
        *line = line.replace("((s[126] >> lane) & 1u) != 0", "LANE_ON");
        if family == "VOPC" {
            let (low, upper) = if name.starts_with("v_cmpx") {
                (126, 127)
            } else {
                (106, 107)
            };
            *line = line.replace(
                &format!("s[{low}] = Ballot(lane, t);"),
                &format!("uint2 m = Ballot64(lane, t); s[{low}] = m.x; s[{upper}] = m.y;"),
            );
        } else if matches!(
            name,
            "v_add_co_ci_u32" | "v_sub_co_ci_u32" | "v_subrev_co_ci_u32"
        ) && family == "VOP2"
        {
            *line = line
                .replace(
                    "(s[106] >> lane) & 1u",
                    "(Bit64(s[106], s[107], lane) ? 1u : 0u)",
                )
                .replace(
                    "s[106] = Ballot(lane, o);",
                    "uint2 m = Ballot64(lane, o); s[106] = m.x; s[107] = m.y;",
                );
        } else if !family.starts_with("SOP") && line.contains("lane") && !name.contains("addtid") {
            return Err(format!("{name}: untranslated wave64 lane exchange"));
        }
    }
    Ok(lines)
}
