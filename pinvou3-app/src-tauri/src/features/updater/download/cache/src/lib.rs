//! Fresh cache object opening only; no protected namespace, ACL or execution rights.
mod platform;
use std::{fs::File, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheObjectError {
    Unsupported,
    AlreadyExists,
    IoUnavailable,
    InvalidObject,
    InvalidPath,
}
impl std::fmt::Display for CacheObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "CACHE_OBJECT_UNSUPPORTED",
            Self::AlreadyExists => "CACHE_OBJECT_ALREADY_EXISTS",
            Self::IoUnavailable => "CACHE_OBJECT_IO_UNAVAILABLE",
            Self::InvalidObject => "CACHE_OBJECT_INVALID",
            Self::InvalidPath => "CACHE_OBJECT_PATH_INVALID",
        })
    }
}
impl std::error::Error for CacheObjectError {}

/// Owns a newly created actual File opened with the platform's exclusive sharing
/// rules. No automatic Debug/Serialize/Clone or ordinary-File constructor exists.
/// Trusted callers can explicitly duplicate this borrowed handle; all duplicates
/// must close before the OS releases sharing restrictions. This is not sealed
/// read-only storage, namespace/ACL protection, authenticity or an execution object.
pub struct ExclusiveCacheFile {
    file: File,
}
impl ExclusiveCacheFile {
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }
}
/// Atomically create and exclusively open one new file. The owner supplies and
/// protects its root/path/ancestors/ACL and keeps the handle throughout transfer.
/// No existing file is truncated or removed. Post-create validation failure may
/// leave an untrusted new file; cleanup stays with the owner, never a path race.
pub fn create_exclusive_cache(path: &Path) -> Result<ExclusiveCacheFile, CacheObjectError> {
    platform::create(path).map(|file| ExclusiveCacheFile { file })
}
