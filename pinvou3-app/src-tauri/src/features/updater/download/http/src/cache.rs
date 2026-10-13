//! Compose actual restricted HTTP EOF with actual file writes on an owner worker.
use crate::{FileBody, FileChunk, FileHttpError};
use pinvou_upgrade_byte_check::{
    ByteCheckError, CacheWritePermission, FileSegmentWriter, SegmentWriteError,
};
use pinvou_upgrade_transfer::SegmentReceipt;
use std::fs::File;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheReceiveError {
    Http(FileHttpError),
    Write(SegmentWriteError),
}
impl std::fmt::Display for CacheReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => e.fmt(f),
            Self::Write(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for CacheReceiveError {}

/// Run on an independent blocking worker, never inside an async executor poll.
/// The supplied runtime must remain alive and actively drive IO/timers; in a
/// current-thread runtime, its owner must keep Runtime::block_on running while
/// this worker executes (for example by awaiting spawn_blocking). Handle::block_on
/// alone does not drive that runtime's IO. No runtime or worker is created here.
///
/// Consume the actual body and derive the only plan/target from it. Only its real
/// SegmentFinished EOF event can complete the borrowed actual-file writer.
/// Cancellation is polled while awaiting network data at most every 100 ms of
/// scheduled timer progress; this interval is not a total/idle download deadline.
/// Blocking file IO cannot be interrupted mid-call. Failures drop the body and
/// retain untrusted file bytes for owner isolation/cleanup, without a receipt.
///
/// The owner independently protects the file, explicitly authorizes replacement,
/// preserves prefix continuity and later performs complete protected validation.
/// This API returns no final byte hash, durable cache trust or execution rights.
pub fn receive_cache_body_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    mut body: FileBody<'a>,
    file: &mut File,
    permission: CacheWritePermission,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SegmentReceipt<'a>, CacheReceiveError> {
    let mut writer = FileSegmentWriter::start(file, body.plan(), permission, &mut cancelled)
        .map_err(CacheReceiveError::Write)?;
    loop {
        let chunk = runtime.block_on(next_or_cancel(&mut body, &mut cancelled))?;
        match chunk {
            FileChunk::Data(bytes) => writer
                .write(&bytes, &mut cancelled)
                .map_err(CacheReceiveError::Write)?,
            FileChunk::SegmentFinished(receipt) => {
                return writer
                    .finish_after_network_eof(receipt, &mut cancelled)
                    .map_err(CacheReceiveError::Write);
            }
        }
    }
}
async fn next_or_cancel<'a>(
    body: &mut FileBody<'a>,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<FileChunk<'a>, CacheReceiveError> {
    let next = body.next_chunk();
    tokio::pin!(next);
    loop {
        if cancelled() {
            return Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::Cancelled,
            )));
        }
        tokio::select! {
            result = &mut next => return result.map_err(CacheReceiveError::Http),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}
