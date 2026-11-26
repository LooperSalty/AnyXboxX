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

/// The text of `bytes`: UTF-16 (either byte order, with its mark) loses its zero bytes.
fn narrow(bytes: &[u8]) -> String {
    let utf16 = matches!(bytes, [0xFF, 0xFE, ..] | [0xFE, 0xFF, ..]);
    if utf16 {
        bytes
            .iter()
            .filter(|&&byte| byte != 0)
            .map(|&byte| byte as char)
            .collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
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

fn attribute_text(text: &str, element: &str, attribute: &str) -> String {
    let Some(start) = text.find(&format!("<{element}")) else {
        return String::new();
    };
    let tag = &text[start..];
    let tag = tag.split('>').next().unwrap_or(tag);
    let key = format!(" {attribute}=\"");
    let Some(start) = tag.find(&key) else {
        return String::new();
    };
    let value = &tag[start + key.len()..];
    value
        .find('"')
        .map_or_else(String::new, |end| value[..end].to_owned())
}

/// The Title ID declared by `<TitleId>` (hexadecimal), or 0 when it is missing or malformed.
pub fn title_id() -> u32 {
    let value = element_text(&read_text(), "TitleId");
    if value.is_empty() || value.len() > MAX_TITLE_ID_DIGITS {
        return 0;
    }
    match u32::from_str_radix(&value, 16) {
        Ok(id) => {
            trace!("[runtime] Title ID du jeu : {id:08X}");
            id
        }
        Err(_) => 0,
    }
}

/// The persistent local storage size declared by `<SizeMB>`, in bytes; 0 when missing.
#[allow(dead_code)] // read by the local storage object (next step of the port)
pub fn local_storage_bytes() -> u64 {
    let value = element_text(&read_text(), "SizeMB");
    if value.is_empty() || value.len() > MAX_SIZE_DIGITS {
        return 0;
    }
    value
        .parse::<u64>()
        .map_or(0, |megabytes| megabytes * 1024 * 1024)
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
        assert_eq!(element_text(&narrow(&bytes), "TitleId"), "AB");
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
