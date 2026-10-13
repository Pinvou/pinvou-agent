//! Complete observed file-byte comparison, not file protection or execution rights.
//! The owner authenticates the expected target and maintains exclusive physical
//! object protection. A byte match alone must never become VerifiedExecutionObject.

use pinvou_upgrade_transfer::Artifact;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

mod readonly;
pub use readonly::{check_read_only_cache, copy_read_only_cache};

mod segment;
pub use segment::{CacheWritePermission, FileSegmentWriter, SegmentWriteError};

mod copy;
pub use copy::copy_and_check;

const BUFFER_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteCheckError {
    Cancelled,
    IoUnavailable,
    TooLong,
    TooShort,
    HashMismatch,
    DestinationNotEmpty,
    NotRegular,
}
impl std::fmt::Display for ByteCheckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "FILE_CHECK_CANCELLED",
            Self::IoUnavailable => "FILE_CHECK_IO_UNAVAILABLE",
            Self::TooLong => "FILE_CHECK_TOO_LONG",
            Self::TooShort => "FILE_CHECK_TOO_SHORT",
            Self::HashMismatch => "FILE_CHECK_HASH_MISMATCH",
            Self::DestinationNotEmpty => "FILE_COPY_DESTINATION_NOT_EMPTY",
            Self::NotRegular => "FILE_COPY_NOT_REGULAR",
        })
    }
}
// Original IO errors may contain private paths. No underlying error is retained.
impl std::error::Error for ByteCheckError {}

/// Only the bytes observed during this scan matched the original size/hash.
/// There is no durable identity, authenticity, current qualification or file lock.
pub struct ByteMatch<'a> {
    target: Artifact<'a>,
}
impl<'a> ByteMatch<'a> {
    pub fn target(&self) -> Artifact<'a> {
        self.target
    }
}

/// Synchronous bounded-memory scan of the same open file, starting at byte zero.
/// The file position ends at EOF on success. Run on a blocking worker in async
/// workflows; the owner keeps the handle/object protected throughout and after.
/// Cancellation is checked before seeking/reads and again after EOF. No elapsed
/// deadline or retry limit is imposed here, and no path is reopened or promoted.
pub fn check_file<'a>(
    file: &mut File,
    target: Artifact<'a>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    check_seekable(file, target, &mut cancelled)
}

fn check_seekable<'a>(
    file: &mut (impl Read + Seek),
    target: Artifact<'a>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    check_reader(file, target, cancelled)
}

fn check_reader<'a>(
    reader: &mut impl Read,
    target: Artifact<'a>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    scan_reader(reader, target, cancelled, &mut |_, _| Ok(()))
}

fn scan_reader<'a, F: FnMut() -> bool>(
    reader: &mut impl Read,
    target: Artifact<'a>,
    cancelled: &mut F,
    visit: &mut impl FnMut(&[u8], &mut F) -> Result<(), ByteCheckError>,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut hash = Sha256::new();
    let mut length = 0_u64;
    loop {
        if cancelled() {
            return Err(ByteCheckError::Cancelled);
        }
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ByteCheckError::IoUnavailable),
        };
        length = length
            .checked_add(count as u64)
            .filter(|length| *length <= target.size())
            .ok_or(ByteCheckError::TooLong)?;
        visit(&buffer[..count], cancelled)?;
        hash.update(&buffer[..count]);
    }
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    if length != target.size() {
        return Err(ByteCheckError::TooShort);
    }
    if hash.finalize().as_slice() != target.sha256() {
        return Err(ByteCheckError::HashMismatch);
    }
    Ok(ByteMatch { target })
}

#[cfg(test)]
mod tests;
