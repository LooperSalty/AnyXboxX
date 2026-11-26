//! EXP, the exports of a graphics shader (port of `_export` in `emu/rdna2_exec_graphics.py`; fields of
//! `exp_fields` in `emu/rdna2_graphics.py`, from AMD's "RDNA 2 Instruction Set Architecture").
//!
//! The 64-bit word: bits 0-3 EN (one bit per channel), 4-9 TARGET, 10 COMPR, 11 DONE, 12 VM, then the
//! four source VGPRs in the second word (bits 32-39, 40-47, 48-55, 56-63). Each active lane records its
//! four values under the target in `Wave::exports`, a disabled channel recording 0; a later export to
//! the same target replaces the lane's values. Every target is recorded, the null and invalid ones
//! included, as in the Python; DONE and VM change nothing. COMPR (16-bit exports) is refused before any
//! lane, as the Python does.

use crate::error::Error;
use crate::wave::Wave;

/// Values per export: x, y, z, w.
pub const CHANNELS: usize = 4;
const TARGET_SHIFT: u64 = 4;
const TARGET_MASK: u64 = 0x3F;
const COMPR_BIT: u64 = 10;
const SOURCE_SHIFT: u64 = 32;
const REGISTER_BITS: u64 = 8;

/// `(first target, last index, name)` of the gfx10 targets (`TARGETS` in `emu/rdna2_exp.py`); a name
/// with a last index of 0 takes no number.
const TARGETS: [(u32, u32, &str); 6] = [
    (9, 0, "null"),
    (8, 0, "mrtz"),
    (20, 0, "prim"),
    (0, 7, "mrt"),
    (12, 4, "pos"),
    (32, 31, "param"),
];

/// The gfx10 name of a target, or `invalid_target_N`, as LLVM prints it (`target_name`).
pub fn target_name(target: u32) -> String {
    for (first, last_index, name) in TARGETS {
        if (first..=first + last_index).contains(&target) {
            return if last_index == 0 {
                name.to_string()
            } else {
                format!("{name}{}", target - first)
            };
        }
    }
    format!("invalid_target_{target}")
}

/// The fields of an EXP word that the export reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpFields {
    pub enable: u32,
    pub target: u32,
    pub compressed: bool,
    pub sources: [usize; CHANNELS],
}

/// Decodes the 64-bit EXP word (`exp_fields`).
pub fn fields(word: u64) -> ExpFields {
    let mut sources = [0; CHANNELS];
    for (channel, source) in sources.iter_mut().enumerate() {
        *source = ((word >> (SOURCE_SHIFT + REGISTER_BITS * channel as u64)) & 0xFF) as usize;
    }
    ExpFields {
        enable: (word & 0xF) as u32,
        target: ((word >> TARGET_SHIFT) & TARGET_MASK) as u32,
        compressed: (word >> COMPR_BIT) & 1 == 1,
        sources,
    }
}

/// Executes one EXP instruction for every lane of EXEC. A target appears in `Wave::exports` only when
/// at least one lane exported to it.
pub fn execute(wave: &mut Wave, word: u64) -> Result<(), Error> {
    let fields = fields(word);
    if fields.compressed {
        return Err(Error::Untranslated(format!(
            "exp {} compr : export compressé (16 bits) non pris en charge",
            target_name(fields.target)
        )));
    }
    for lane in wave.active()? {
        let mut values = [0; CHANNELS];
        for (channel, value) in values.iter_mut().enumerate() {
            if fields.enable >> channel & 1 == 1 {
                *value = wave.v[fields.sources[channel]][lane];
            }
        }
        wave.exports
            .entry(fields.target)
            .or_default()
            .insert(lane, values);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EXP word: target, enable bits, flags (bit 10 and up of the first word), source registers.
    fn exp(target: u64, enable: u64, flags: u64, sources: [u64; 4]) -> u64 {
        let mut word = 0x3E << 26 | flags | target << 4 | enable;
        for (channel, source) in sources.iter().enumerate() {
            word |= source << (32 + 8 * channel);
        }
        word
    }

    #[test]
    fn target_names_follow_llvm_for_gfx10() {
        let names: Vec<String> = [0, 7, 8, 9, 12, 16, 17, 20, 21, 32, 63]
            .into_iter()
            .map(target_name)
            .collect();
        assert_eq!(
            names,
            [
                "mrt0",
                "mrt7",
                "mrtz",
                "null",
                "pos0",
                "pos4",
                "invalid_target_17",
                "prim",
                "invalid_target_21",
                "param0",
                "param31"
            ]
        );
    }

    #[test]
    fn disabled_channels_record_zero_and_inactive_lanes_record_nothing() {
        let mut wave = Wave::new(32).unwrap();
        wave.write_mask(crate::wave::EXEC_LO, 0b101).unwrap();
        wave.v[3] = (100..132).collect();
        // exp pos0, v0, off, v3, off done vm
        execute(&mut wave, exp(12, 0b0101, 3 << 11, [0, 7, 3, 9])).unwrap();
        let lanes = &wave.exports[&12];
        assert_eq!(lanes.keys().copied().collect::<Vec<_>>(), [0, 2]);
        assert_eq!(lanes[&2], [2, 0, 102, 0]);
    }

    #[test]
    fn a_second_export_to_a_target_replaces_the_lane_values() {
        let mut wave = Wave::new(64).unwrap();
        execute(&mut wave, exp(32, 0xF, 0, [0; 4])).unwrap();
        wave.v[0][63] = 7;
        execute(&mut wave, exp(32, 0x1, 0, [0; 4])).unwrap();
        assert_eq!(wave.exports[&32][&63], [7, 0, 0, 0]);
        assert_eq!(wave.exports[&32].len(), 64);
    }

    #[test]
    fn an_empty_exec_creates_no_target_and_compr_is_refused_even_then() {
        let mut wave = Wave::new(32).unwrap();
        wave.write_mask(crate::wave::EXEC_LO, 0).unwrap();
        execute(&mut wave, exp(0, 0xF, 0, [0; 4])).unwrap();
        assert!(wave.exports.is_empty());
        match execute(&mut wave, exp(0, 0xF, 1 << 10, [0; 4])) {
            Err(Error::Untranslated(message)) => {
                assert!(message.contains("mrt0 compr"), "{message}")
            }
            other => panic!("expected untranslated, got {other:?}"),
        }
    }
}
