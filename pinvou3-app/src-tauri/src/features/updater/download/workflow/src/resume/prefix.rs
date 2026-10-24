//! Bounded streaming scans/copy on actual already-owned file handles.
use crate::{Error, platform};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};
const BUFFER_BYTES: usize = 64 * 1024;
fn poll(cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn length(file: &File, expected: u64) -> Result<(), Error> {
    let metadata = file.metadata().map_err(|_| Error::IoUnavailable)?;
    if !metadata.is_file() || metadata.len() != expected {
        return Err(Error::UnsafeObject);
    }
    Ok(())
}
pub(super) fn scan(
    file: &mut File,
    expected: u64,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<String, Error> {
    poll(cancelled)?;
    let identity = platform::identity(file)?;
    length(file, expected)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    let mut remaining = expected;
    let mut digest = Sha256::new();
    let mut buffer = [0; BUFFER_BYTES];
    while remaining != 0 {
        poll(cancelled)?;
        let cap = remaining.min(BUFFER_BYTES as u64) as usize;
        let count = file
            .read(&mut buffer[..cap])
            .map_err(|_| Error::IoUnavailable)?;
        if count == 0 {
            return Err(Error::ByteMismatch);
        }
        digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    poll(cancelled)?;
    length(file, expected)?;
    if platform::identity(file)? != identity {
        return Err(Error::UnsafeObject);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(super) fn copy_checked(
    source: &mut File,
    destination: &mut File,
    length_bytes: u64,
    expected_sha256: &str,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
    if length_bytes == 0 || !crate::valid_hash(expected_sha256) {
        return Err(Error::InvalidInput);
    }
    // Verify the actual original prefix before copying, and the actual new file
    // after copying. A successful write/progress callback is not a byte proof.
    if scan(source, length_bytes, cancelled)? != expected_sha256 {
        return Err(Error::ByteMismatch);
    }
    length(destination, 0)?;
    let source_identity = platform::identity(source)?;
    let destination_identity = platform::identity(destination)?;
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| Error::IoUnavailable)?;
    let mut remaining = length_bytes;
    let mut copied_digest = Sha256::new();
    let mut buffer = [0; BUFFER_BYTES];
    while remaining != 0 {
        poll(cancelled)?;
        let cap = remaining.min(BUFFER_BYTES as u64) as usize;
        let count = source
            .read(&mut buffer[..cap])
            .map_err(|_| Error::IoUnavailable)?;
        if count == 0 {
            return Err(Error::ByteMismatch);
        }
        destination
            .write_all(&buffer[..count])
            .map_err(|_| Error::IoUnavailable)?;
        copied_digest.update(&buffer[..count]);
        remaining -= count as u64;
    }
    let copied_sha256: String = copied_digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if copied_sha256 != expected_sha256 {
        return Err(Error::ByteMismatch);
    }
    destination.sync_all().map_err(|_| Error::IoUnavailable)?;
    if scan(destination, length_bytes, cancelled)? != expected_sha256 {
        return Err(Error::ByteMismatch);
    }
    length(source, length_bytes)?;
    if platform::identity(source)? != source_identity
        || platform::identity(destination)? != destination_identity
    {
        return Err(Error::UnsafeObject);
    }
    Ok(())
}
