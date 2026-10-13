//! Read-only ZIP32 byte inventory. Signed metadata, immutable source protection,
//! native identity selection and protected extraction remain owner duties.

mod directory;
mod paths;
mod stream;

use pinvou_upgrade_transfer::Artifact;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const SAFE_SIZE: u64 = 9_007_199_254_740_991;
const MAX_METADATA: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: u16 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    InvalidLimits,
    Cancelled,
    IoUnavailable,
    PackageBytes(pinvou_upgrade_byte_check::ByteCheckError),
    InvalidStructure,
    UnsupportedFormat,
    ResourceLimit,
    UnsafePath,
    UnsafeFileType,
    InvalidEntryBytes,
    CompressedTail,
}
impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidLimits => "ARCHIVE_LIMITS_INVALID",
            Self::Cancelled => "ARCHIVE_CANCELLED",
            Self::IoUnavailable => "ARCHIVE_IO_UNAVAILABLE",
            Self::PackageBytes(_) => "ARCHIVE_PACKAGE_BYTES_INVALID",
            Self::InvalidStructure => "ARCHIVE_STRUCTURE_INVALID",
            Self::UnsupportedFormat => "ARCHIVE_FORMAT_UNSUPPORTED",
            Self::ResourceLimit => "ARCHIVE_RESOURCE_LIMIT",
            Self::UnsafePath => "ARCHIVE_PATH_REFUSED",
            Self::UnsafeFileType => "ARCHIVE_FILE_TYPE_REFUSED",
            Self::InvalidEntryBytes => "ARCHIVE_ENTRY_BYTES_INVALID",
            Self::CompressedTail => "ARCHIVE_COMPRESSED_TAIL_REFUSED",
        })
    }
}
impl std::error::Error for ArchiveError {}

/// Explicit owner-supplied capacities, not production certification or defaults.
/// Path registry accounting has a separate bound from physical ZIP metadata.
#[derive(Clone, Copy)]
pub struct ArchiveLimits {
    file_bytes: u64,
    expanded_bytes: u64,
    entries: u16,
    metadata_bytes: u64,
    path_state_bytes: u64,
    ratio: u64,
}
impl ArchiveLimits {
    pub fn new(
        file_bytes: u64,
        expanded_bytes: u64,
        entries: u16,
        metadata_bytes: u64,
        path_state_bytes: u64,
        ratio: u64,
    ) -> Result<Self, ArchiveError> {
        if file_bytes == 0
            || file_bytes > SAFE_SIZE
            || expanded_bytes == 0
            || expanded_bytes > SAFE_SIZE
            || entries == 0
            || entries > MAX_ENTRIES
            || metadata_bytes == 0
            || metadata_bytes > MAX_METADATA
            || path_state_bytes == 0
            || path_state_bytes > MAX_METADATA
            || ratio == 0
            || ratio > SAFE_SIZE
        {
            return Err(ArchiveError::InvalidLimits);
        }
        Ok(Self {
            file_bytes,
            expanded_bytes,
            entries,
            metadata_bytes,
            path_state_bytes,
            ratio,
        })
    }
}

pub struct MemberBytes {
    name: String,
    size: u64,
    sha256: [u8; 32],
}
impl MemberBytes {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> [u8; 32] {
        self.sha256
    }
}

/// Complete observed inventory only. This is not a Manifest, native signature,
/// immutable object token, safe extraction result or installation permission.
pub struct PackageInventory<'a> {
    target: Artifact<'a>,
    members: Vec<MemberBytes>,
}
impl<'a> PackageInventory<'a> {
    pub fn target(&self) -> Artifact<'a> {
        self.target
    }
    pub fn members(&self) -> &[MemberBytes] {
        &self.members
    }
}

struct Budget {
    limits: ArchiveLimits,
    metadata: u64,
    expanded: u64,
}
impl Budget {
    fn metadata(&mut self, bytes: u64) -> Result<(), ArchiveError> {
        self.metadata = self
            .metadata
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.metadata_bytes)
            .ok_or(ArchiveError::ResourceLimit)?;
        Ok(())
    }
    fn expanded(&mut self, bytes: u64, compressed: u64) -> Result<(), ArchiveError> {
        let minimum =
            bytes / self.limits.ratio + u64::from(!bytes.is_multiple_of(self.limits.ratio));
        if bytes > self.limits.file_bytes || compressed < minimum {
            return Err(ArchiveError::ResourceLimit);
        }
        self.expanded = self
            .expanded
            .checked_add(bytes)
            .filter(|n| *n <= self.limits.expanded_bytes)
            .ok_or(ArchiveError::ResourceLimit)?;
        Ok(())
    }
}

/// Synchronous scan on a blocking worker. The owner protects the same regular
/// file throughout; cancellation does not interrupt an active OS operation.
/// Internal ZIP members are opaque files: no recursive archive interpretation.
pub fn inspect_file<'a>(
    file: &mut File,
    target: Artifact<'a>,
    limits: ArchiveLimits,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PackageInventory<'a>, ArchiveError> {
    poll(&mut cancelled)?;
    if !file
        .metadata()
        .map_err(|_| ArchiveError::IoUnavailable)?
        .is_file()
    {
        return Err(ArchiveError::UnsafeFileType);
    }
    check_package(file, target, &mut cancelled)?;
    let mut budget = Budget {
        limits,
        metadata: 0,
        expanded: 0,
    };
    let entries = directory::scan(file, target.size(), &mut budget, &mut cancelled)?;
    let mut members = Vec::new();
    for entry in &entries {
        poll(&mut cancelled)?;
        if !entry.directory {
            members.push(stream::member(file, entry, &mut cancelled)?);
        }
    }
    // A second actual container scan detects visible later changes, but cannot
    // replace exclusive ownership or detect every transient concurrent change.
    check_package(file, target, &mut cancelled)?;
    poll(&mut cancelled)?;
    Ok(PackageInventory { target, members })
}

fn check_package(
    file: &mut File,
    target: Artifact<'_>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), ArchiveError> {
    pinvou_upgrade_byte_check::check_file(file, target, cancelled)
        .map(|_| ())
        .map_err(|e| {
            if e == pinvou_upgrade_byte_check::ByteCheckError::Cancelled {
                ArchiveError::Cancelled
            } else {
                ArchiveError::PackageBytes(e)
            }
        })
}
fn poll(cancelled: &mut impl FnMut() -> bool) -> Result<(), ArchiveError> {
    if cancelled() {
        Err(ArchiveError::Cancelled)
    } else {
        Ok(())
    }
}
fn read_at(
    file: &mut File,
    offset: u64,
    bytes: &mut [u8],
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), ArchiveError> {
    poll(cancelled)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| ArchiveError::IoUnavailable)?;
    for mut byte_slice in bytes.chunks_mut(65_536) {
        while !byte_slice.is_empty() {
            poll(cancelled)?;
            match file.read(byte_slice) {
                Ok(0) => return Err(ArchiveError::IoUnavailable),
                Ok(count) => byte_slice = &mut byte_slice[count..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(ArchiveError::IoUnavailable),
            }
        }
    }
    Ok(())
}
fn u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes[..2].try_into().unwrap())
}
fn u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

#[cfg(test)]
mod tests;
