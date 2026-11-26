//! The formats of typed buffer accesses (buffer_*_format, MUBUF; tbuffer_*_format, MTBUF). Port of
//! `Format`, `FORMATS` and `format_for` in `emu/rdna2_buffer_format.py`; the table itself is generated
//! (`format_table`), the conversions are in `format_convert`.
//!
//! A format is one code of the GFX10 unified format (7 bits): the FORMAT field of the V# word 3 for
//! MUBUF, of the instruction for MTBUF. The widths of a name read from the high bits down to bit 0:
//! 2_10_10_10 is x on bits 9-0 and w on bits 31-30, so `widths` starts at bit 0. The 11/10-bit float
//! formats (10_11_11_FLOAT, 11_11_10_FLOAT) are not in the table.
//!
//! Stores take only the UINT and SINT formats and the 32-bit FLOAT ones; the others are refused,
//! named, as the Python does (their conversion is not checked).

use crate::error::Error;
use crate::format_table::{BUFFER_FORMATS, FORMAT_NAMES};

const NAME_PREFIX: &str = "GFX10_FORMAT_";
const DWORD_BITS: u32 = 32;

/// How the bits of a component become a 32-bit value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unorm,
    Snorm,
    Uscaled,
    Sscaled,
    Uint,
    Sint,
    Float,
}

/// One typed buffer format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub code: u32,
    pub kind: Kind,
    /// The widths of the components in bits, from bit 0.
    pub widths: &'static [u32],
    /// The bytes of one element.
    pub size: usize,
    /// The alignment the address must have, in bytes.
    pub align: u64,
}

impl Format {
    /// The number of components.
    pub fn count(&self) -> usize {
        self.widths.len()
    }

    /// True for the formats a store may write: UINT, SINT and the 32-bit FLOAT formats.
    pub fn storable(&self) -> bool {
        match self.kind {
            Kind::Uint | Kind::Sint => true,
            Kind::Float => self.widths.iter().all(|width| *width == DWORD_BITS),
            _ => false,
        }
    }

    /// True for the integer formats (DST_SEL = 1 then gives 1, not 1.0).
    pub fn is_integer(&self) -> bool {
        matches!(self.kind, Kind::Uint | Kind::Sint)
    }

    /// The name, without its `GFX10_FORMAT_` prefix (`8_8_UNORM`).
    pub fn name(&self) -> String {
        let full = format_name(self.code);
        full.strip_prefix(NAME_PREFIX).unwrap_or(&full).to_string()
    }
}

/// The full name of a format code (`GFX10_FORMAT_8_UNORM`), or the code itself when the table has none.
pub fn format_name(code: u32) -> String {
    FORMAT_NAMES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or_else(|| code.to_string(), |(_, name)| (*name).to_string())
}

/// The format of `code` for a load or, with `store`, a store; `Untranslated` when it is not supported.
pub fn format_for(code: u32, store: bool) -> Result<&'static Format, Error> {
    let found = BUFFER_FORMATS
        .iter()
        .find(|format| format.code == code)
        .ok_or_else(|| {
            Error::Untranslated(format!("format {} non pris en charge", format_name(code)))
        })?;
    if store && !found.storable() {
        return Err(Error::Untranslated(format!(
            "écriture au format {} non prise en charge (conversion non vérifiée)",
            found.name()
        )));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_start_at_bit_zero_and_give_size_and_alignment() {
        let packed = format_for(50, false).unwrap(); // 2_10_10_10_UNORM
        assert_eq!(packed.widths, &[10, 10, 10, 2]);
        assert_eq!((packed.size, packed.align), (4, 4));
        let bytes = format_for(56, false).unwrap(); // 8_8_8_8_UNORM
        assert_eq!((bytes.count(), bytes.size, bytes.align), (4, 4, 1));
        assert_eq!(bytes.name(), "8_8_8_8_UNORM");
    }

    #[test]
    fn invalid_and_small_float_formats_are_refused() {
        assert!(matches!(format_for(0, false), Err(Error::Untranslated(_))));
        assert!(matches!(format_for(36, false), Err(Error::Untranslated(_)))); // 10_11_11_FLOAT
        assert!(matches!(
            format_for(127, false),
            Err(Error::Untranslated(_))
        ));
    }

    #[test]
    fn stores_take_integer_and_32_bit_float_formats_only() {
        assert!(format_for(5, true).is_ok()); // 8_UINT
        assert!(format_for(77, true).is_ok()); // 32_32_32_32_FLOAT
        assert!(format_for(1, true).is_err()); // 8_UNORM
        assert!(format_for(13, true).is_err()); // 16_FLOAT
        assert!(format_for(3, true).is_err()); // 8_USCALED
    }
}
