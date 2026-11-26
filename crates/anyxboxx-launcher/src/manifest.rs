//! The optional SHA-256 manifest of the game folder. It is a record to compare by eye or against a
//! later run, never a gate: PLAY does not wait for it. It runs in the background with progress,
//! can be cancelled, and remembers each hash by (name, size, modification time) so a file that did
//! not change is never read again.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::verify::required_files;

const HASH_BUFFER_BYTES: usize = 1024 * 1024;

/// One line of the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// The manifest in `sha256sum` layout: `<sha256>  <size>  <name>`, one file per line.
pub fn manifest_text(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}  {:>14}  {}\n", e.sha256, e.size, e.name))
        .collect()
}

/// What a hash was computed for: the file is the same when all three still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub name: String,
    pub size: u64,
    pub mtime_secs: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cache {
    pub entries: Vec<CacheEntry>,
}

impl Cache {
    /// The remembered hash of `name` when its size and time are the ones it was computed for.
    pub fn lookup(&self, name: &str, size: u64, mtime_secs: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.name == name && e.size == size && e.mtime_secs == mtime_secs)
            .map(|e| e.sha256.as_str())
    }

    /// Remembers a hash, replacing the older one of the same file.
    pub fn insert(&mut self, entry: CacheEntry) {
        self.entries.retain(|e| e.name != entry.name);
        self.entries.push(entry);
    }
}

/// Where the cache file lives, given `%LOCALAPPDATA%` and the game folder it belongs to.
pub fn cache_path(local_app_data: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    local_app_data.filter(|v| !v.is_empty()).map(|root| {
        PathBuf::from(root)
            .join("AnyXboxX")
            .join("launcher")
            .join("manifest-cache.toml")
    })
}

pub fn current_cache_path() -> Option<PathBuf> {
    cache_path(std::env::var_os("LOCALAPPDATA").as_deref())
}

fn load_cache(path: &Path) -> Cache {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_cache(path: &Path, cache: &Cache) {
    if let Ok(text) = toml::to_string_pretty(cache) {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(path, text);
    }
}

/// Size and modification time (whole seconds) of a file.
pub fn file_key(path: &Path) -> io::Result<(u64, u64)> {
    let meta = std::fs::metadata(path)?;
    let secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Ok((meta.len(), secs))
}

/// Progress shared between the worker and the window.
#[derive(Debug)]
pub struct Progress {
    pub total_bytes: AtomicU64,
    pub done_bytes: AtomicU64,
    pub files_total: AtomicUsize,
    pub files_done: AtomicUsize,
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
    pub started: Instant,
}

impl Progress {
    pub fn new() -> Self {
        Progress {
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            files_total: AtomicUsize::new(0),
            files_done: AtomicUsize::new(0),
            current: Mutex::new(String::new()),
            cancel: AtomicBool::new(false),
            started: Instant::now(),
        }
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total_bytes.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done_bytes.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }

    /// The line shown on the bar: file counter, name, speed and time left.
    pub fn describe(&self) -> String {
        let done = self.done_bytes.load(Ordering::Relaxed);
        let total = self.total_bytes.load(Ordering::Relaxed);
        let (rate, eta) = rate_and_eta(done, total, self.started.elapsed().as_secs_f64());
        let name = self.current.lock().map(|c| c.clone()).unwrap_or_default();
        format!(
            "file {} of {}: {name}, {}/s, {}",
            (self.files_done.load(Ordering::Relaxed) + 1)
                .min(self.files_total.load(Ordering::Relaxed).max(1)),
            self.files_total.load(Ordering::Relaxed),
            format_bytes(rate as u64),
            eta.map_or("time left unknown".to_string(), format_eta),
        )
    }
}

/// Bytes per second so far and the seconds left at that speed (`None` before any data).
pub fn rate_and_eta(done: u64, total: u64, elapsed_secs: f64) -> (f64, Option<f64>) {
    if elapsed_secs <= 0.0 || done == 0 {
        return (0.0, None);
    }
    let rate = done as f64 / elapsed_secs;
    (rate, Some(total.saturating_sub(done) as f64 / rate))
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn format_eta(secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    match secs {
        0..=59 => format!("{secs} s left"),
        60..=3599 => format!("{} min {} s left", secs / 60, secs % 60),
        _ => format!("{} h {} min left", secs / 3600, (secs % 3600) / 60),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done(Vec<Entry>),
    Cancelled,
    Failed(String),
}

/// The files of the manifest: the required ones that exist and every top-level `.rpf`, sorted.
pub fn list_files(dir: &Path) -> io::Result<Vec<String>> {
    let mut names: Vec<String> = required_files()
        .into_iter()
        .map(|(n, _)| n.to_string())
        .filter(|n| dir.join(n).is_file())
        .collect();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_rpf = Path::new(&name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rpf"));
        if is_rpf
            && entry.file_type()?.is_file()
            && !names
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&name))
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Hashes the files of `dir` (blocking: run it on a thread), reusing the cache and updating
/// `progress` as bytes go by. Stops with `Cancelled` as soon as `progress.cancel` is set.
pub fn run(dir: &Path, cache_file: Option<&Path>, progress: &Progress) -> Outcome {
    let names = match list_files(dir) {
        Ok(names) => names,
        Err(e) => return Outcome::Failed(format!("{}: {e}", dir.display())),
    };
    let mut keys = Vec::new();
    for name in &names {
        match file_key(&dir.join(name)) {
            Ok(key) => keys.push(key),
            Err(e) => return Outcome::Failed(format!("{name}: {e}")),
        }
    }
    progress
        .total_bytes
        .store(keys.iter().map(|k| k.0).sum(), Ordering::Relaxed);
    progress.files_total.store(names.len(), Ordering::Relaxed);
    let mut cache = cache_file.map(load_cache).unwrap_or_default();
    let mut entries = Vec::new();
    for (name, (size, mtime)) in names.iter().zip(keys) {
        if progress.cancel.load(Ordering::Relaxed) {
            persist(cache_file, &cache);
            return Outcome::Cancelled;
        }
        if let Ok(mut current) = progress.current.lock() {
            current.clone_from(name);
        }
        let sha256 = match cache.lookup(name, size, mtime) {
            Some(known) => {
                progress.done_bytes.fetch_add(size, Ordering::Relaxed);
                known.to_string()
            }
            None => match hash_file(&dir.join(name), progress) {
                Ok(Some(hash)) => hash,
                Ok(None) => {
                    persist(cache_file, &cache);
                    return Outcome::Cancelled;
                }
                Err(e) => {
                    persist(cache_file, &cache);
                    return Outcome::Failed(format!("{name}: {e}"));
                }
            },
        };
        cache.insert(CacheEntry {
            name: name.clone(),
            size,
            mtime_secs: mtime,
            sha256: sha256.clone(),
        });
        entries.push(Entry {
            name: name.clone(),
            size,
            sha256,
        });
        progress.files_done.fetch_add(1, Ordering::Relaxed);
    }
    persist(cache_file, &cache);
    Outcome::Done(entries)
}

fn persist(cache_file: Option<&Path>, cache: &Cache) {
    if let Some(path) = cache_file {
        save_cache(path, cache);
    }
}

/// The SHA-256 of a file, or `None` when cancelled midway.
fn hash_file(path: &Path, progress: &Progress) -> io::Result<Option<String>> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        progress
            .done_bytes
            .fetch_add(read as u64, Ordering::Relaxed);
    }
    Ok(Some(
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}

/// A time before `UNIX_EPOCH` or an unreadable one counts as 0 (kept for the tests).
#[allow(dead_code)]
fn epoch_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        for (name, _) in required_files() {
            std::fs::write(dir.path().join(name), name.as_bytes()).expect("write");
        }
        std::fs::write(dir.path().join("audio.rpf"), b"audio").expect("write");
        std::fs::write(dir.path().join("note.txt"), b"ignored").expect("write");
        dir
    }

    #[test]
    fn the_file_list_is_the_required_files_and_the_archives() {
        let dir = folder();
        let names = list_files(dir.path()).expect("list");
        assert!(names.contains(&"audio.rpf".to_string()));
        assert!(names.contains(&"game_scarlett_master.exe".to_string()));
        assert!(!names.contains(&"note.txt".to_string()));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn a_run_hashes_every_file_and_the_text_has_a_line_each() {
        let dir = folder();
        let progress = Progress::new();
        let Outcome::Done(entries) = run(dir.path(), None, &progress) else {
            panic!("expected Done");
        };
        assert_eq!(entries.len(), required_files().len() + 1);
        assert!(entries.iter().all(|e| e.sha256.len() == 64));
        assert_eq!(manifest_text(&entries).lines().count(), entries.len());
        assert!((progress.fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_cached_hash_is_reused_until_the_file_changes() {
        let dir = folder();
        let cache_file = dir.path().join("cache.toml");
        let first = run(dir.path(), Some(&cache_file), &Progress::new());
        let Outcome::Done(first) = first else {
            panic!("expected Done");
        };
        // Poison the cache for one file: the second run must return the poisoned hash.
        let mut cache = load_cache(&cache_file);
        let entry = cache
            .entries
            .iter_mut()
            .find(|e| e.name == "audio.rpf")
            .expect("cached");
        entry.sha256 = "f".repeat(64);
        save_cache(&cache_file, &cache);
        let Outcome::Done(second) = run(dir.path(), Some(&cache_file), &Progress::new()) else {
            panic!("expected Done");
        };
        let hash_of = |entries: &[Entry]| {
            entries
                .iter()
                .find(|e| e.name == "audio.rpf")
                .map(|e| e.sha256.clone())
        };
        assert_eq!(hash_of(&second), Some("f".repeat(64)));
        assert_ne!(hash_of(&first), hash_of(&second));
        // A changed size gives a different key, so the poisoned entry no longer applies.
        std::fs::write(dir.path().join("audio.rpf"), b"audio changed").expect("write");
        let Outcome::Done(third) = run(dir.path(), Some(&cache_file), &Progress::new()) else {
            panic!("expected Done");
        };
        assert_ne!(hash_of(&third), Some("f".repeat(64)));
    }

    #[test]
    fn cache_lookup_needs_name_size_and_time_to_match() {
        let mut cache = Cache::default();
        cache.insert(CacheEntry {
            name: "a".into(),
            size: 10,
            mtime_secs: 5,
            sha256: "h1".into(),
        });
        assert_eq!(cache.lookup("a", 10, 5), Some("h1"));
        assert_eq!(cache.lookup("a", 11, 5), None);
        assert_eq!(cache.lookup("a", 10, 6), None);
        assert_eq!(cache.lookup("b", 10, 5), None);
        cache.insert(CacheEntry {
            name: "a".into(),
            size: 11,
            mtime_secs: 5,
            sha256: "h2".into(),
        });
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.lookup("a", 11, 5), Some("h2"));
    }

    #[test]
    fn the_cache_survives_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("sub").join("c.toml");
        let mut cache = Cache::default();
        cache.insert(CacheEntry {
            name: "a".into(),
            size: 1,
            mtime_secs: 2,
            sha256: "x".into(),
        });
        save_cache(&path, &cache);
        assert_eq!(load_cache(&path), cache);
        assert_eq!(load_cache(&dir.path().join("none.toml")), Cache::default());
    }

    #[test]
    fn a_cancel_stops_the_run() {
        let dir = folder();
        let progress = Progress::new();
        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(run(dir.path(), None, &progress), Outcome::Cancelled);
    }

    #[test]
    fn a_cancel_stops_the_run_even_when_all_hashes_are_cached() {
        let dir = folder();
        let cache_file = dir.path().join("cache.toml");
        assert!(matches!(
            run(dir.path(), Some(&cache_file), &Progress::new()),
            Outcome::Done(_)
        ));
        let progress = Progress::new();
        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            run(dir.path(), Some(&cache_file), &progress),
            Outcome::Cancelled
        );
    }

    #[test]
    fn list_files_deduplicates_required_archives_case_insensitively() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("SHADERS.RPF"), b"shaders").expect("write");
        let names = list_files(dir.path()).expect("list");
        let matches: Vec<&String> = names
            .iter()
            .filter(|n| n.eq_ignore_ascii_case("shaders.rpf"))
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "got duplicate shader archive entries: {names:?}"
        );
    }

    #[test]
    fn a_missing_folder_fails_with_a_message() {
        let dir = tempfile::tempdir().expect("temp dir");
        let outcome = run(&dir.path().join("nope"), None, &Progress::new());
        assert!(matches!(outcome, Outcome::Failed(_)));
    }

    #[test]
    fn speed_and_time_left_follow_the_bytes_done() {
        assert_eq!(rate_and_eta(0, 100, 5.0), (0.0, None));
        let (rate, eta) = rate_and_eta(50, 150, 5.0);
        assert!((rate - 10.0).abs() < 1e-9);
        assert!((eta.expect("eta") - 10.0).abs() < 1e-9);
    }

    #[test]
    fn sizes_and_durations_read_well() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(format_eta(42.0), "42 s left");
        assert_eq!(format_eta(125.0), "2 min 5 s left");
        assert_eq!(format_eta(7500.0), "2 h 5 min left");
    }

    #[test]
    fn the_cache_path_is_under_local_app_data() {
        let path = cache_path(Some(std::ffi::OsStr::new("C:\\L"))).expect("path");
        assert!(path.to_string_lossy().contains("manifest-cache.toml"));
        assert_eq!(cache_path(None), None);
        assert_eq!(epoch_secs(UNIX_EPOCH), 0);
    }
}
