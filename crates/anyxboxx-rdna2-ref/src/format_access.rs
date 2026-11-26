//! The fields of a typed buffer access and the checks of its V#: port of `access`, `selects`,
//! `check_descriptor`, `check_load_selects` and `store_selects_ok` of `emu/rdna2_buffer_format.py`, and
//! of `_resolve` in `emu/rdna2_exec_format.py`.
//!
//! The 64-bit word (MUBUFe_gfx10 and MTBUFe_gfx10 of LLVM): bits 0-11 OFFSET, 12 OFFEN, 13 IDXEN,
//! 16 LDS (MUBUF), 19-25 FORMAT (MTBUF), 32-39 VADDR, 40-47 VDATA, 48-52 SRSRC (in units of four SGPRs),
//! 55 TFE, 56-63 SOFFSET. V# word 3: DST_SEL_X/Y/Z/W in bits 2-0, 5-3, 8-6, 11-9, FORMAT in 18-12,
//! ADD_TID_ENABLE in bit 23; word 1: SWIZZLE_ENABLE in bit 31.
//!
//! MUBUF takes its format from the V# and applies DST_SEL; MTBUF takes it from the instruction, with no
//! DST_SEL, and a V# whose FORMAT is INVALID (0) disables the access (zeros read, nothing written).

use crate::buffer_format::{Format, format_for};
use crate::error::Error;

const OFFSET_MASK: u64 = 0xFFF;
const OFFEN_BIT: u32 = 12;
const IDXEN_BIT: u32 = 13;
const MUBUF_LDS_BIT: u32 = 16;
const MTBUF_FORMAT_SHIFT: u32 = 19;
const TFE_BIT: u32 = 55;
const FORMAT_MASK: u32 = 0x7F;
const WORD3_FORMAT_SHIFT: u32 = 12;
const ADD_TID_BIT: u32 = 23;
const SWIZZLE_ENABLE_BIT: u32 = 31;
const SELECT_BITS: u32 = 3;
const SELECT_MASK: u32 = 0x7;
pub const SELECT_ZERO: u32 = 0;
pub const SELECT_ONE: u32 = 1;
pub const SELECT_X: u32 = 4;

/// The fields of one typed access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    /// MTBUF: the format is in the instruction.
    pub typed: bool,
    pub store: bool,
    /// The components read or written (x = 1 ... xyzw = 4).
    pub count: usize,
    pub d16: bool,
    pub high: bool,
    pub offset: u64,
    pub offen: bool,
    pub idxen: bool,
    pub vaddr: usize,
    pub vdata: usize,
    /// The first SGPR of the V#.
    pub srsrc: usize,
    pub soffset: u32,
    /// The format of the instruction (MTBUF).
    pub fixed: Option<u32>,
}

/// (typed, store, count) of a typed access name, or `None` when the name is not one
/// (`^(t?)buffer_(load|store)_format_(x|xy|xyz|xyzw)$`).
pub fn parse_name(name: &str) -> Option<(bool, bool, usize, bool, bool)> {
    let (typed, rest) = match name.strip_prefix('t') {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    let rest = rest.strip_prefix("buffer_")?;
    let (store, rest) = if let Some(rest) = rest.strip_prefix("load_format_") {
        (false, rest)
    } else {
        (true, rest.strip_prefix("store_format_")?)
    };
    let (d16, rest) = rest
        .strip_prefix("d16_")
        .map_or((false, rest), |s| (true, s));
    let (high, rest) = rest
        .strip_prefix("hi_")
        .map_or((false, rest), |s| (true, s));
    if high && (!d16 || rest != "x") {
        return None;
    }
    let count = match rest {
        "x" => 1,
        "xy" => 2,
        "xyz" => 3,
        "xyzw" => 4,
        _ => return None,
    };
    Some((typed, store, count, d16, high))
}

/// True when `name` is a typed buffer access this module runs.
pub fn supports(name: &str) -> bool {
    parse_name(name).is_some()
}

fn bit(word: u64, index: u32) -> bool {
    word >> index & 1 == 1
}

/// The fields of the access `name` (`word`: its 64 bits); `Untranslated` for lds and tfe.
pub fn access(name: &str, word: u64) -> Result<Access, Error> {
    let (typed, store, count, d16, high) = parse_name(name).ok_or_else(|| {
        Error::Untranslated(format!("instruction non traduite : {name} (accès typé)"))
    })?;
    if !typed && bit(word, MUBUF_LDS_BIT) {
        return Err(Error::Untranslated(format!(
            "{name} avec lds non pris en charge"
        )));
    }
    if bit(word, TFE_BIT) {
        return Err(Error::Untranslated(format!(
            "{name} avec tfe non pris en charge"
        )));
    }
    Ok(Access {
        typed,
        store,
        count,
        d16,
        high,
        offset: word & OFFSET_MASK,
        offen: bit(word, OFFEN_BIT),
        idxen: bit(word, IDXEN_BIT),
        vaddr: ((word >> 32) & 0xFF) as usize,
        vdata: ((word >> 40) & 0xFF) as usize,
        srsrc: ((word >> 48) & 0x1F) as usize * 4,
        soffset: ((word >> 56) & 0xFF) as u32,
        fixed: typed.then_some((word >> MTBUF_FORMAT_SHIFT) as u32 & FORMAT_MASK),
    })
}

/// DST_SEL_X, Y, Z and W of V# word 3.
pub fn selects(word3: u32) -> [u32; 4] {
    std::array::from_fn(|i| (word3 >> (SELECT_BITS * i as u32)) & SELECT_MASK)
}

/// The FORMAT field of V# word 3.
fn descriptor_format(word3: u32) -> u32 {
    (word3 >> WORD3_FORMAT_SHIFT) & FORMAT_MASK
}

fn check_descriptor(words: &[u32; 4]) -> Result<(), Error> {
    if words[1] >> SWIZZLE_ENABLE_BIT & 1 == 1 || words[3] >> ADD_TID_BIT & 1 == 1 {
        return Err(Error::Untranslated(
            "V# avec SWIZZLE_ENABLE ou ADD_TID_ENABLE non pris en charge".into(),
        ));
    }
    Ok(())
}

/// MUBUF loads: DST_SEL 0, 1 or a component of the format (X to W), for each component read.
fn check_load_selects(format: &Format, word3: u32, count: usize) -> Result<(), Error> {
    for (component, &select) in selects(word3).iter().take(count).enumerate() {
        if select == SELECT_ZERO || select == SELECT_ONE {
            continue;
        }
        if select < SELECT_X || (select - SELECT_X) as usize >= format.count() {
            return Err(Error::Untranslated(format!(
                "DST_SEL {select} de la composante {component} non pris en charge avec le format {}",
                format.name()
            )));
        }
    }
    Ok(())
}

/// MUBUF stores: DST_SEL must name X, Y, Z, W in order for the components of the format, with DST_SEL_Z
/// constant (0 or 1) for a two-component format. The ISA (section 8.1.4, table 31, p. 59) only says that
/// BUFFER_STORE_FORMAT_* takes DST_SEL from the V#; what DST_SEL_Z = Z or W does to a two-component
/// store is not documented, so those V# are refused.
fn store_selects_ok(format: &Format, word3: u32) -> bool {
    let found = selects(word3);
    if (0..format.count()).any(|i| found[i] != SELECT_X + i as u32) {
        return false;
    }
    format.count() != 2 || found[2] == SELECT_ZERO || found[2] == SELECT_ONE
}

/// The format of the access and whether its V# enables it; `Untranslated` outside the supported case.
pub fn resolve(
    name: &str,
    access: &Access,
    words: &[u32; 4],
) -> Result<(&'static Format, bool), Error> {
    check_descriptor(words)?;
    let word3 = words[3];
    let code = access.fixed.unwrap_or_else(|| descriptor_format(word3));
    let format = format_for(code, access.store && !access.d16)?;
    if access.d16
        && ((access.store
            && !format.is_integer()
            && format.kind != crate::buffer_format::Kind::Float)
            || (!access.store && format.is_integer() && format.widths[0] == 32))
    {
        return Err(Error::Untranslated(format!(
            "{name}: unsupported d16 format"
        )));
    }
    let refuse = |what: String| Err(Error::Untranslated(format!("{name} : {what}")));
    if access.typed && access.count > format.count() {
        return refuse(format!(
            "{} composantes pour le format {}",
            access.count,
            format.name()
        ));
    }
    if access.store && access.count != format.count() {
        return refuse(format!(
            "{} composantes écrites au format {}",
            access.count,
            format.name()
        ));
    }
    if !access.typed && access.store && !store_selects_ok(format, word3) {
        return refuse(format!(
            "DST_SEL {:?} non pris en charge en écriture",
            selects(word3)
        ));
    }
    if !access.typed && !access.store {
        check_load_selects(format, word3, access.count)?;
    }
    let enabled = !access.typed || descriptor_format(word3) != 0;
    Ok((format, enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: u32 = 4 | 5 << 3 | 6 << 6 | 7 << 9;

    #[test]
    fn names_give_the_form_and_the_component_count() {
        assert_eq!(
            parse_name("tbuffer_load_format_xyz"),
            Some((true, false, 3, false, false))
        );
        assert_eq!(
            parse_name("buffer_store_format_x"),
            Some((false, true, 1, false, false))
        );
        assert_eq!(
            parse_name("buffer_load_format_d16_x"),
            Some((false, false, 1, true, false))
        );
        assert_eq!(parse_name("buffer_load_dword"), None);
    }

    #[test]
    fn lds_and_tfe_are_refused() {
        assert!(access("buffer_load_format_x", 1 << 16).is_err());
        assert!(access("tbuffer_load_format_x", 1 << 16).is_ok());
        assert!(access("tbuffer_load_format_x", 1 << 55).is_err());
        let fields = access("tbuffer_load_format_x", 22 << 19 | 3 << 48).unwrap();
        assert_eq!((fields.fixed, fields.srsrc), (Some(22), 12));
    }

    #[test]
    fn mtbuf_with_an_invalid_descriptor_format_is_disabled() {
        let fields = access("tbuffer_load_format_x", 22 << 19).unwrap();
        let (_, enabled) = resolve("t", &fields, &[0, 0, 0, IDENTITY]).unwrap();
        assert!(!enabled);
        let (_, enabled) = resolve("t", &fields, &[0, 0, 0, IDENTITY | 1 << 12]).unwrap();
        assert!(enabled);
    }

    #[test]
    fn mubuf_stores_need_identity_selects() {
        let fields = access("buffer_store_format_xy", 0).unwrap();
        let word3 = |selects: u32| selects | 27 << 12; // 16_16_UINT
        assert!(resolve("b", &fields, &[0, 0, 0, word3(IDENTITY & 0x3F)]).is_ok());
        assert!(resolve("b", &fields, &[0, 0, 0, word3(IDENTITY)]).is_err());
        assert!(resolve("b", &fields, &[0, 0, 0, word3(5 | 4 << 3)]).is_err());
    }
}
