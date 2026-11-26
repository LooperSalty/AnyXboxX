//! The game's identity, read from its `MicrosoftGame.config` (the folder is `ANYXBOXX_GAME_DIR`,
//! which the native mode sets before starting the title). Port of `game_config.cpp`: only what the
//! runtime needs (Title ID and persistent local storage size), read as text, not as XML.

use std::path::PathBuf;

use crate::log::trace;

/// Largest config file read; a bigger one is ignored, as in the C++.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CONFIG_FILE: &str = "MicrosoftGame.config";
/// The Title ID is at most eight hexadecimal digits.
const MAX_TITLE_ID_DIGITS: usize = 8;
/// A size in megabytes is at most nine decimal digits.
#[allow(dead_code)]
const MAX_SIZE_DIGITS: usize = 9;

/// The contents of the config file as ASCII text, or empty when it is missing or unreadable.
/// UTF-16 files (with a byte order mark) keep their non-zero bytes, enough for tags and digits.
fn read_text() -> String {
    let Some(folder) = std::env::var_os("ANYXBOXX_GAME_DIR") else {
        return String::new();
    };
    let path = PathBuf::from(folder).join(CONFIG_FILE);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return String::new();
    };
    if metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
        return String::new();
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return String::new();
    };
    narrow(&bytes)
}

/// The text of `bytes`: UTF-16 (either byte order, with its mark) drops its BOM and zero bytes.
fn narrow(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] | [0xFE, 0xFF, rest @ ..] => rest
            .iter()
            .filter(|&&byte| byte != 0)
            .map(|&byte| byte as char)
            .collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// The text between `<name>` and `</name>`, or empty.
fn element_text(text: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let Some(start) = text.find(&open) else {
        return String::new();
    };
    let body = &text[start + open.len()..];
    body.find(&close)
        .map(|end| body[..end].to_string())
        .unwrap_or_default()
}

/// Package identity name from the Identity element in the game configuration.
pub fn package_name() -> String {
    attribute_text(&read_text(), "Identity", "Name")
}

pub fn package_details() -> (String, String, String, String) {
    let text = read_text();
    (
        attribute_text(&text, "Identity", "Publisher"),
        attribute_text(&text, "Identity", "Version"),
        attribute_text(&text, "ShellVisuals", "DefaultDisplayName"),
        element_text(&text, "StoreId"),
    )
}

/// Finds the opening tag `<element ...>` without matching longer element names sharing the prefix.
fn opening_tag<'a>(text: &'a str, element: &str) -> Option<&'a str> {
    let needle = format!("<{element}");
    let mut rest = text;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        let boundary = after
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/');
        if boundary {
            let tag = &rest[pos..];
            return Some(tag.split('>').next().unwrap_or(tag));
        }
        rest = after;
    }
    None
}

fn attribute_text(text: &str, element: &str, attribute: &str) -> String {
    let Some(tag) = opening_tag(text, element) else {
        return String::new();
    };
    let needle = format!("{attribute}=\"");
    let mut rest = tag;
    while let Some(pos) = rest.find(&needle) {
        let preceded_by_ws = pos > 0 && rest.as_bytes()[pos - 1].is_ascii_whitespace();
        if preceded_by_ws {
            let value = &rest[pos + needle.len()..];
            return value
                .find('"')
                .map_or_else(String::new, |end| value[..end].to_owned());
        }
        rest = &rest[pos + needle.len()..];
    }
    String::new()
}

fn parse_title_id(text: &str) -> u32 {
    let raw = element_text(text, "TitleId");
    let value = raw.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() || value.len() > MAX_TITLE_ID_DIGITS {
        return 0;
    }
    u32::from_str_radix(value, 16).unwrap_or(0)
}

fn parse_local_storage_bytes(text: &str) -> u64 {
    let raw = element_text(text, "SizeMB");
    let value = raw.trim();
    if value.is_empty() || value.len() > MAX_SIZE_DIGITS {
        return 0;
    }
    value
        .parse::<u64>()
        .map_or(0, |megabytes| megabytes * 1024 * 1024)
}

/// The Title ID declared by `<TitleId>` (hexadecimal), or 0 when it is missing or malformed.
pub fn title_id() -> u32 {
    let id = parse_title_id(&read_text());
    if id != 0 {
        trace!("[runtime] Title ID du jeu : {id:08X}");
    }
    id
}

/// The persistent local storage size declared by `<SizeMB>`, in bytes; 0 when missing.
#[allow(dead_code)] // read by the local storage object (next step of the port)
pub fn local_storage_bytes() -> u64 {
    parse_local_storage_bytes(&read_text())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_read_between_their_tags() {
        let text = "<Game><TitleId>7952987A</TitleId></Game>";
        assert_eq!(element_text(text, "TitleId"), "7952987A");
        assert_eq!(element_text(text, "SizeMB"), "");
    }

    #[test]
    fn utf16_files_lose_their_zero_bytes() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "<TitleId>AB</TitleId>".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        let narrowed = narrow(&bytes);
        assert_eq!(narrowed, "<TitleId>AB</TitleId>");
        assert_eq!(element_text(&narrowed, "TitleId"), "AB");
    }

    #[test]
    fn attributes_parse_across_newlines_tabs_and_skip_prefix_elements() {
        let text = "<Game>\n\
            <IdentityExtra Name=\"Wrong\" />\n\
            <Identity\n\
            \tName=\"MyGame\"\n\
            \nPublisher=\"CN=Dev\"\n\
            \tVersion=\"1.2.3.4\" />\n\
            </Game>";
        assert_eq!(attribute_text(text, "Identity", "Name"), "MyGame");
        assert_eq!(attribute_text(text, "Identity", "Publisher"), "CN=Dev");
        assert_eq!(attribute_text(text, "Identity", "Version"), "1.2.3.4");
    }

    #[test]
    fn title_id_and_storage_size_trim_whitespace_and_hex_prefix() {
        let text = "<Game><TitleId> 0x7952987A </TitleId><SizeMB>\n  128 \t</SizeMB></Game>";
        assert_eq!(parse_title_id(text), 0x7952_987A);
        assert_eq!(parse_local_storage_bytes(text), 128 * 1024 * 1024);
    }

    #[test]
    fn the_title_id_and_storage_size_come_from_the_folder_of_the_game() {
        let folder = std::env::temp_dir().join(format!("axg-config-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(CONFIG_FILE),
            "<Game><TitleId>1234ABCD</TitleId><PersistentLocalStorage><SizeMB>64</SizeMB></PersistentLocalStorage></Game>",
        )
        .unwrap();
        // SAFETY: this test is the only one that sets this variable, and no other thread reads it.
        unsafe { std::env::set_var("ANYXBOXX_GAME_DIR", &folder) };
        assert!(read_text().contains("1234ABCD"));
        assert_eq!(element_text(&read_text(), "TitleId"), "1234ABCD");
        assert_eq!(local_storage_bytes(), 64 * 1024 * 1024);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
