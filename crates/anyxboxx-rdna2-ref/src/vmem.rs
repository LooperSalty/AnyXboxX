//! Vector memory accesses of the reference interpreter, shared by FLAT, GLOBAL, SCRATCH and MUBUF: loads
//! and stores of 1 to 64 bytes, and the atomics on 32 or 64 bits. Port of `VECTOR_PATTERN`,
//! `VECTOR_ATOMIC`, `rdna2_exec_atomic.vector`, `_access` and `_vector_atomic_ext` in
//! `emu/rdna2_exec_memory.py`.
//!
//! A load of fewer than 4 bytes fills one VGPR, zero-extended or, for `sbyte` and `sshort`,
//! sign-extended. An access that is not inside its buffer (MUBUF) reads zeros and writes nothing.
//! The float atomics (fcmpswap, fmin, fmax, 32-bit and `_x2`) follow `atomic_float`.

use crate::atomic;
use crate::ds::{load_into, store_bytes};
use crate::ds_atomic::{lane_value, set_lane_value};
use crate::error::Error;
use crate::wave::Wave;

const DWORD: usize = 4;

/// What a vector memory mnemonic does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Load {
        size: usize,
        signed: bool,
    },
    Store {
        size: usize,
    },
    /// `kind` is the `atomic::apply` kind and `width` 32 or 64. `pair`: the compared value follows
    /// the written one in the VGPRs (cmpswap).
    Atomic {
        kind: &'static str,
        width: u32,
        pair: bool,
    },
}

/// The address spaces of the vector memory mnemonics.
const SPACES: [&str; 4] = ["global", "flat", "scratch", "buffer"];

/// The 32-bit atomics that have a plain name (`global_atomic_add`), and their kind.
const SIMPLE: [(&str, &str); 10] = [
    ("add", "add"),
    ("sub", "sub"),
    ("swap", "swap"),
    ("and", "and"),
    ("or", "or"),
    ("xor", "xor"),
    ("umin", "umin"),
    ("umax", "umax"),
    ("smin", "smin"),
    ("smax", "smax"),
];

/// The atomics with a 32-bit form only, or the `_x2` 64-bit form (`global_atomic_inc_x2`).
const EXTENDED_32: [&str; 7] = ["cmpswap", "inc", "dec", "fcmpswap", "fmin", "fmax", "csub"];
const EXTENDED_X2: [&str; 16] = [
    "swap", "cmpswap", "add", "sub", "smin", "umin", "smax", "umax", "and", "or", "xor", "inc",
    "dec", "fcmpswap", "fmin", "fmax",
];

/// The atomics whose DATA is a pair: the written value, then the compared one.
fn is_pair(kind: &str) -> bool {
    matches!(kind, "cmpswap" | "fcmpswap")
}

/// The size in bytes and the signedness of a data type suffix (`dwordx4`, `sbyte`, `b64`…).
fn access_type(suffix: &str) -> Option<(usize, bool)> {
    Some(match suffix {
        "dword" | "b32" => (4, false),
        "dwordx2" | "b64" => (8, false),
        "dwordx3" | "b96" => (12, false),
        "dwordx4" | "b128" => (16, false),
        "dwordx8" => (32, false),
        "dwordx16" => (64, false),
        "ubyte" | "byte" | "u8" | "b8" => (1, false),
        "sbyte" | "i8" => (1, true),
        "ushort" | "short" | "u16" | "b16" => (2, false),
        "sshort" | "i16" => (2, true),
        _ => return None,
    })
}

/// The atomic named `op` in `space` (`global_atomic_<op>`), or `None` when it is not one the port runs.
fn atomic_kind(space: &str, op: &str) -> Option<Kind> {
    if space == "scratch" {
        return None;
    }
    if let Some((_, kind)) = SIMPLE.iter().find(|(name, _)| *name == op) {
        return Some(Kind::Atomic {
            kind,
            width: 32,
            pair: false,
        });
    }
    if let Some(base) = op.strip_suffix("_x2") {
        let kind: &'static str = EXTENDED_X2.iter().find(|known| **known == base)?;
        return Some(Kind::Atomic {
            kind,
            width: 64,
            pair: is_pair(kind),
        });
    }
    let kind: &'static str = EXTENDED_32.iter().find(|known| **known == op)?;
    Some(Kind::Atomic {
        kind,
        width: 32,
        pair: is_pair(kind),
    })
}

/// What a vector memory mnemonic does, or `None` when it is not one of them.
pub fn shape(name: &str) -> Option<Kind> {
    let (space, rest) = SPACES.iter().find_map(|space| {
        name.strip_prefix(*space)
            .and_then(|after| after.strip_prefix('_'))
            .map(|rest| (*space, rest))
    })?;
    if let Some(suffix) = rest.strip_prefix("load_") {
        let (size, signed) = access_type(suffix)?;
        return Some(Kind::Load { size, signed });
    }
    if let Some(suffix) = rest.strip_prefix("store_") {
        let (size, _) = access_type(suffix)?;
        return Some(Kind::Store { size });
    }
    atomic_kind(space, rest.strip_prefix("atomic_")?)
}

/// The little-endian value of `bytes` (at most 8 of them).
fn le_value(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .rev()
        .fold(0u64, |acc, byte| acc << 8 | u64::from(*byte))
}

fn atomic_lane(
    wave: &mut Wave,
    (kind, width, pair): (&str, u32, bool),
    address: u64,
    (data, vdst): (usize, usize),
    lane: usize,
    returns: bool,
) -> Result<(), Error> {
    let words = width as usize / 32;
    let size = words * DWORD;
    let old = le_value(&wave.memory.read(address, size));
    let a = lane_value(wave, data, lane, words)?;
    let b = if pair {
        lane_value(wave, data + words, lane, words)?
    } else {
        0
    };
    let new = atomic::apply(kind, width, old, a, b).ok_or_else(|| {
        Error::Untranslated(format!("instruction non traduite : atomique {kind}"))
    })?;
    wave.memory.write(address, &new.to_le_bytes()[..size]);
    if returns {
        set_lane_value(wave, vdst, lane, old, words)?;
    }
    Ok(())
}

/// One lane's access at `address`. `inside` is false for a MUBUF access outside its records: a load
/// then gives zeros, and a store or an atomic does nothing. `data` is DATA (the source of a store or an
/// atomic, the first of a pair), `vdst` the destination of a load or of an atomic's old value.
pub fn access(
    wave: &mut Wave,
    kind: Kind,
    address: u64,
    (data, vdst): (usize, usize),
    lane: usize,
    inside: bool,
    returns: bool,
) -> Result<(), Error> {
    match kind {
        Kind::Load { size, signed } => {
            let bytes = if inside {
                wave.memory.read(address, size)
            } else {
                vec![0; size]
            };
            load_into(wave, vdst, lane, &bytes, signed)
        }
        Kind::Store { size } => {
            if inside {
                let bytes = store_bytes(wave, data, lane, size)?;
                wave.memory.write(address, &bytes);
            }
            Ok(())
        }
        Kind::Atomic { kind, width, pair } => {
            if !inside {
                return Ok(());
            }
            atomic_lane(
                wave,
                (kind, width, pair),
                address,
                (data, vdst),
                lane,
                returns,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_typed_loads_and_stores() {
        assert_eq!(
            shape("global_load_dwordx4"),
            Some(Kind::Load {
                size: 16,
                signed: false
            })
        );
        assert_eq!(
            shape("flat_load_sbyte"),
            Some(Kind::Load {
                size: 1,
                signed: true
            })
        );
        assert_eq!(shape("scratch_store_short"), Some(Kind::Store { size: 2 }));
        assert_eq!(
            shape("buffer_load_b96"),
            Some(Kind::Load {
                size: 12,
                signed: false
            })
        );
        assert_eq!(shape("global_load_dword_addtid"), None);
    }

    #[test]
    fn atomics_by_name_and_width() {
        assert_eq!(
            shape("flat_atomic_add"),
            Some(Kind::Atomic {
                kind: "add",
                width: 32,
                pair: false
            })
        );
        assert_eq!(
            shape("global_atomic_cmpswap_x2"),
            Some(Kind::Atomic {
                kind: "cmpswap",
                width: 64,
                pair: true
            })
        );
        assert_eq!(
            shape("buffer_atomic_csub"),
            Some(Kind::Atomic {
                kind: "csub",
                width: 32,
                pair: false
            })
        );
    }

    #[test]
    fn scratch_has_no_atomics() {
        assert_eq!(shape("scratch_atomic_add"), None);
        assert_eq!(shape("scratch_atomic_fmin"), None);
    }

    #[test]
    fn float_atomics_and_their_pairs() {
        assert_eq!(
            shape("global_atomic_fmin"),
            Some(Kind::Atomic {
                kind: "fmin",
                width: 32,
                pair: false
            })
        );
        assert_eq!(
            shape("flat_atomic_fcmpswap_x2"),
            Some(Kind::Atomic {
                kind: "fcmpswap",
                width: 64,
                pair: true
            })
        );
        assert_eq!(shape("global_atomic_fadd"), None);
    }
}
