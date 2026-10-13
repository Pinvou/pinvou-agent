//! Compose actual restricted HTTP EOF with actual file writes on an owner worker.
use crate::observer::{self, CacheObserver, NoopObserver, WaitPhase};
use crate::{FileBody, FileChunk, FileHttpError};
use pinvou_upgrade_byte_check::{
    ByteCheckError, CacheWritePermission, FileSegmentWriter, SegmentWriteError,
};
use pinvou_upgrade_transfer::{Artifact, SegmentReceipt, TransferError, WriteMode};
use std::fs::File;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheReceiveError {
    OwnerStopped,
    Http(FileHttpError),
    Write(SegmentWriteError),
}
impl std::fmt::Display for CacheReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OwnerStopped => f.write_str("FILE_CACHE_OWNER_STOPPED"),
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
    body: FileBody<'a>,
    file: &mut File,
    permission: CacheWritePermission,
    mut cancelled: impl FnMut() -> bool,
) -> Result<SegmentReceipt<'a>, CacheReceiveError> {
    receive_cache_body_with_observer_on_worker(
        runtime,
        body,
        file,
        permission,
        &mut cancelled,
        &mut NoopObserver,
    )
}

/// B8's same actual-file/EOF operation with temporary owner observations.
/// Response Stop precedes writer start; Progress/Waiting Stop may leave untrusted
/// bytes. Continue never replaces explicit write permission or safe-stop proof.
pub fn receive_cache_body_with_observer_on_worker<'a>(
    runtime: &tokio::runtime::Handle,
    body: FileBody<'a>,
    file: &mut File,
    permission: CacheWritePermission,
    mut cancelled: impl FnMut() -> bool,
    observer: &mut dyn CacheObserver,
) -> Result<SegmentReceipt<'a>, CacheReceiveError> {
    receive_cache_with_write(
        runtime,
        body,
        file,
        permission,
        &mut cancelled,
        observer,
        &mut |writer, bytes, cancel| writer.write(bytes, cancel),
    )
}

pub(super) fn receive_cache_with_write<'a, F: FnMut() -> bool>(
    runtime: &tokio::runtime::Handle,
    mut body: FileBody<'a>,
    file: &mut File,
    permission: CacheWritePermission,
    cancelled: &mut F,
    observer: &mut dyn CacheObserver,
    write: &mut impl FnMut(
        &mut FileSegmentWriter<'_, 'a>,
        &[u8],
        &mut F,
    ) -> Result<(), SegmentWriteError>,
) -> Result<SegmentReceipt<'a>, CacheReceiveError> {
    let plan = body.plan();
    observer::response(observer, &body)?;
    let mut writer = FileSegmentWriter::start(file, plan, permission, &mut *cancelled)
        .map_err(CacheReceiveError::Write)?;
    let start = match plan.write_mode() {
        WriteMode::ReplaceFromZero => 0,
        WriteMode::Append { offset } => offset,
    };
    let mut response_bytes = 0_u64;
    loop {
        let chunk = runtime.block_on(await_or_cancel(
            body.next_chunk(),
            &mut *cancelled,
            observer,
            plan.target(),
            WaitPhase::Body,
        ))?;
        match chunk {
            FileChunk::Data(bytes) => {
                write(&mut writer, &bytes, cancelled).map_err(CacheReceiveError::Write)?;
                response_bytes = response_bytes
                    .checked_add(bytes.len() as u64)
                    .filter(|count| *count <= plan.expected_response_length())
                    .ok_or(CacheReceiveError::Write(SegmentWriteError::Transfer(
                        TransferError::BodyTooLong,
                    )))?;
                let next_offset = start
                    .checked_add(response_bytes)
                    .filter(|offset| *offset <= plan.target().size())
                    .ok_or(CacheReceiveError::Write(SegmentWriteError::Transfer(
                        TransferError::BodyTooLong,
                    )))?;
                observer::progress(observer, plan.target(), response_bytes, next_offset)?;
            }
            FileChunk::SegmentFinished(receipt) => {
                return writer
                    .finish_after_network_eof(receipt, &mut *cancelled)
                    .map_err(CacheReceiveError::Write);
            }
        }
    }
}
pub(super) async fn await_or_cancel<T>(
    next: impl std::future::Future<Output = Result<T, FileHttpError>>,
    cancelled: &mut impl FnMut() -> bool,
    observer: &mut dyn CacheObserver,
    target: Artifact<'_>,
    phase: WaitPhase,
) -> Result<T, CacheReceiveError> {
    tokio::pin!(next);
    loop {
        if cancelled() {
            return Err(CacheReceiveError::Write(SegmentWriteError::File(
                ByteCheckError::Cancelled,
            )));
        }
        tokio::select! {
            result = &mut next => return result.map_err(CacheReceiveError::Http),
            _ = tokio::time::sleep(Duration::from_millis(100)) => observer::waiting(observer, target, phase)?
        }
    }
}
