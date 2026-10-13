use crate::{Artifact, ByteCheckError, ByteMatch, check_seekable, scan_reader};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

/// Copy the current source bytes into an independently protected fresh destination,
/// sync it, then rescan that same destination handle. Existing matches are not input.
/// The owner supplies exclusive object protection; an empty metadata length does
/// not establish it. Failure can leave untrusted partial bytes for owner cleanup.
/// No path is reopened, truncated, deleted, promoted or given execution rights.
pub fn copy_and_check<'a>(
    source: &mut File,
    destination: &mut File,
    target: Artifact<'a>,
    mut cancelled: impl FnMut() -> bool,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    copy_with_sync(source, destination, target, &mut cancelled, &mut |file| {
        file.sync_all().map_err(|_| ByteCheckError::IoUnavailable)
    })
}

fn copy_with_sync<'a>(
    source: &mut File,
    destination: &mut File,
    target: Artifact<'a>,
    cancelled: &mut impl FnMut() -> bool,
    sync: &mut impl FnMut(&File) -> Result<(), ByteCheckError>,
) -> Result<ByteMatch<'a>, ByteCheckError> {
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    let source_metadata = source
        .metadata()
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    let destination_metadata = destination
        .metadata()
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    if !source_metadata.is_file() || !destination_metadata.is_file() {
        return Err(ByteCheckError::NotRegular);
    }
    if destination_metadata.len() != 0 {
        return Err(ByteCheckError::DestinationNotEmpty);
    }
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    source
        .seek(SeekFrom::Start(0))
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    destination
        .seek(SeekFrom::Start(0))
        .map_err(|_| ByteCheckError::IoUnavailable)?;
    scan_reader(source, target, cancelled, &mut |bytes, cancelled| {
        write_chunk(destination, bytes, cancelled)
    })?;
    if cancelled() {
        return Err(ByteCheckError::Cancelled);
    }
    sync(destination)?;
    check_seekable(destination, target, cancelled)
}

fn write_chunk(
    writer: &mut impl Write,
    mut bytes: &[u8],
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), ByteCheckError> {
    while !bytes.is_empty() {
        if cancelled() {
            return Err(ByteCheckError::Cancelled);
        }
        match writer.write(bytes) {
            Ok(0) => return Err(ByteCheckError::IoUnavailable),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ByteCheckError::IoUnavailable),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
