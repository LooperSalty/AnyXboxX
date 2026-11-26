//! COM identifiers as the runtime compares them, and their text form (`{XXXXXXXX-XXXX-…}`).

use std::fmt;

/// A COM interface or provider identifier, laid out like Windows' `GUID`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    /// Parses `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` (braces optional). `None` if malformed.
    pub fn parse(text: &str) -> Option<Guid> {
        let text = text.trim().trim_start_matches('{').trim_end_matches('}');
        let hex: String = text.chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 || text.len() != 36 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let field = |range: std::ops::Range<usize>| u64::from_str_radix(&hex[range], 16).ok();
        let mut data4 = [0u8; 8];
        for (index, byte) in data4.iter_mut().enumerate() {
            *byte = field(16 + 2 * index..18 + 2 * index)? as u8;
        }
        Some(Guid {
            data1: field(0..8)? as u32,
            data2: field(8..12)? as u16,
            data3: field(12..16)? as u16,
            data4,
        })
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let d = self.data4;
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            self.data1, self.data2, self.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_text_form_of_the_runtime_tables() {
        let guid = Guid::parse("073B7DCB-1FCF-4030-94BE-E3C9EB623428").unwrap();
        assert_eq!(guid.data1, 0x073B_7DCB);
        assert_eq!(guid.data2, 0x1FCF);
        assert_eq!(guid.data3, 0x4030);
        assert_eq!(guid.data4, [0x94, 0xBE, 0xE3, 0xC9, 0xEB, 0x62, 0x34, 0x28]);
        assert_eq!(guid.to_string(), "{073B7DCB-1FCF-4030-94BE-E3C9EB623428}");
    }

    #[test]
    fn rejects_malformed_text() {
        assert_eq!(Guid::parse("073B7DCB-1FCF-4030-94BE"), None);
        assert_eq!(Guid::parse("073B7DCB-1FCF-4030-94BE-E3C9EB62342Z"), None);
        assert_eq!(Guid::parse(""), None);
    }
}
