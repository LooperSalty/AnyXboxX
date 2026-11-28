//! Original guest shader container storage. Without RAM trim the original
//! `Vec` is borrowed directly; an opted-in container can be packed losslessly.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ram_trim::COUNTERS;

pub(super) enum Container {
    Kept(Vec<u8>),
    Trimmable(Box<Trimmable>),
}

pub(super) struct Trimmable {
    bytes: Mutex<Storage>,
    hash: blake3::Hash,
    len: usize,
    builder_spilled: OnceLock<bool>,
}

#[derive(Clone)]
enum Storage {
    Raw(Arc<[u8]>),
    Packed(Arc<[u8]>),
    /// Too small or did not compress enough; the bytes will not change.
    Incompressible(Arc<[u8]>),
}

pub(super) enum Bytes<'a> {
    Borrowed(&'a [u8]),
    Shared(Arc<[u8]>),
    Inflated(Vec<u8>),
}

impl AsRef<[u8]> for Bytes<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Shared(bytes) => bytes,
            Self::Inflated(bytes) => bytes,
        }
    }
}

impl Container {
    pub(super) fn new(bytes: &[u8], hash: blake3::Hash, trim: bool) -> Self {
        if !trim {
            return Self::Kept(bytes.to_vec());
        }
        let len = bytes.len();
        COUNTERS.shader_container_orig_bytes.fetch_add(len as u64, Relaxed);
        COUNTERS.shader_container_retained_bytes.fetch_add(len as u64, Relaxed);
        Self::Trimmable(Box::new(Trimmable {
            bytes: Mutex::new(Storage::Raw(Arc::from(bytes))),
            hash,
            len,
            builder_spilled: OnceLock::new(),
        }))
    }

    pub(super) fn trimmable(&self) -> bool {
        matches!(self, Self::Trimmable(_))
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Kept(bytes) => bytes.len(),
            Self::Trimmable(trim) => trim.len,
        }
    }

    pub(super) fn hash(&self) -> blake3::Hash {
        match self {
            Self::Kept(bytes) => blake3::hash(bytes),
            Self::Trimmable(trim) => trim.hash,
        }
    }

    pub(super) fn bytes(&self) -> Bytes<'_> {
        match self {
            Self::Kept(bytes) => Bytes::Borrowed(bytes),
            Self::Trimmable(trim) => {
                // Keep the selected backing alive without holding the lock while
                // inflating or while callers parse, cache or save the bytes.
                let storage = trim.bytes.lock().unwrap_or_else(|e| e.into_inner()).clone();
                match storage {
                    Storage::Raw(raw) | Storage::Incompressible(raw) => Bytes::Shared(raw),
                    Storage::Packed(packed) => Bytes::Inflated(inflate(&packed, trim.len)),
                }
            }
        }
    }

    pub(super) fn builder_spilled(&self, check: impl FnOnce(&[u8]) -> bool) -> bool {
        match self {
            Self::Kept(bytes) => check(bytes),
            Self::Trimmable(trim) => *trim.builder_spilled.get_or_init(|| check(self.bytes().as_ref())),
        }
    }

    /// Called only after the shader has made a host PSO and no build pins it.
    pub(super) fn compress(&self) {
        const MIN_LEN: usize = 1024;
        const MIN_SAVING: usize = 256;
        let Self::Trimmable(trim) = self else { return };
        let mut storage = trim.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let Storage::Raw(raw) = &*storage else { return };
        if raw.len() >= MIN_LEN {
            let mut buf = vec![0u8; zlib_rs::compress_bound(raw.len())];
            let (packed, rc) = zlib_rs::compress_slice(&mut buf, raw, zlib_rs::DeflateConfig::new(1));
            if rc == zlib_rs::ReturnCode::Ok && packed.len() + MIN_SAVING < raw.len() {
                let saved = (raw.len() - packed.len()) as u64;
                let packed: &[u8] = packed;
                *storage = Storage::Packed(Arc::from(packed));
                COUNTERS.shader_container_retained_bytes.fetch_sub(saved, Relaxed);
                super::KEPT[1].fetch_sub(saved, Relaxed);
                return;
            }
            if rc != zlib_rs::ReturnCode::Ok {
                tracing::warn!(hash = %trim.hash, ?rc, "shader container compression failed; keeping original bytes");
            }
        }
        *storage = Storage::Incompressible(raw.clone());
    }
}

fn inflate(packed: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let (bytes, rc) = zlib_rs::decompress_slice(&mut out, packed, zlib_rs::InflateConfig::default());
    assert_eq!(rc, zlib_rs::ReturnCode::Ok, "internally compressed shader container is invalid");
    assert_eq!(bytes.len(), len, "internally compressed shader container has the wrong length");
    out
}
