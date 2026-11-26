//! The program counter instructions: s_getpc_b64, s_setpc_b64, s_swappc_b64 (SOP1) and s_call_b64
//! (SOPK). Port of `emu/rdna2_exec_pc.py`.
//!
//! The address of an instruction is `PROGRAM_BASE` plus its byte offset in the program, on 64 bits;
//! `PROGRAM_BASE` is 0. The program is not copied into the wave's memory: an address that s_getpc_b64
//! reads serves to come back into the code, not to read data. Semantics (AMD's RDNA 2 XML
//! specification, and AMD's RDNA 3 pseudo-code of the same instructions, as the Python cites them):
//! - s_getpc_b64: D = the address of the next instruction;
//! - s_setpc_b64: PC = S0 (an SGPR pair);
//! - s_swappc_b64: the target is S0, read first; D = the next address; PC = the target;
//! - s_call_b64: D = the next address; PC = PC + 4 + SIMM16 × 4 (the displacement of s_branch).
//!
//! As in the Python: D must be an aligned SGPR pair (even) or NULL; S0 must be an aligned SGPR pair
//! below 128, or NULL (which reads 0); a target outside the program or not on a 4-byte boundary is
//! refused. The target is checked before D is written.

use crate::error::Error;
use crate::scalar::{Fields, sign_extend};
use crate::wave::{NULL, Wave};

/// Address of byte 0 of the program (`PROGRAM_BASE`).
pub const PROGRAM_BASE: u64 = 0;
const DWORD: i64 = 4;
/// S0 of s_setpc and s_swappc must be an SGPR pair below this code (`SGPR_END`).
const SGPR_END: u32 = 128;

/// The family each instruction is executed in (`FAMILIES`): another family's opcode of the same name
/// is not one of them.
pub fn family_of(name: &str) -> Option<&'static str> {
    match name {
        "s_getpc_b64" | "s_setpc_b64" | "s_swappc_b64" => Some("SOP1"),
        "s_call_b64" => Some("SOPK"),
        _ => None,
    }
}

fn refused(message: String) -> Error {
    Error::Untranslated(message)
}

/// Writes the return address into the pair D (`_write`, `check_destination`).
fn write_destination(wave: &mut Wave, name: &str, code: u32, value: u64) -> Result<(), Error> {
    if code != NULL && code % 2 == 1 {
        return Err(refused(format!(
            "{name} : destination s{code} impaire, une paire de SGPR doit être alignée"
        )));
    }
    wave.write_scalar(code, value, 2)
}

/// The pair S0 of s_setpc and s_swappc (`_jump_source`).
fn jump_source(wave: &Wave, name: &str, code: u32) -> Result<i128, Error> {
    if code >= SGPR_END || (code != NULL && code % 2 == 1) {
        return Err(refused(format!(
            "{name} : S0 doit être une paire de SGPR alignée (opérande {code})"
        )));
    }
    Ok(i128::from(wave.read_scalar(code, None, 2)?))
}

/// The byte offset of a target address, refused outside the program or off a word (`_offset`).
fn target_offset(name: &str, target: i128, length: usize) -> Result<usize, Error> {
    let offset = target - i128::from(PROGRAM_BASE);
    if offset < 0 || offset >= length as i128 || offset % i128::from(DWORD) != 0 {
        return Err(refused(format!(
            "{name} : cible 0x{:X} hors du programme ou non alignée",
            target as u64
        )));
    }
    Ok(offset as usize)
}

/// Executes the instruction at `wave.pc` and returns the offset of the next one to run.
/// `following`: the offset of the instruction after it; `length`: the program's size in bytes.
pub fn execute(
    wave: &mut Wave,
    name: &str,
    fields: &Fields,
    following: usize,
    length: usize,
) -> Result<usize, Error> {
    let return_address = PROGRAM_BASE + following as u64;
    match name {
        "s_getpc_b64" => {
            write_destination(wave, name, fields.sdst, return_address)?;
            Ok(following)
        }
        "s_call_b64" => {
            let displacement = sign_extend(u64::from(fields.simm16), 16) * DWORD;
            let target =
                i128::from(PROGRAM_BASE) + wave.pc as i128 + i128::from(DWORD + displacement);
            let target = target_offset(name, target, length)?;
            write_destination(wave, name, fields.sdst, return_address)?;
            Ok(target)
        }
        _ => {
            let target = target_offset(name, jump_source(wave, name, fields.ssrc0)?, length)?;
            if name == "s_swappc_b64" {
                write_destination(wave, name, fields.sdst, return_address)?;
            }
            Ok(target)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(sdst: u32, ssrc0: u32, simm16: u32) -> Fields {
        Fields {
            sdst,
            ssrc0,
            ssrc1: 0,
            simm16,
        }
    }

    #[test]
    fn getpc_writes_the_next_address_into_an_aligned_pair() {
        let mut wave = Wave::new(32).unwrap();
        wave.pc = 8;
        assert_eq!(
            execute(&mut wave, "s_getpc_b64", &fields(4, 0, 0), 12, 64),
            Ok(12)
        );
        assert_eq!((wave.s[4], wave.s[5]), (12, 0));
        assert!(matches!(
            execute(&mut wave, "s_getpc_b64", &fields(5, 0, 0), 12, 64),
            Err(Error::Untranslated(_))
        ));
    }

    #[test]
    fn call_jumps_like_a_branch_and_checks_the_target_before_writing() {
        let mut wave = Wave::new(32).unwrap();
        wave.pc = 8;
        // simm16 = -2: 8 + 4 - 8 = 4.
        assert_eq!(
            execute(&mut wave, "s_call_b64", &fields(2, 0, 0xFFFE), 12, 64),
            Ok(4)
        );
        assert_eq!(wave.s[2], 12);
        // Before the program: refused, and D is left alone.
        let result = execute(&mut wave, "s_call_b64", &fields(6, 0, 0xFFF0), 12, 64);
        assert!(matches!(result, Err(Error::Untranslated(_))));
        assert_eq!(wave.s[6], 0);
    }

    #[test]
    fn setpc_and_swappc_read_an_aligned_pair_and_refuse_targets_off_the_program() {
        let mut wave = Wave::new(64).unwrap();
        wave.s[10] = 20;
        assert_eq!(
            execute(&mut wave, "s_setpc_b64", &fields(0, 10, 0), 4, 32),
            Ok(20)
        );
        assert_eq!(
            execute(&mut wave, "s_swappc_b64", &fields(10, 10, 0), 4, 32),
            Ok(20)
        );
        assert_eq!((wave.s[10], wave.s[11]), (4, 0));
        // NULL reads 0; an odd pair, a code past the SGPRs, or a misaligned target are refused.
        assert_eq!(
            execute(&mut wave, "s_setpc_b64", &fields(0, NULL, 0), 4, 32),
            Ok(0)
        );
        for code in [11, 128, 129] {
            let result = execute(&mut wave, "s_setpc_b64", &fields(0, code, 0), 4, 32);
            assert!(matches!(result, Err(Error::Untranslated(_))), "{code}");
        }
        wave.s[10] = 6;
        let result = execute(&mut wave, "s_setpc_b64", &fields(0, 10, 0), 4, 32);
        assert!(matches!(result, Err(Error::Untranslated(_))));
        wave.s[11] = 1;
        wave.s[10] = 0;
        let result = execute(&mut wave, "s_setpc_b64", &fields(0, 10, 0), 4, 32);
        assert!(matches!(result, Err(Error::Untranslated(_))));
    }
}
