//! The PE32+ modules of a package folder and their import and export names, as
//! `emu/bench.py` and `emu/translation.py` list them. Unreadable files give empty tables, like
//! the Python code that catches pefile's errors.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use victory_pe::{ImageInfo, ImportSymbol};

use crate::dll_name;

const DOS_HEADER_SIZE: usize = 0x40;
const PE_SIGNATURE: &[u8] = b"PE\0\0";
const PE32_PLUS_MAGIC: [u8; 2] = 0x20B_u16.to_le_bytes();
const E_LFANEW: usize = 0x3C;
const MAGIC_OFFSET: usize = 24;
const NT_PROBE_SIZE: usize = MAGIC_OFFSET + 2;
/// `PATTERNS` of bench.py: `*.dll` and `*.exe`, matched case-insensitively as on Windows.
const EXTENSIONS: [&str; 2] = ["dll", "exe"];

/// `bench.is_pe32_plus`: true for an x86-64 module (a 32-bit or .NET launcher is not one).
pub fn is_pe32_plus(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut dos = [0u8; DOS_HEADER_SIZE];
    if file.read_exact(&mut dos).is_err() || &dos[..2] != b"MZ" {
        return false;
    }
    let pe = u32::from_le_bytes(dos[E_LFANEW..E_LFANEW + 4].try_into().expect("4 bytes"));
    if file.seek(SeekFrom::Start(u64::from(pe))).is_err() {
        return false;
    }
    let mut nt = [0u8; NT_PROBE_SIZE];
    if file.read_exact(&mut nt).is_err() {
        return false;
    }
    &nt[..4] == PE_SIGNATURE && nt[MAGIC_OFFSET..MAGIC_OFFSET + 2] == PE32_PLUS_MAGIC
}

fn has_module_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// `translation._modules` / `bench.run`: the PE32+ modules of `folder`, sorted by lower-case name.
pub fn package_modules(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| has_module_extension(p) && p.is_file() && is_pe32_plus(p))
        .collect();
    paths.sort_by_key(|p| file_name(p).to_lowercase());
    paths
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parse(path: &Path) -> Option<ImageInfo> {
    ImageInfo::parse(&std::fs::read(path).ok()?).ok()
}

/// How the Python reports write an imported function: its name, or `#ordinal`.
pub fn symbol_name(symbol: &ImportSymbol) -> String {
    symbol.to_string()
}

/// `translation._imported_names`: {normalised DLL: names}, ordinals written `#n`.
pub fn imported_names(path: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in parse(path).map(|i| i.imports).unwrap_or_default() {
        let names = found.entry(dll_name(&module.dll)).or_default();
        names.extend(module.entries.iter().map(|e| symbol_name(&e.symbol)));
    }
    found
}

/// `translation._exported_names`: the named exports, forwarders included.
pub fn exported_names(path: &Path) -> BTreeSet<String> {
    parse(path)
        .map(|info| info.exports.into_iter().filter_map(|e| e.name).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn junk_is_not_a_module() {
        let dir = std::env::temp_dir().join(format!("anyxboxx-modules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("notes.dll");
        std::fs::write(&junk, b"MZ not a PE").unwrap();
        assert!(!is_pe32_plus(&junk));
        assert!(!is_pe32_plus(&dir.join("absent.dll")));
        assert!(package_modules(&dir).is_empty());
        assert!(imported_names(&junk).is_empty() && exported_names(&junk).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extensions_match_without_case() {
        assert!(has_module_extension(Path::new("GAME.EXE")));
        assert!(has_module_extension(Path::new("a.Dll")));
        assert!(!has_module_extension(Path::new("a.sys")));
    }

    #[test]
    fn pe32_plus_detection_handles_large_dos_stubs_and_bad_offsets() {
        let dir = std::env::temp_dir().join(format!("anyxboxx-modules-pe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let make = |e_lfanew: u32, magic: u16, size: usize| {
            let mut bytes = vec![0u8; size];
            bytes[..2].copy_from_slice(b"MZ");
            bytes[E_LFANEW..E_LFANEW + 4].copy_from_slice(&e_lfanew.to_le_bytes());
            let pe = e_lfanew as usize;
            if pe + NT_PROBE_SIZE <= bytes.len() {
                bytes[pe..pe + 4].copy_from_slice(PE_SIGNATURE);
                bytes[pe + MAGIC_OFFSET..pe + MAGIC_OFFSET + 2]
                    .copy_from_slice(&magic.to_le_bytes());
            }
            bytes
        };

        let standard = dir.join("standard.dll");
        std::fs::write(&standard, make(0x80, 0x20B, 0x200)).unwrap();
        assert!(is_pe32_plus(&standard));

        // NT header past the old 0x400 probe window.
        let large_stub = dir.join("large_stub.dll");
        std::fs::write(&large_stub, make(0x500, 0x20B, 0x600)).unwrap();
        assert!(is_pe32_plus(&large_stub));

        let pe32 = dir.join("pe32.dll");
        std::fs::write(&pe32, make(0x80, 0x10B, 0x200)).unwrap();
        assert!(!is_pe32_plus(&pe32));

        let huge_offset = dir.join("huge_offset.dll");
        std::fs::write(&huge_offset, make(u32::MAX, 0x20B, 0x80)).unwrap();
        assert!(!is_pe32_plus(&huge_offset));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
