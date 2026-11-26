//! Instruction encodings: family, opcode and length of each instruction (port of `emu/rdna2.py`).
//!
//! The formats come from AMD's public « RDNA 2 Instruction Set Architecture » (microcode formats).
//! Mnemonics come from the table generated from LLVM's encoding tests (`mnemonics.rs`), completed by
//! the ISA-only encodings the Python keeps in `ISA_ONLY_MNEMONICS`.

use crate::mnemonics::mnemonic as llvm_mnemonic;

/// Bytes per 32-bit word.
pub const DWORD: usize = 4;
/// Source operand code of a literal constant: one more word after the instruction.
pub const LITERAL: u32 = 255;

/// (shift, expected value, family) of the first word, most specific first.
const PREFIXES: &[(u32, u32, &str)] = &[
    (23, 0x17D, "SOP1"),
    (23, 0x17E, "SOPC"),
    (23, 0x17F, "SOPP"),
    (25, 0x3F, "VOP1"),
    (25, 0x3E, "VOPC"),
    (26, 0x32, "VINTRP"),
    (26, 0x33, "VOP3P"),
    (26, 0x35, "VOP3"),
    (26, 0x36, "DS"),
    (26, 0x37, "FLAT"),
    (26, 0x38, "MUBUF"),
    (26, 0x3A, "MTBUF"),
    (26, 0x3C, "MIMG"),
    (26, 0x3D, "SMEM"),
    (26, 0x3E, "EXP"),
    (28, 0xB, "SOPK"),
    (30, 0x2, "SOP2"),
    (31, 0x0, "VOP2"),
];

/// Opcode field of each family: (shift, mask).
fn opcode_field(family: &str) -> (u32, u32) {
    match family {
        "SOP2" => (23, 0x7F),
        "SOPK" => (23, 0x1F),
        "SOP1" => (8, 0xFF),
        "SOPC" | "SOPP" => (16, 0x7F),
        "VOP1" => (9, 0xFF),
        "VOPC" => (17, 0xFF),
        "VOP2" => (25, 0x3F),
        "VOP3" => (16, 0x3FF),
        "VOP3P" => (16, 0x7F),
        "VINTRP" => (16, 0x3),
        "DS" => (18, 0xFF),
        "FLAT" | "MUBUF" | "MIMG" => (18, 0x7F),
        "MTBUF" => (16, 0x7),
        "SMEM" => (18, 0xFF),
        _ => (0, 0),
    }
}

/// Families whose encoding is two words long.
fn is_wide(family: &str) -> bool {
    matches!(
        family,
        "VOP3"
            | "VOP3P"
            | "DS"
            | "FLAT"
            | "MUBUF"
            | "MTBUF"
            | "MIMG"
            | "SMEM"
            | "EXP"
            | "SCRATCH"
            | "GLOBAL"
    )
}

/// Opcodes of VOP2 whose encoding carries a literal even without the 255 operand.
const VOP2_WITH_LITERAL: &[u32] = &[0x20, 0x21, 0x2C, 0x2D, 0x37, 0x38];
/// SOPK opcode s_setreg_imm32: the literal is a second word.
const SOPK_SETREG_IMM32: u32 = 0x15;
/// The extension words of SDWA and DPP.
const VOP_EXTENSIONS: &[u32] = &[0xE9, 0xEA, 0xF9, 0xFA];

/// One instruction of a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction {
    /// Byte offset from the start of the program.
    pub offset: usize,
    pub family: &'static str,
    pub opcode: u32,
    /// Length in 32-bit words.
    pub size: usize,
}

/// The family, opcode and length of the instruction at one offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Classified {
    pub family: &'static str,
    pub opcode: u32,
    pub size: usize,
}

/// The family of a first word, `"inconnue"` when no prefix matches.
pub fn family(word: u32) -> &'static str {
    for &(shift, value, family) in PREFIXES {
        if word >> shift == value {
            return if family == "FLAT" {
                match (word >> 14) & 0x3 {
                    1 => "SCRATCH",
                    2 => "GLOBAL",
                    _ => "FLAT",
                }
            } else {
                family
            };
        }
    }
    "inconnue"
}

fn has_literal(family: &str, word: u32, second: Option<u32>) -> bool {
    let lit = LITERAL;
    match family {
        "SOP2" | "SOPC" => word & 0xFF == lit || (word >> 8) & 0xFF == lit,
        "SOP1" => word & 0xFF == lit,
        "SOPK" => (word >> 23) & 0x1F == SOPK_SETREG_IMM32,
        "VOP1" | "VOPC" => word & 0x1FF == lit,
        "VOP2" => word & 0x1FF == lit || VOP2_WITH_LITERAL.contains(&((word >> 25) & 0x3F)),
        "VOP3" | "VOP3P" => second.is_some_and(|second| {
            [
                second & 0x1FF,
                (second >> 9) & 0x1FF,
                (second >> 18) & 0x1FF,
            ]
            .contains(&lit)
        }),
        _ => false,
    }
}

fn has_extension(family: &str, word: u32) -> bool {
    matches!(family, "VOP1" | "VOP2" | "VOPC") && VOP_EXTENSIONS.contains(&(word & 0x1FF))
}

fn size(family: &str, word: u32, second: Option<u32>) -> usize {
    let mut size = if is_wide(family) { 2 } else { 1 };
    if family == "MIMG" {
        size += ((word >> 1) & 0x3) as usize;
    }
    if has_extension(family, word) {
        size + 1
    } else {
        size + usize::from(has_literal(family, word, second))
    }
}

fn opcode(family: &str, word: u32, second: Option<u32>) -> u32 {
    let base = if matches!(family, "SCRATCH" | "GLOBAL") {
        "FLAT"
    } else {
        family
    };
    let (shift, mask) = opcode_field(base);
    let mut opcode = (word >> shift) & mask;
    // Bit OPM: the eighth bit of the opcode, in MIMG (bit 0) and MUBUF (bit 25).
    match base {
        "MIMG" => opcode |= (word & 1) << 7,
        "MUBUF" => opcode |= ((word >> 25) & 1) << 7,
        _ => {}
    }
    // MTBUFe_gfx10 of LLVM: Inst{53} is bit 3 of the opcode, in the second word.
    if let Some(second) = second.filter(|_| base == "MTBUF") {
        opcode |= ((second >> 21) & 1) << 3;
    }
    opcode
}

fn word_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// The family, opcode and length of the instruction at `offset` (which must hold a word).
pub fn classify(data: &[u8], offset: usize) -> Classified {
    let word = word_at(data, offset);
    let second = (data.len() >= offset + 2 * DWORD).then(|| word_at(data, offset + DWORD));
    let family = family(word);
    Classified {
        family,
        opcode: opcode(family, word, second),
        size: size(family, word, second),
    }
}

/// The ISA-only encodings that LLVM's tests do not name (`ISA_ONLY_MNEMONICS` in rdna2.py).
fn isa_only(family: &str, opcode: u32) -> Option<&'static str> {
    match (family, opcode) {
        ("VINTRP", 0) => Some("v_interp_p1_f32"),
        ("VINTRP", 1) => Some("v_interp_p2_f32"),
        ("VINTRP", 2) => Some("v_interp_mov_f32"),
        ("MTBUF", 2) => Some("tbuffer_load_format_xyz"),
        ("MTBUF", 6) => Some("tbuffer_store_format_xyz"),
        _ => None,
    }
}

/// The name of an instruction, if known.
pub fn mnemonic(family: &str, opcode: u32) -> Option<&'static str> {
    llvm_mnemonic(family, opcode).or_else(|| isa_only(family, opcode))
}

/// The instructions of a program; a last one is `"tronquée"` when the binary is cut.
pub fn decode(data: &[u8]) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset + DWORD <= data.len() {
        let Classified {
            family,
            opcode,
            size,
        } = classify(data, offset);
        if offset + size * DWORD > data.len() {
            let words = (data.len() - offset) / DWORD;
            instructions.push(Instruction {
                offset,
                family: "tronquée",
                opcode: 0,
                size: words,
            });
            break;
        }
        instructions.push(Instruction {
            offset,
            family,
            opcode,
            size,
        });
        offset += size * DWORD;
    }
    instructions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_le_bytes()).collect()
    }

    #[test]
    fn the_prefixes_pick_the_family_of_each_first_word() {
        // SOP2 s_add_u32 s0, s1, s2 : 10 | opcode 0 | sdst 0 | ssrc1 2 | ssrc0 1
        assert_eq!(family(0b10 << 30 | 2 << 8 | 1), "SOP2");
        // SOP1 s_mov_b32 (opcode 3) s0, s1 : 0x17D | sdst 0 | opcode 3 | ssrc0 1
        assert_eq!(family(0x17D << 23 | 3 << 8 | 1), "SOP1");
        // VOP2 (prefix 0) and VOP1 (prefix 0x3F)
        assert_eq!(family(0), "VOP2");
        assert_eq!(family(0x3F << 25), "VOP1");
    }

    #[test]
    fn a_literal_adds_one_word_and_a_second_vop3_word_is_read() {
        // SOP1 s_mov_b32 s0, literal : the literal is the second word.
        let program = bytes(&[0x17D << 23 | 3 << 8 | 255, 0x1234_5678]);
        let classified = classify(&program, 0);
        assert_eq!((classified.family, classified.size), ("SOP1", 2));
        assert_eq!(mnemonic("SOP1", classified.opcode), Some("s_mov_b32"));
    }

    #[test]
    fn decode_names_a_cut_program_and_ends_on_whole_words() {
        let program = bytes(&[0x17D << 23 | 3 << 8 | 1, 0x17D << 23 | 3 << 8 | 255]);
        let instructions = decode(&program);
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[1].family, "tronquée");
        assert_eq!(instructions[1].size, 1);
        assert!(decode(&[1, 2]).is_empty());
    }
}
