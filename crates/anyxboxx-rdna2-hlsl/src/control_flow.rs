//! Uniform scalar branches split the program into basic blocks. As in the Python
//! emitter, use a loop and guards so group barriers work on WARP.
use std::{collections::BTreeSet, fmt::Write};

use anyxboxx_rdna2_ref::decode::{Instruction, mnemonic};

fn condition(name: &str, lanes: usize) -> Option<&'static str> {
    Some(match name {
        "s_branch" => "true",
        "s_cbranch_scc0" => "!scc",
        "s_cbranch_scc1" => "scc",
        "s_cbranch_vccz" if lanes == 64 => "(s[106] | s[107]) == 0u",
        "s_cbranch_vccnz" if lanes == 64 => "(s[106] | s[107]) != 0u",
        "s_cbranch_execz" if lanes == 64 => "(s[126] | s[127]) == 0u",
        "s_cbranch_execnz" if lanes == 64 => "(s[126] | s[127]) != 0u",
        "s_cbranch_vccz" => "s[106] == 0",
        "s_cbranch_vccnz" => "s[106] != 0",
        "s_cbranch_execz" => "s[126] == 0",
        "s_cbranch_execnz" => "s[126] != 0",
        _ => return None,
    })
}

pub fn emit(
    data: &[u8],
    instructions: &[Instruction],
    lanes: usize,
    finish: &str,
    output: &mut String,
    statements: impl Fn(&str, &Instruction, &[u8]) -> Result<Vec<String>, String>,
) -> Result<(), String> {
    let mut leaders = BTreeSet::from([0]);
    let mut blocks = Vec::with_capacity(instructions.len());
    for inst in instructions {
        let name = mnemonic(inst.family, inst.opcode).ok_or_else(|| {
            format!(
                "untranslated instruction: {} opcode {}",
                inst.family, inst.opcode
            )
        })?;
        let end = inst.offset + inst.size * 4;
        let bytes = &data[inst.offset..end];
        let branch = if let Some(test) = condition(name, lanes) {
            let displacement = i16::from_le_bytes(bytes[..2].try_into().unwrap()) as i64;
            let target = end as i64 + 4 * displacement;
            if target < 0
                || (target as usize != data.len()
                    && instructions
                        .binary_search_by_key(&(target as usize), |instruction| instruction.offset)
                        .is_err())
            {
                return Err(format!("{name}: invalid branch target {target}"));
            }
            leaders.insert(target as usize);
            Some((test, target))
        } else {
            None
        };
        if end < data.len() && (branch.is_some() || name == "s_endpgm") {
            leaders.insert(end);
        }
        blocks.push((inst, bytes, name, branch));
    }

    let has_flow = leaders.len() > 1 || blocks.iter().any(|item| item.3.is_some());
    let indent = if has_flow { "      " } else { "  " };
    if has_flow {
        output.push_str("  uint pc = 0;\n  [loop] while (pc != END_PC) {\n");
    }
    for (inst, bytes, name, branch) in blocks {
        if has_flow && leaders.contains(&inst.offset) {
            if inst.offset > 0 {
                output.push_str("    }\n");
            }
            writeln!(output, "    if (pc == {}u) {{", inst.offset).unwrap();
        }
        if let Some((test, target)) = branch {
            writeln!(
                output,
                "{indent}if ({test}) {{ pc = {target}u; continue; }}"
            )
            .unwrap();
        } else {
            let lines = if name == "s_endpgm" {
                vec![finish.to_owned()]
            } else {
                statements(name, inst, bytes)?
            };
            for line in lines {
                writeln!(output, "{indent}{line}").unwrap();
            }
        }
        let next = inst.offset + bytes.len();
        if next < data.len() && leaders.contains(&next) && name != "s_endpgm" {
            writeln!(output, "      pc = {next}u; continue;").unwrap();
        }
    }
    if has_flow {
        writeln!(output, "    }}\n    {finish}\n  }}").unwrap();
    }
    Ok(())
}
