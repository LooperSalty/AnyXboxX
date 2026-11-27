//! Identity of a prebuilt executable authorized by an independently supplied manifest digest.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
use super::locked_path::LockedPath;

pub const MANIFEST_PIN_ENV: &str = "ANYXBOXX_VICTORY_MANIFEST_SHA256";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildProfile {
    Release,
    Diag,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    executable_sha256: String,
    executable_size: u64,
    source_commit: String,
    tombox_gitlink: String,
    target: String,
    profile: BuildProfile,
    translator_version: std::num::NonZeroU32,
}

/// Provenance for the bytes selected for a prebuilt launch. The translator version is the
/// shader cache ABI of that build.
#[derive(Debug, Serialize)]
pub(super) struct VerifiedIdentity {
    trust: &'static str,
    manifest_sha256: String,
    #[serde(flatten)]
    manifest: Manifest,
}

impl VerifiedIdentity {
    pub(super) fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub(super) fn log_line(&self) -> String {
        // This record consists only of strings and integers.
        format!(
            "VICTORY_RUNTIME_IDENTITY={}",
            serde_json::to_string(self).expect("serialize runtime identity")
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("set {MANIFEST_PIN_ENV} to the manifest SHA-256 from your trusted build provider")]
    MissingPin,
    #[error("{MANIFEST_PIN_ENV} must be a 64-digit hexadecimal SHA-256")]
    InvalidPin,
    #[error("cannot open or read {path:?}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: io::Error,
    },
    #[error("the identity manifest exceeds {0} bytes")]
    ManifestTooLarge(u64),
    #[error("the identity manifest does not match the trusted SHA-256")]
    ManifestDigest,
    #[error("invalid identity manifest: {0}")]
    ManifestJson(#[from] serde_json::Error),
    #[error("unsupported identity schema {0}")]
    Schema(u32),
    #[error("invalid identity field {0}")]
    Field(&'static str),
    #[error("the executable size or SHA-256 does not match the identity manifest")]
    ExecutableDigest,
    #[error("the requested diagnostics require a prebuilt with profile diag")]
    Diagnostics,
    #[error("prebuilt execution requires Windows executable sharing protection")]
    Unsupported,
}

#[derive(Debug)]
pub(super) struct VerifiedPrebuilt {
    pub(super) identity: VerifiedIdentity,
    #[cfg(windows)]
    locked: LockedPath,
}

impl VerifiedPrebuilt {
    #[cfg(windows)]
    pub(super) fn open(exe: &Path, pin: &str) -> Result<Self, super::RunError> {
        use std::io::Read;

        use sha2::{Digest, Sha256};

        const MANIFEST_LIMIT: u64 = 4096;
        if !is_hex(pin, 64) {
            return Err(IdentityError::InvalidPin.into());
        }
        let locked = LockedPath::open(exe)?;
        let mut manifest_path = locked.path().as_os_str().to_os_string();
        manifest_path.push(".identity.json");
        let manifest_path = Path::new(&manifest_path);
        let mut bytes = Vec::new();
        LockedPath::open_file(manifest_path)
            .and_then(|file| file.take(MANIFEST_LIMIT + 1).read_to_end(&mut bytes))
            .map_err(|source| read_error(manifest_path, source))?;
        if bytes.len() as u64 > MANIFEST_LIMIT {
            return Err(IdentityError::ManifestTooLarge(MANIFEST_LIMIT).into());
        }
        let manifest_sha256 = format!("{:x}", Sha256::digest(&bytes));
        if !manifest_sha256.eq_ignore_ascii_case(pin) {
            return Err(IdentityError::ManifestDigest.into());
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(IdentityError::from)?;
        manifest.validate()?;
        let metadata = locked
            .file()
            .metadata()
            .map_err(|source| read_error(locked.path(), source))?;
        if metadata.len() != manifest.executable_size {
            return Err(IdentityError::ExecutableDigest.into());
        }
        let mut hasher = Sha256::new();
        let size = io::copy(&mut locked.file(), &mut hasher)
            .map_err(|source| read_error(locked.path(), source))?;
        if size != manifest.executable_size
            || !format!("{:x}", hasher.finalize()).eq_ignore_ascii_case(&manifest.executable_sha256)
        {
            return Err(IdentityError::ExecutableDigest.into());
        }
        Ok(Self {
            identity: VerifiedIdentity {
                trust: "pinned-manifest-sha256",
                manifest_sha256,
                manifest,
            },
            locked,
        })
    }

    #[cfg(not(windows))]
    pub(super) fn open(_exe: &Path, _pin: &str) -> Result<Self, super::RunError> {
        Err(IdentityError::Unsupported.into())
    }

    pub(super) fn profile(&self) -> BuildProfile {
        self.identity.manifest.profile
    }

    #[cfg(windows)]
    pub(super) fn path(&self) -> &Path {
        self.locked.path()
    }

    #[cfg(not(windows))]
    pub(super) fn path(&self) -> &Path {
        unreachable!("prebuilt verification requires Windows")
    }

    #[cfg(windows)]
    pub(super) fn recheck(&self) -> io::Result<()> {
        self.locked.recheck()
    }

    #[cfg(not(windows))]
    pub(super) fn recheck(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            IdentityError::Unsupported,
        ))
    }
}

#[cfg(windows)]
impl Manifest {
    fn validate(&self) -> Result<(), IdentityError> {
        if self.schema != 1 {
            return Err(IdentityError::Schema(self.schema));
        }
        for (field, value, length) in [
            ("executable_sha256", self.executable_sha256.as_str(), 64),
            ("source_commit", self.source_commit.as_str(), 40),
            ("tombox_gitlink", self.tombox_gitlink.as_str(), 40),
        ] {
            if !is_hex(value, length) || value.bytes().all(|byte| byte == b'0') {
                return Err(IdentityError::Field(field));
            }
        }
        if self.executable_size == 0 {
            return Err(IdentityError::Field("executable_size"));
        }
        if self.target != "x86_64-pc-windows-msvc" {
            return Err(IdentityError::Field("target"));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(windows)]
pub(super) fn read_error(path: &Path, source: io::Error) -> IdentityError {
    IdentityError::Read {
        path: path.to_path_buf(),
        source,
    }
}
