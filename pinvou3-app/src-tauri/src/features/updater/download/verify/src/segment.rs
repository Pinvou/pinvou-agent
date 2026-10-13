//! Actual segment writes only; complete byte verification remains a separate scan.
use crate::{BUFFER_BYTES, ByteCheckError};
use pinvou_upgrade_transfer::{BodyPlan, BodyProgress, SegmentReceipt, TransferError, WriteMode};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentWriteError {
    File(ByteCheckError),
    Transfer(TransferError),
    WritePermissionMismatch,
    PrefixLengthMismatch,
    ChunkAboveLimit,
    ReceiptMismatch,
    DestinationLengthMismatch,
}
impl std::fmt::Display for SegmentWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(e) => e.fmt(f),
            Self::Transfer(e) => e.fmt(f),
            Self::WritePermissionMismatch => f.write_str("FILE_SEGMENT_WRITE_PERMISSION_MISMATCH"),
            Self::PrefixLengthMismatch => f.write_str("FILE_SEGMENT_PREFIX_LENGTH_MISMATCH"),
            Self::ChunkAboveLimit => f.write_str("FILE_SEGMENT_CHUNK_ABOVE_LIMIT"),
            Self::ReceiptMismatch => f.write_str("FILE_SEGMENT_RECEIPT_MISMATCH"),
            Self::DestinationLengthMismatch => {
                f.write_str("FILE_SEGMENT_DESTINATION_LENGTH_MISMATCH")
            }
        }
    }
}
impl std::error::Error for SegmentWriteError {}

/// Explicit cache-owner intent, independent of the HTTP response plan.
/// Neither choice establishes physical ownership, prefix integrity or rights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheWritePermission {
    ReplaceExisting,
    AppendExisting,
}

/// Borrows the owner's protected cache handle. The separate ReplaceExisting
/// permission authorizes discarding its bytes for a 200 plan; 206 never truncates.
/// Neither a regular file nor matching length proves ownership or source trust.
/// All methods perform blocking IO and belong on the owner's blocking worker.
/// Errors latch; dropping a failed writer leaves untrusted bytes for its owner.
pub struct FileSegmentWriter<'file, 'target> {
    file: &'file mut File,
    plan: BodyPlan<'target>,
    progress: BodyProgress<'target>,
    failure: Option<SegmentWriteError>,
}
impl<'file, 'target> FileSegmentWriter<'file, 'target> {
    pub fn start(
        file: &'file mut File,
        plan: BodyPlan<'target>,
        permission: CacheWritePermission,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Self, SegmentWriteError> {
        poll(&mut cancelled)?;
        match (plan.write_mode(), permission) {
            (WriteMode::ReplaceFromZero, CacheWritePermission::ReplaceExisting)
            | (WriteMode::Append { .. }, CacheWritePermission::AppendExisting) => {}
            _ => return Err(SegmentWriteError::WritePermissionMismatch),
        }
        let metadata = file.metadata().map_err(|_| io_error())?;
        if !metadata.is_file() {
            return Err(SegmentWriteError::File(ByteCheckError::NotRegular));
        }
        let offset = match plan.write_mode() {
            WriteMode::ReplaceFromZero => 0,
            WriteMode::Append { offset } => {
                if metadata.len() != offset {
                    return Err(SegmentWriteError::PrefixLengthMismatch);
                }
                offset
            }
        };
        poll(&mut cancelled)?;
        if plan.write_mode() == WriteMode::ReplaceFromZero {
            file.set_len(0).map_err(|_| io_error())?;
        }
        file.seek(SeekFrom::Start(offset)).map_err(|_| io_error())?;
        poll(&mut cancelled)?;
        Ok(Self {
            file,
            plan,
            progress: plan.receive(),
            failure: None,
        })
    }

    /// Feed each actual transport chunk once. The 64 KiB bound limits each call,
    /// not total artifact size. A failed partial write cannot be retried here.
    pub fn write(
        &mut self,
        bytes: &[u8],
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<(), SegmentWriteError> {
        self.write_with(bytes, &mut cancelled, write_bytes)
    }

    fn write_with<F: FnMut() -> bool>(
        &mut self,
        bytes: &[u8],
        cancelled: &mut F,
        write: impl FnOnce(&mut File, &[u8], &mut F) -> Result<(), SegmentWriteError>,
    ) -> Result<(), SegmentWriteError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        let result = (|| {
            poll(cancelled)?;
            if bytes.len() > BUFFER_BYTES {
                return Err(SegmentWriteError::ChunkAboveLimit);
            }
            self.progress
                .observe_bytes(bytes.len() as u64)
                .map_err(SegmentWriteError::Transfer)?;
            write(self.file, bytes, cancelled)
        })();
        if let Err(error) = result {
            self.failure = Some(error);
        }
        result
    }

    /// Consume only after the restricted transport reports actual segment EOF.
    /// Its receipt must bind the same target and end offset as these writes.
    /// A returned receipt remains a count observation, never a full hash match.
    pub fn finish_after_network_eof(
        self,
        transport: SegmentReceipt<'target>,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<SegmentReceipt<'target>, SegmentWriteError> {
        self.finish_with_sync(transport, &mut cancelled, &mut |file| {
            file.sync_all().map_err(|_| io_error())
        })
    }

    fn finish_with_sync(
        mut self,
        transport: SegmentReceipt<'target>,
        cancelled: &mut impl FnMut() -> bool,
        sync: &mut impl FnMut(&File) -> Result<(), SegmentWriteError>,
    ) -> Result<SegmentReceipt<'target>, SegmentWriteError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        poll(cancelled)?;
        let receipt = self
            .progress
            .finish()
            .map_err(SegmentWriteError::Transfer)?;
        if transport.target() != self.plan.target()
            || transport.next_offset() != receipt.next_offset()
        {
            return Err(SegmentWriteError::ReceiptMismatch);
        }
        let check_length = |file: &File| {
            if file.metadata().map_err(|_| io_error())?.len() != receipt.next_offset() {
                return Err(SegmentWriteError::DestinationLengthMismatch);
            }
            Ok(())
        };
        check_length(self.file)?;
        poll(cancelled)?;
        sync(self.file)?;
        poll(cancelled)?;
        check_length(self.file)?;
        poll(cancelled)?;
        Ok(receipt)
    }
}
fn io_error() -> SegmentWriteError {
    SegmentWriteError::File(ByteCheckError::IoUnavailable)
}
fn poll(cancelled: &mut impl FnMut() -> bool) -> Result<(), SegmentWriteError> {
    if cancelled() {
        Err(SegmentWriteError::File(ByteCheckError::Cancelled))
    } else {
        Ok(())
    }
}
fn write_bytes(
    writer: &mut impl Write,
    mut bytes: &[u8],
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), SegmentWriteError> {
    while !bytes.is_empty() {
        poll(cancelled)?;
        match writer.write(bytes) {
            Ok(0) => return Err(io_error()),
            Ok(count) => bytes = &bytes[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(io_error()),
        }
    }
    poll(cancelled)
}
#[cfg(test)]
mod tests;
