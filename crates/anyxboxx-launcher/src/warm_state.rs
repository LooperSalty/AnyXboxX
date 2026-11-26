//! Is the shader cache ready for this VIctory build and this environment ("smart warm")?
//!
//! After a successful `victory warm` the launcher writes a marker holding a fingerprint: the
//! build of victory.exe, the VICTORY_* variables (the cache key depends on them) and the game's
//! archives. The next launch compares the fingerprint of the moment with the marker: a missing,
//! unreadable or different marker means the warm runs again before the game starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyxboxx::run::Env;

pub const MARKER_FILE: &str = "warm-state.txt";
/// Variables that do not change the shader cache: the game folder and the log level.
const NOT_IN_KEY: [&str; 2] = ["VICTORY_GAME_DIR", "VICTORY_LOG"];

/// `%LOCALAPPDATA%\AnyXboxX\launcher\warm-state.txt`, `None` when LOCALAPPDATA is not set or empty.
pub fn marker_path(local_app_data: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    local_app_data.filter(|v| !v.is_empty()).map(|local| {
        PathBuf::from(local)
            .join("AnyXboxX")
            .join("launcher")
            .join(MARKER_FILE)
    })
}

/// FNV-1a, 64 bits: a stable fingerprint without a new dependency (not a security hash).
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Identity of victory.exe: its size and modification time in seconds. A missing file (not built
/// yet) has its own identity, so the first build always counts as a change.
pub fn build_id(exe: &Path) -> String {
    match std::fs::metadata(exe) {
        Ok(meta) => {
            let secs = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            format!("{}:{secs}", meta.len())
        }
        Err(_) => "not-built".to_string(),
    }
}

/// The cache-relevant variables, sorted and deduplicated (last entry wins), one `KEY=VALUE` per line.
pub fn env_key(env: &Env) -> String {
    let mut effective: BTreeMap<&str, String> = BTreeMap::new();
    for (key, value) in env {
        if key.starts_with("VICTORY_") && !NOT_IN_KEY.contains(&key.as_str()) {
            effective.insert(key.as_str(), value.to_string_lossy().into_owned());
        }
    }
    effective
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `name:size:modified` of each archive, in the order given.
pub fn archive_identities(archives: &[PathBuf]) -> Vec<String> {
    archives
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            match std::fs::metadata(path) {
                Ok(meta) => {
                    let secs = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs());
                    format!("{name}:{}:{secs}", meta.len())
                }
                Err(_) => format!("{name}:missing"),
            }
        })
        .collect()
}

/// The fingerprint of build, environment and archives, as a 16-digit hex string.
pub fn fingerprint(build: &str, env: &Env, archives: &[String]) -> String {
    let text = format!(
        "build={build}\nenv:\n{}\narchives:\n{}",
        env_key(env),
        archives.join("\n")
    );
    format!("{:016x}", fnv1a(&text))
}

/// Whether the warm must run: always when asked to, otherwise when the marker is missing or
/// holds another fingerprint (`marker` is the file's text, `None` when it cannot be read).
pub fn needs_warm(always: bool, marker: Option<&str>, fingerprint: &str) -> bool {
    always || marker.map(str::trim) != Some(fingerprint)
}

/// Reads the marker file; `None` when it is missing or unreadable.
pub fn read_marker(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Writes the fingerprint after a successful warm, creating the folder when needed.
pub fn write_marker(path: &Path, fingerprint: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{fingerprint}\n"))
}

/// Deletes the marker (the shader cache was cleaned): the next launch warms again. A missing
/// marker is not an error.
pub fn forget_marker(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn a_forgotten_marker_makes_the_next_launch_warm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("warm-state.txt");
        write_marker(&path, "fp").expect("write");
        assert!(!needs_warm(false, read_marker(&path).as_deref(), "fp"));
        forget_marker(&path).expect("forget");
        assert!(needs_warm(false, read_marker(&path).as_deref(), "fp"));
        forget_marker(&path).expect("a missing marker is fine");
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), OsString::from(*v)))
            .collect()
    }

    fn archives() -> Vec<String> {
        vec!["a.rpf:10:5".to_string()]
    }

    #[test]
    fn a_missing_marker_means_the_warm_runs() {
        let fp = fingerprint("1:2", &env(&[("VICTORY_RELAY", "2")]), &archives());
        assert!(needs_warm(false, None, &fp));
    }

    #[test]
    fn the_same_fingerprint_in_the_marker_skips_the_warm() {
        let fp = fingerprint("1:2", &env(&[("VICTORY_RELAY", "2")]), &archives());
        assert!(!needs_warm(false, Some(&format!("{fp}\n")), &fp));
    }

    #[test]
    fn a_different_marker_means_the_warm_runs() {
        let fp = fingerprint("1:2", &env(&[]), &archives());
        assert!(needs_warm(false, Some("0000000000000000"), &fp));
    }

    #[test]
    fn always_re_warm_ignores_a_current_marker() {
        let fp = fingerprint("1:2", &env(&[]), &archives());
        assert!(needs_warm(true, Some(&fp), &fp));
    }

    #[test]
    fn a_new_victory_build_changes_the_fingerprint() {
        let before = fingerprint("100:1", &env(&[]), &archives());
        let after = fingerprint("100:2", &env(&[]), &archives());
        assert_ne!(before, after);
    }

    #[test]
    fn a_changed_cache_variable_changes_the_fingerprint() {
        let before = fingerprint("1:1", &env(&[("VICTORY_RELAY", "2")]), &archives());
        let after = fingerprint("1:1", &env(&[("VICTORY_RELAY", "0")]), &archives());
        assert_ne!(before, after);
    }

    #[test]
    fn the_game_folder_and_log_level_do_not_change_the_fingerprint() {
        let base = fingerprint("1:1", &env(&[("VICTORY_RELAY", "2")]), &archives());
        let moved = fingerprint(
            "1:1",
            &env(&[
                ("VICTORY_RELAY", "2"),
                ("VICTORY_GAME_DIR", "D:\\Other"),
                ("VICTORY_LOG", "debug"),
            ]),
            &archives(),
        );
        assert_eq!(base, moved);
    }

    #[test]
    fn non_victory_variables_are_not_part_of_the_key() {
        assert_eq!(env_key(&env(&[("PATH", "x"), ("VKD3D_CONFIG", "y")])), "");
        assert_eq!(
            env_key(&env(&[("VICTORY_B", "2"), ("VICTORY_A", "1")])),
            "VICTORY_A=1\nVICTORY_B=2"
        );
    }

    #[test]
    fn a_changed_archive_changes_the_fingerprint() {
        let before = fingerprint("1:1", &env(&[]), &["a.rpf:10:5".to_string()]);
        let after = fingerprint("1:1", &env(&[]), &["a.rpf:11:5".to_string()]);
        assert_ne!(before, after);
    }

    #[test]
    fn the_marker_round_trips_through_its_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join("AnyXboxX")
            .join("launcher")
            .join(MARKER_FILE);
        assert!(read_marker(&path).is_none());
        write_marker(&path, "abc").expect("write");
        let text = read_marker(&path).expect("read");
        assert!(!needs_warm(false, Some(&text), "abc"));
    }

    #[test]
    fn the_marker_lives_under_appdata() {
        let path = marker_path(Some(std::ffi::OsStr::new(r"C:\L"))).expect("path");
        assert!(path.ends_with(Path::new("AnyXboxX").join("launcher").join(MARKER_FILE)));
        assert!(marker_path(None).is_none());
        assert!(marker_path(Some(std::ffi::OsStr::new(""))).is_none());
    }

    #[test]
    fn overridden_victory_variables_keep_only_the_last_value() {
        let overridden = env(&[
            ("VICTORY_RELAY", "2"),
            ("VICTORY_A", "1"),
            ("VICTORY_RELAY", "0"),
        ]);
        let direct = env(&[("VICTORY_A", "1"), ("VICTORY_RELAY", "0")]);
        assert_eq!(env_key(&overridden), "VICTORY_A=1\nVICTORY_RELAY=0");
        assert_eq!(
            fingerprint("1:1", &overridden, &archives()),
            fingerprint("1:1", &direct, &archives())
        );
    }

    #[test]
    fn a_missing_victory_exe_is_its_own_build_identity() {
        assert_eq!(
            build_id(Path::new("does-not-exist/victory.exe")),
            "not-built"
        );
    }
}
