//! RDNA 2 to HLSL translation. Unsupported instructions are rejected by name.

use anyxboxx_rdna2_ref::decode::{Instruction, decode};

mod buffer_memory;
mod control_flow;
mod division;
mod extensions;
mod flat_atomic;
mod flat_memory;
pub mod graphics;
mod half;
mod half_templates;
mod image;
mod int16;
mod int16_templates;
mod lds;
mod lds_lanes;
mod packed_half;
mod sad;
mod scalar;
mod scalar_atomic;
mod scalar_binary;
mod scalar_compare;
mod scalar_immediate;
mod scalar_memory;
mod scalar_movrel;
mod scalar_wide;
mod subword;
mod typed_buffer;
mod vector_binary;
mod vector_compare;
mod vector_dot;
mod vector_movrel;
mod vector_ternary;
mod vector_unary;
mod wave64;
mod wide;
mod wide_templates;

const PRELUDE: &str = concat!(
    include_str!("prelude0.hlsl"),
    include_str!("prelude1.hlsl"),
    include_str!("prelude2.hlsl"),
    include_str!("prelude3.hlsl"),
    include_str!("prelude4.hlsl"),
);

/// Translate a wave32 compute program, rejecting unsupported instructions by name.
pub fn translate(data: &[u8]) -> Result<String, String> {
    translate_wave(data, 32)
}

/// Compile a virtual wave of 32 or 64 lanes into one D3D12 thread group.
pub fn translate_wave(data: &[u8], lanes: usize) -> Result<String, String> {
    let (header, init) = match lanes {
        32 => (
            include_str!("wave32_head.hlsl"),
            include_str!("wave32_init.hlsl"),
        ),
        64 => (
            include_str!("wave64_head.hlsl"),
            include_str!("wave64_init.hlsl"),
        ),
        _ => return Err(format!("unsupported wave size: {lanes}")),
    };
    if !data.len().is_multiple_of(4) {
        return Err("truncated instruction".into());
    }
    let mut output = [header, PRELUDE, init].concat();
    control_flow::emit(
        data,
        &decode(data),
        lanes,
        "return;",
        &mut output,
        if lanes == 64 {
            wave64::statements
        } else {
            statements
        },
    )?;
    output.push_str("}\n");
    Ok(output)
}

fn statements(name: &str, instruction: &Instruction, bytes: &[u8]) -> Result<Vec<String>, String> {
    let word = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let second = bytes
        .get(4..8)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let pair = word as u64 | ((second.unwrap_or(0) as u64) << 32);
    let literal = bytes
        .get(8..12)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let family = instruction.family;
    // Cache operations and scheduling hints leave the oracle's architectural state unchanged.
    if matches!(
        name,
        "s_nop"
            | "s_waitcnt"
            | "s_barrier"
            | "s_sleep"
            | "s_setprio"
            | "s_ttracedata"
            | "s_clause"
            | "s_inst_prefetch"
            | "s_icache_inv"
            | "s_incperflevel"
            | "s_decperflevel"
            | "s_ttracedata_imm"
            | "s_wait_idle"
            | "s_waitcnt_depctr"
            | "s_dcache_inv"
            | "s_gl1_inv"
            | "s_dcache_wb"
            | "buffer_gl0_inv"
            | "buffer_gl1_inv"
            | "ds_nop"
            | "v_nop"
            | "v_pipeflush"
            | "v_clrexcp"
            | "s_atc_probe"
            | "s_atc_probe_buffer"
    ) {
        return Ok(vec![]);
    }
    if name == "s_getpc_b64" {
        let d = (word >> 16) & 127;
        if d == 125 {
            return Ok(vec![]);
        }
        if d % 2 != 0 {
            return Err(format!("{name}: unaligned SGPR pair s{d}"));
        }
        let pc = (instruction.offset + bytes.len()) as u64;
        let high = if d + 1 == 125 {
            "s_null".into()
        } else {
            format!("s[{}]", d + 1)
        };
        return Ok(vec![format!(
            "s[{d}] = 0x{:X}u; {high} = 0x{:X}u;",
            pc as u32,
            pc >> 32
        )]);
    }
    if let Some((kinds, template)) = half_templates::operation(name) {
        return half::statement(
            name,
            family,
            pair,
            if family == "VOP3" { literal } else { second },
            kinds,
            template,
        )
        .map(|line| vec![line]);
    }
    let line = match family {
        "VOP3" if name.starts_with("v_cmp") => vector_compare::vop3(name, pair, literal, 32)?,
        "VOP3" if division::supports(name) => division::statement(name, pair, literal)?,
        "VOP1" | "VOP3" if wide_templates::template(name).is_some() => {
            wide::statement(name, family, pair)?
        }
        "VOP1" | "VOP2" | "VOPC" if matches!(word & 511, 0xe9 | 0xea | 0xf9 | 0xfa) => {
            extensions::statement(
                name,
                family,
                word,
                second.ok_or_else(|| format!("{name}: missing extension word"))?,
            )?
        }
        "VOP2" | "VOP3" | "VOP3P" if packed_half::supports(name) => packed_half::statement(
            name,
            family,
            pair,
            if family == "VOP2" { second } else { literal },
        )?,
        "VOP3" if sad::supports(name) => sad::statement(name, pair, literal)?,
        "VOP3" | "VOP3P" if int16::supports(name) => {
            int16::statement(name, family == "VOP3P", pair, literal)?
        }
        "MIMG" => return image::statements(name, bytes),
        "MTBUF" | "MUBUF" if anyxboxx_rdna2_ref::format_access::supports(name) => {
            return typed_buffer::statements(name, pair);
        }
        "MUBUF" => return buffer_memory::statements(name, pair),
        "GLOBAL" | "FLAT" => return flat_memory::statements(name, family, pair),
        "DS" => return lds::statements(name, pair),
        "SMEM" => return scalar_memory::statements(name, pair),
        "VOP3P" => vector_dot::statement(name, pair, literal)?,
        "VOP3" => vector_ternary::statement(name, pair, literal)?,
        "VOPC" => vector_compare::statement(name, word, second)?,
        "VOP2" => vector_binary::statement(name, word, second)?,
        "VOP1" => {
            return Ok(vector_unary::statement(name, word, second)?
                .into_iter()
                .collect());
        }
        "SOPK" => {
            return Ok(scalar_immediate::statement(name, word)?
                .into_iter()
                .collect());
        }
        "SOP1" if scalar_movrel::supports(name) => {
            return Ok(scalar_movrel::statement(name, word, second)?
                .into_iter()
                .collect());
        }
        "SOP1" | "SOP2" | "SOPC" if scalar_wide::supports(name, family) => {
            scalar_wide::statement(name, family, word)?
        }
        "SOP1" => scalar::statement(name, word, second)?,
        "SOP2" => scalar_binary::statement(name, word, second)?,
        "SOPC" => scalar_compare::statement(name, word, second)?,
        "SOPP" if name == "s_endpgm" => "return;".into(),
        _ => return Err(format!("untranslated instruction: {name}")),
    };
    Ok(vec![line])
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn rejects_untranslated_instruction_by_name() {
        let data = 0xbe80_0c80u32.to_le_bytes();
        assert!(translate(&data).unwrap_err().contains("s_brev_b64"));
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(translate(&[0]).is_err());
    }
}
