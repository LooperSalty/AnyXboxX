//! Pin the executable and the resolved directory chain until CreateProcess opens the image.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, GetFileInformationByHandle,
};

use super::{RunError, identity::read_error};

#[derive(Debug)]
pub(super) struct LockedPath {
    path: PathBuf,
    file: File,
    _directories: Vec<File>,
}

impl LockedPath {
    pub(super) fn open(exe: &Path) -> Result<Self, RunError> {
        let path = exe.canonicalize().map_err(|source| {
            if source.kind() == io::ErrorKind::NotFound {
                RunError::NoVictoryExe(exe.display().to_string())
            } else {
                read_error(exe, source).into()
            }
        })?;
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(':'))
        {
            return Err(read_error(exe, io::Error::other("expected an .exe file")).into());
        }
        // Lock from the root down: a parent cannot be redirected while a child is opened.
        // The filesystem root itself cannot be renamed; the OS owns the volume mapping.
        let parents: Vec<_> = path
            .ancestors()
            .skip(1)
            .filter(|dir| dir.parent().is_some())
            .collect();
        let mut directories = Vec::with_capacity(parents.len());
        for dir in parents.into_iter().rev() {
            directories.push(open_locked(dir, true).map_err(|source| read_error(dir, source))?);
        }
        let file = Self::open_file(&path).map_err(|source| read_error(&path, source))?;
        Ok(Self {
            path,
            file,
            _directories: directories,
        })
    }

    pub(super) fn open_file(path: &Path) -> io::Result<File> {
        open_locked(path, false)
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn file(&self) -> &File {
        &self.file
    }

    pub(super) fn recheck(&self) -> io::Result<()> {
        let current = Self::open_file(&self.path)?;
        if file_id(&current)? != file_id(&self.file)? {
            return Err(io::Error::other("verified VIctory executable was replaced"));
        }
        Ok(())
    }
}

fn open_locked(path: &Path, directory: bool) -> io::Result<File> {
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let file = OpenOptions::new()
        .read(true)
        // Deny both writes and delete/rename while the handle is open.
        .share_mode(FILE_SHARE_READ)
        .custom_flags(flags)
        .open(path)?;
    let metadata = file.metadata()?;
    let expected_kind = if directory {
        metadata.is_dir()
    } else {
        metadata.is_file()
    };
    if !expected_kind || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other(
            "expected a regular filesystem entry without redirection",
        ));
    }
    Ok(file)
}

fn file_id(file: &File) -> io::Result<(u32, u32, u32)> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The live handle came from OpenOptions and `info` is writable for the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        info.dwVolumeSerialNumber,
        info.nFileIndexHigh,
        info.nFileIndexLow,
    ))
}
