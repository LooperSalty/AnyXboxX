//! The quick check of the game folder: the files VIctory needs are present and structurally whole.
//! Only the first kilobytes of each file are read (PE headers and section bounds, the RPF7 header
//! and tables), so it ends in a moment even when the archives weigh 130 GB. The hashes are a
//! separate, optional job (`manifest`). The launcher ships no expected hashes or sizes.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::format::{self, HEAD_BYTES};

pub const GAME_EXE: &str = "game_scarlett_master.exe";
pub const SHADERS_RPF: &str = "shaders.rpf";
/// The DLLs `--precompile` translates with the game (see anyxboxx::run::PRECOMPILED_DLLS).
pub const PRECOMPILE_DLLS: [&str; 2] = ["XboxBvhBuilder.dll", "amd_fidelityfx_upscaler_xbox.dll"];

/// Files the launcher needs in the game folder, with the reason for each.
pub fn required_files() -> Vec<(&'static str, &'static str)> {
    let mut files = vec![(GAME_EXE, "the game executable")];
    files.push((SHADERS_RPF, "shader pack, needed by shader preloading"));
    files.extend(
        PRECOMPILE_DLLS
            .iter()
            .map(|name| (*name, "translated with the game by shader preloading")),
    );
    files
}

/// Why a file is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Missing,
    /// Present but not whole: empty, or cut, or not the format its name promises.
    Corrupted(String),
    /// The file could not be read.
    Unreadable(String),
}

/// The result of the quick check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Required files with their problem, in `required_files` order.
    pub problems: Vec<(String, Problem)>,
}

impl Report {
    /// True when every required file is present and whole.
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks `dir`. Errors only when `dir` itself is not a folder; a bad file is a problem in the report.
pub fn verify_folder(dir: &Path) -> io::Result<Report> {
    if !dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("'{}' is not a folder", dir.display()),
        ));
    }
    let mut report = Report::default();
    for (name, _) in required_files() {
        if let Err(problem) = check_file(&dir.join(name), name) {
            report.problems.push((name.to_string(), problem));
        }
    }
    Ok(report)
}

/// Checks one required file by its size and its first bytes.
fn check_file(path: &Path, name: &str) -> Result<(), Problem> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) | Err(_) => return Err(Problem::Missing),
    };
    if meta.len() == 0 {
        return Err(Problem::Corrupted("the file is empty".to_string()));
    }
    let head = read_head(path).map_err(|e| Problem::Unreadable(e.to_string()))?;
    let structure = if is_ext(name, "exe") || is_ext(name, "dll") {
        format::check_pe(&head, meta.len())
    } else if is_ext(name, "rpf") {
        format::check_rpf7(&head, meta.len())
    } else {
        Ok(())
    };
    structure.map_err(Problem::Corrupted)
}

fn is_ext(name: &str, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn read_head(path: &Path) -> io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(HEAD_BYTES);
    File::open(path)?
        .take(HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PE image of 0x300 bytes whose one section ends at the end of the file.
    fn good_pe() -> Vec<u8> {
        let mut b = vec![0u8; 0x300];
        b[0..2].copy_from_slice(b"MZ");
        b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        b[0x94..0x96].copy_from_slice(&0u16.to_le_bytes());
        // One section header at 0x98 (after the 20-byte COFF header that starts at 0x84):
        // SizeOfRawData at +16, PointerToRawData at +20, ending at the file's last byte.
        b[0xA8..0xAC].copy_from_slice(&0x100u32.to_le_bytes());
        b[0xAC..0xB0].copy_from_slice(&0x200u32.to_le_bytes());
        b
    }

    /// A RPF7 header with no entries and no names: 16 bytes of tables.
    fn good_rpf() -> Vec<u8> {
        let mut b = b"RPF7".to_vec();
        b.extend_from_slice(&[0u8; 12]);
        b
    }

    /// Writes a complete game folder: every required file whole, plus one extra archive.
    fn game_folder(dir: &Path) {
        for (name, _) in required_files() {
            let bytes = if is_ext(name, "rpf") {
                good_rpf()
            } else {
                good_pe()
            };
            std::fs::write(dir.join(name), bytes).expect("write");
        }
        std::fs::write(dir.join("audio.rpf"), good_rpf()).expect("write");
    }

    #[test]
    fn a_complete_folder_passes_the_quick_check() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        let report = verify_folder(dir.path()).expect("folder");
        assert!(report.is_ok(), "{:?}", report.problems);
    }

    #[test]
    fn a_missing_executable_is_the_first_problem() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        std::fs::remove_file(dir.path().join(GAME_EXE)).expect("remove");
        let report = verify_folder(dir.path()).expect("folder");
        assert_eq!(
            report.problems,
            vec![(GAME_EXE.to_string(), Problem::Missing)]
        );
    }

    #[test]
    fn a_truncated_pe_is_corrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        let mut cut = good_pe();
        cut.truncate(0x90);
        std::fs::write(dir.path().join(GAME_EXE), cut).expect("write");
        let report = verify_folder(dir.path()).expect("folder");
        assert_eq!(report.problems.len(), 1);
        assert!(matches!(&report.problems[0].1, Problem::Corrupted(_)));
    }

    #[test]
    fn an_archive_with_only_its_header_passes_without_reading_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        // The header alone is enough for the quick check: nothing else is read.
        std::fs::write(dir.path().join(SHADERS_RPF), good_rpf()).expect("write");
        assert!(verify_folder(dir.path()).expect("folder").is_ok());
    }

    #[test]
    fn a_missing_file_is_reported_as_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        std::fs::remove_file(dir.path().join(SHADERS_RPF)).expect("remove");
        let report = verify_folder(dir.path()).expect("folder");
        assert_eq!(
            report.problems,
            vec![(SHADERS_RPF.to_string(), Problem::Missing)]
        );
    }

    #[test]
    fn a_truncated_archive_is_reported_as_corrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        // Entry table declares 2 entries (32 bytes) but the file ends after its 16-byte header.
        let mut cut = b"RPF7".to_vec();
        cut.extend_from_slice(&2u32.to_le_bytes());
        cut.extend_from_slice(&[0u8; 8]);
        std::fs::write(dir.path().join(SHADERS_RPF), cut).expect("write");
        let report = verify_folder(dir.path()).expect("folder");
        assert_eq!(report.problems.len(), 1);
        assert!(matches!(&report.problems[0].1, Problem::Corrupted(m) if m.contains("truncated")));
    }

    #[test]
    fn an_empty_executable_is_corrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        game_folder(dir.path());
        std::fs::write(dir.path().join(GAME_EXE), b"").expect("write");
        let report = verify_folder(dir.path()).expect("folder");
        assert!(matches!(&report.problems[0].1, Problem::Corrupted(m) if m.contains("empty")));
    }

    #[test]
    fn a_folder_that_does_not_exist_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(verify_folder(&dir.path().join("nope")).is_err());
    }
}
