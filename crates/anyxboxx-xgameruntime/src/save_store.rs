//! Save storage on the PC's disk (C++ `save_store.cpp`): containers, blobs, the simulated quota.
//!
//! `<root>\<TitleId>\saves\<configuration>\<XUID>\<container>\<blob>.blob`, where the root is
//! `ANYXBOXX_SAVE_DIR`, else `%LOCALAPPDATA%\AnyXboxX`. Names the game gives are encoded: every
//! byte outside `[A-Za-z0-9_-]` becomes `%XX`, so no name can leave its folder.
//!
//! Every function takes the folders it works in, so the tests run in a temporary folder and never
//! touch the environment.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::log::trace;
use crate::object::{E_FAIL, HResult, S_OK};

/// Simulated quota per configuration and user, in bytes.
pub const QUOTA_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_CONTAINER_NAME: usize = 255;
pub const MAX_DISPLAY_NAME: usize = 127;
pub const MAX_BLOB_NAME: usize = 64;
pub const MAX_BLOB_BYTES: usize = 16 * 1024 * 1024;

/// The documented GameSave errors (XGameSave.h), as the C++ runtime returns them.
pub const E_GS_INVALID_CONTAINER_NAME: HResult = 0x8083_0001_u32 as i32;
pub const E_GS_UPDATE_TOO_BIG: HResult = 0x8083_0005_u32 as i32;
pub const E_GS_QUOTA_EXCEEDED: HResult = 0x8083_0006_u32 as i32;
pub const E_GS_PROVIDED_BUFFER_TOO_SMALL: HResult = 0x8083_0007_u32 as i32;
pub const E_GS_BLOB_NOT_FOUND: HResult = 0x8083_0008_u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_PATH_NOT_FOUND): a folder of the save store cannot be created.
pub const E_PATH_NOT_FOUND: HResult = 0x8007_0003_u32 as i32;

const BLOB_SUFFIX: &str = ".blob";
const DISPLAY_FILE: &str = "~display";
/// The display name is read up to this many bytes, as the C++ does.
const DISPLAY_READ_LIMIT: u64 = 256;

/// One blob of a container: its name as the game wrote it, and its size (32 bits, as the title sees it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    pub name: Vec<u8>,
    pub size: u32,
}

/// One container: names, blob count and total size, and the time of its last write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerInfo {
    pub name: Vec<u8>,
    pub display_name: Vec<u8>,
    pub blob_count: u32,
    pub total_size: u64,
    /// Seconds since 1970, as `time_t`.
    pub last_modified: i64,
}

/// One write (`erase` false) or deletion (`erase` true) of an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOperation {
    pub blob: Vec<u8>,
    pub erase: bool,
    pub data: Vec<u8>,
}

/// The save root: `ANYXBOXX_SAVE_DIR`, else `%LOCALAPPDATA%\AnyXboxX`; `None` when neither is set.
pub fn root_folder() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("ANYXBOXX_SAVE_DIR").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    env::var_os("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("AnyXboxX"))
}

/// The save root and the Title ID of the running game; `None` when no root is configured.
pub fn title_root() -> Option<(PathBuf, u32)> {
    root_folder().map(|root| (root, crate::game_config::title_id()))
}

/// The local storage folder of the game (`local`), created on demand; `None` on failure.
pub fn local_storage_folder(root: &Path, title_id: u32) -> Option<PathBuf> {
    ensure_folder(title_folder(root, title_id).join("local"))
}

/// Whether a name the game gave is usable: not empty, at most `max` bytes.
pub fn valid_name(name: &[u8], max: usize) -> bool {
    !name.is_empty() && name.len() <= max
}

/// Encodes a game name into a file name: `[A-Za-z0-9_-]` stay, every other byte becomes `%XX`.
pub fn encode(name: &[u8]) -> String {
    let mut out = String::with_capacity(name.len());
    for &byte in name {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Inverse of [`encode`]. A malformed escape decodes to a zero byte, as the C++ `wcstoul` does.
pub fn decode(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let digits = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            out.push(u8::from_str_radix(digits, 16).unwrap_or(0));
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

/// The title folder, created on demand.
pub fn title_folder(root: &Path, title_id: u32) -> PathBuf {
    root.join(format!("{title_id:08X}"))
}

/// The provider folder of a configuration and user (`saves`), created on demand; `None` on failure.
pub fn provider_folder(
    root: &Path,
    title_id: u32,
    configuration: &[u8],
    xuid: u64,
) -> Option<PathBuf> {
    ensure_folder(
        title_folder(root, title_id)
            .join("saves")
            .join(encode(configuration))
            .join(format!("{xuid:016X}")),
    )
}

/// The folder of the plain files of XGameSaveFiles (`files`), created on demand.
pub fn files_folder(
    root: &Path,
    title_id: u32,
    configuration: &[u8],
    xuid: u64,
) -> Option<PathBuf> {
    ensure_folder(
        title_folder(root, title_id)
            .join("files")
            .join(encode(configuration))
            .join(format!("{xuid:016X}")),
    )
}

/// The folder of one container of a provider.
pub fn container_folder(provider: &Path, name: &[u8]) -> PathBuf {
    provider.join(encode(name))
}

/// The containers of a provider whose names start with `prefix` (all of them when `None`).
pub fn list_containers(provider: &Path, prefix: Option<&[u8]>) -> Vec<ContainerInfo> {
    let Ok(entries) = fs::read_dir(provider) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = decode(&entry.file_name().to_string_lossy());
            if !prefix.is_none_or(|prefix| name.starts_with(prefix)) {
                return None;
            }
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
            Some(describe(&entry.path(), name, modified))
        })
        .collect()
}

/// The container `name` of a provider, if it exists.
pub fn find_container(provider: &Path, name: &[u8]) -> Option<ContainerInfo> {
    let folder = container_folder(provider, name);
    let meta = fs::metadata(&folder).ok().filter(|meta| meta.is_dir())?;
    Some(describe(&folder, name.to_vec(), meta.modified().ok()))
}

/// Deletes a container and its blobs. A missing container is not an error, as in the C++.
pub fn delete_container(provider: &Path, name: &[u8]) -> HResult {
    let folder = container_folder(provider, name);
    if let Ok(entries) = fs::read_dir(&folder) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
                // As the C++: a file that cannot be deleted is left for the folder removal to report.
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    match fs::remove_dir(&folder) {
        Ok(()) => S_OK,
        Err(error) if error.kind() == io::ErrorKind::NotFound => S_OK,
        Err(error) => hresult_of(&error),
    }
}

/// The blobs of a container whose names start with `prefix` (all of them when `None`).
pub fn list_blobs(container: &Path, prefix: Option<&[u8]>) -> Vec<BlobInfo> {
    let Ok(entries) = fs::read_dir(container) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().to_string_lossy().into_owned();
            let stem = file.strip_suffix(BLOB_SUFFIX)?;
            let name = decode(stem);
            if !prefix.is_none_or(|prefix| name.starts_with(prefix)) {
                return None;
            }
            // The title reads the low 32 bits of the size, as the C++ `nFileSizeLow`.
            let size = entry.metadata().ok()?.len() as u32;
            Some(BlobInfo { name, size })
        })
        .collect()
}

/// The data of one blob, or `E_GS_BLOB_NOT_FOUND`.
pub fn read_blob(container: &Path, name: &[u8]) -> Result<Vec<u8>, HResult> {
    let mut file = fs::File::open(blob_path(container, name)).map_err(|_| E_GS_BLOB_NOT_FOUND)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|error| hresult_of(&error))?;
    Ok(data)
}

/// The bytes a folder uses on disk, sub-folders included.
pub fn used_bytes(folder: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(folder) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => used_bytes(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// The checks an update passes before anything is written: its size, then the quota.
pub fn check_update(used: u64, written: u64) -> Result<(), HResult> {
    if written > MAX_BLOB_BYTES as u64 {
        Err(E_GS_UPDATE_TOO_BIG)
    } else if used.saturating_add(written) > QUOTA_BYTES {
        Err(E_GS_QUOTA_EXCEEDED)
    } else {
        Ok(())
    }
}

/// Applies an update to a container, then writes its display name. The checks run first, so a
/// refused update leaves the folder as it was.
pub fn apply_update(
    provider: &Path,
    container: &Path,
    display_name: &[u8],
    operations: &[UpdateOperation],
) -> HResult {
    let written: u64 = operations
        .iter()
        .filter(|op| !op.erase)
        .fold(0u64, |acc, op| acc.saturating_add(op.data.len() as u64));
    if let Err(error) = check_update(used_bytes(provider), written) {
        return error;
    }
    if let Err(error) = fs::create_dir_all(container) {
        return hresult_of(&error);
    }
    for operation in operations {
        let path = blob_path(container, &operation.blob);
        if operation.erase {
            let _ = fs::remove_file(&path);
        } else if let Err(error) = write_atomically(&path, &operation.data) {
            return hresult_of(&error);
        }
    }
    if let Err(error) = write_atomically(&container.join(DISPLAY_FILE), display_name) {
        return hresult_of(&error);
    }
    trace!(
        "[sauvegarde] {} opération(s) écrite(s) dans {}",
        operations.len(),
        container.display()
    );
    S_OK
}

/// HRESULT_FROM_WIN32 of an I/O error (`E_FAIL` when the error has no Windows code).
pub fn hresult_of(error: &io::Error) -> HResult {
    match error.raw_os_error() {
        Some(code) if code > 0 => (((code as u32) & 0xFFFF) | 0x8007_0000) as HResult,
        _ => E_FAIL,
    }
}

fn blob_path(container: &Path, name: &[u8]) -> PathBuf {
    container.join(format!("{}{BLOB_SUFFIX}", encode(name)))
}

fn ensure_folder(folder: PathBuf) -> Option<PathBuf> {
    fs::create_dir_all(&folder).ok().map(|()| folder)
}

fn describe(folder: &Path, name: Vec<u8>, modified: Option<SystemTime>) -> ContainerInfo {
    let blobs = list_blobs(folder, None);
    ContainerInfo {
        name,
        display_name: read_display(folder),
        blob_count: blobs.len() as u32,
        total_size: blobs.iter().map(|blob| u64::from(blob.size)).sum(),
        last_modified: modified.map_or(0, unix_seconds),
    }
}

fn read_display(folder: &Path) -> Vec<u8> {
    let Ok(file) = fs::File::open(folder.join(DISPLAY_FILE)) else {
        return Vec::new();
    };
    let mut text = Vec::new();
    let _ = file.take(DISPLAY_READ_LIMIT).read_to_end(&mut text);
    text
}

fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => after.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    }
}

/// Writes a temporary file, then renames it over `path` (replacing it), as MoveFileEx does.
fn write_atomically(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder under the system temporary folder, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let path = env::temp_dir().join(format!("axsave-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("temporary folder");
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(blob: &[u8], data: &[u8]) -> UpdateOperation {
        UpdateOperation {
            blob: blob.to_vec(),
            erase: false,
            data: data.to_vec(),
        }
    }

    #[test]
    fn plain_bytes_stay_and_every_other_byte_is_escaped() {
        assert_eq!(encode(b"a-b_C9"), "a-b_C9");
        assert_eq!(encode(b"x/y~."), "x%2Fy%7E%2E");
        assert_eq!(encode(b"\0"), "%00");
    }

    #[test]
    fn decode_inverts_encode_for_every_byte() {
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&all)), all);
        assert_eq!(decode("%ZZa"), vec![0, b'a']);
    }

    #[test]
    fn names_are_refused_when_empty_or_too_long() {
        assert!(!valid_name(b"", 8));
        assert!(valid_name(b"12345678", 8));
        assert!(!valid_name(b"123456789", 8));
    }

    #[test]
    fn an_update_over_the_blob_size_limit_is_refused_before_writing() {
        assert_eq!(check_update(0, MAX_BLOB_BYTES as u64), Ok(()));
        assert_eq!(
            check_update(0, MAX_BLOB_BYTES as u64 + 1),
            Err(E_GS_UPDATE_TOO_BIG)
        );
    }

    #[test]
    fn the_quota_counts_what_is_already_used() {
        assert_eq!(check_update(QUOTA_BYTES - 10, 10), Ok(()));
        assert_eq!(check_update(QUOTA_BYTES - 10, 11), Err(E_GS_QUOTA_EXCEEDED));
        assert_eq!(check_update(u64::MAX, 1), Err(E_GS_QUOTA_EXCEEDED));
    }

    #[test]
    fn erase_operations_do_not_count_towards_written_bytes() {
        let dir = TempDir::new("erase-size");
        let provider = dir.0.join("provider");
        let container = container_folder(&provider, b"slot");
        let erase = UpdateOperation {
            blob: b"old".to_vec(),
            erase: true,
            data: vec![0u8; MAX_BLOB_BYTES + 1],
        };
        assert_eq!(apply_update(&provider, &container, b"Slot", &[erase]), S_OK);
    }

    #[test]
    fn a_written_blob_is_read_back_and_listed_with_its_size() {
        let dir = TempDir::new("blobs");
        let provider = dir.0.join("provider");
        let container = container_folder(&provider, b"slot 1");
        let ops = [write(b"save", b"hello"), write(b"meta", b"xy")];
        assert_eq!(apply_update(&provider, &container, b"Slot one", &ops), S_OK);

        assert_eq!(read_blob(&container, b"save"), Ok(b"hello".to_vec()));
        assert_eq!(read_blob(&container, b"absent"), Err(E_GS_BLOB_NOT_FOUND));
        let mut blobs = list_blobs(&container, None);
        blobs.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            blobs,
            vec![
                BlobInfo {
                    name: b"meta".to_vec(),
                    size: 2
                },
                BlobInfo {
                    name: b"save".to_vec(),
                    size: 5
                },
            ]
        );
        assert_eq!(list_blobs(&container, Some(b"sa")).len(), 1);
    }

    #[test]
    fn an_erase_removes_the_blob_and_containers_report_their_totals() {
        let dir = TempDir::new("erase");
        let provider = dir.0.join("provider");
        let container = container_folder(&provider, b"slot");
        apply_update(
            &provider,
            &container,
            b"Partie",
            &[write(b"a", b"1234"), write(b"b", b"56")],
        );
        let erase = UpdateOperation {
            blob: b"a".to_vec(),
            erase: true,
            data: Vec::new(),
        };
        assert_eq!(
            apply_update(&provider, &container, b"Partie", &[erase]),
            S_OK
        );

        let info = find_container(&provider, b"slot").expect("the container exists");
        assert_eq!(info.display_name, b"Partie".to_vec());
        assert_eq!((info.blob_count, info.total_size), (1, 2));
        assert!(info.last_modified > 0);
    }

    #[test]
    fn containers_are_listed_by_prefix_and_deleted_once() {
        let dir = TempDir::new("containers");
        let provider = dir.0.join("provider");
        for name in [b"save-a".as_slice(), b"other".as_slice()] {
            apply_update(
                &provider,
                &container_folder(&provider, name),
                b"",
                &[write(b"x", b"1")],
            );
        }
        let all = list_containers(&provider, None);
        assert_eq!(all.len(), 2);
        let saves = list_containers(&provider, Some(b"save"));
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].name, b"save-a".to_vec());

        assert_eq!(delete_container(&provider, b"save-a"), S_OK);
        assert!(find_container(&provider, b"save-a").is_none());
        assert_eq!(delete_container(&provider, b"save-a"), S_OK);
        assert_eq!(list_containers(&provider, None).len(), 1);
    }

    #[test]
    fn the_local_storage_folder_is_created_under_the_title() {
        let dir = TempDir::new("local");
        let folder = local_storage_folder(&dir.0, 0xABCD_0001).expect("created");
        assert!(folder.is_dir());
        assert_eq!(folder, dir.0.join("ABCD0001").join("local"));
    }

    #[test]
    fn used_bytes_sums_the_files_of_sub_folders() {
        let dir = TempDir::new("used");
        let provider = dir.0.join("provider");
        apply_update(
            &provider,
            &container_folder(&provider, b"c"),
            b"",
            &[write(b"x", &[0; 100])],
        );
        assert!(used_bytes(&provider) >= 100);
        assert_eq!(used_bytes(&dir.0.join("missing")), 0);
    }
}
