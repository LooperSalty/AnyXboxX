//! RDNA 2 to HLSL translation. Unsupported instructions are rejected by name.

use anyxboxx_rdna2_ref::decode::{decode, mnemonic};

const PRELUDE: &str = concat!(
    include_str!("prelude0.hlsl"),
    include_str!("prelude1.hlsl"),
    include_str!("prelude2.hlsl"),
);

/// Translate a straight-line wave32 program. Only s_nop and s_endpgm are implemented.
pub fn translate(data: &[u8]) -> Result<String, String> {
    if !data.len().is_multiple_of(4) {
        return Err("truncated instruction".into());
    }
    let instructions = decode(data);
    let mut lines = String::from(PRELUDE);
    for (index, instruction) in instructions.iter().enumerate() {
        let name = mnemonic(instruction.family, instruction.opcode)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{} opcode {}", instruction.family, instruction.opcode));
        match name.as_str() {
            "s_nop" => (),
            "s_endpgm" if index + 1 == instructions.len() => lines.push_str("  return;\n"),
            _ => return Err(format!("untranslated instruction: {name}")),
        }
    }
    lines.push_str("}\n");
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::translate;

    #[test]
    fn rejects_untranslated_instruction_by_name() {
        let data = 0xbe80_0380u32.to_le_bytes();
        assert!(translate(&data).unwrap_err().contains("s_mov_b32"));
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(translate(&[0]).is_err());
    }
}
