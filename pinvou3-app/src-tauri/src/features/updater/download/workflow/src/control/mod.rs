//! Explicit fixed control acquisition followed by the existing cache attempt.
//! No endpoint/CA/header injection, remote retry, continuation grant or install.
mod request;
use crate::{Error, Job, NetworkConsent, Notice, Snapshot, journal::Phase};
use pinvou_upgrade_control_http::{Operation, post_once};
use request::PreparedControl;
use serde::Deserialize;
use std::{
    future::Future,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Original wire and an explicitly supplied time observation. This ordinary
/// tool cannot establish trusted wall time, denial state or current eligibility.
/// The elapsed control request time is added before validating the response;
/// a credential expiry never becomes a logical download-wide deadline.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlAttempt {
    pub request: String,
    pub supplied_time_millis: u64,
}

/// One fixed DownloadInfo POST, complete original verification and one existing
/// restricted GET attempt. The original real writer lock remains held across
/// the POST and file attempt; there is no gap for another writer to take over.
/// Dropping this future requests local cancellation. Only this awaited result
/// follows the blocking worker's real close; a drop is no remote outcome proof.
pub async fn run_control(
    path: PathBuf,
    pin: String,
    input: ControlAttempt,
    consent: NetworkConsent,
    restart: bool,
    observe: impl FnMut(Notice) -> Result<(), Error> + Send + 'static,
) -> Result<Snapshot, Error> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::IoUnavailable)?;
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = crate::runner::StopOnDrop(stop.clone());
    tokio::task::spawn_blocking(move || {
        let mut job = Job::open(&path, &pin)?;
        if stop.load(Ordering::Acquire) || job.journal.phase == Phase::Cancelled {
            return Err(Error::Cancelled);
        }
        if !restart && job.journal.phase == Phase::BytesMatch {
            return job.check(|| stop.load(Ordering::Acquire));
        }
        consent.validate()?;
        let prepared = PreparedControl::new(&job.manifest, input)?;
        // The production operation and service origin are fixed in the existing
        // transport; neither a cache record nor ControlAttempt supplies them.
        let response = runtime.block_on(await_control(
            async {
                let response = post_once(Operation::DownloadInfo, prepared.request().as_bytes())
                    .await
                    .map_err(|_| Error::ControlUnavailable)?;
                Ok((response.status(), response.bytes().to_vec()))
            },
            &stop,
        ))?;
        if stop.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let import = prepared.finish(&job.manifest, response.0, response.1)?;
        if stop.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        crate::runner::run_open_job(job, import, consent, restart, &runtime, observe, &stop)
    })
    .await
    .map_err(|_| Error::IoUnavailable)?
}

// A polling cancellation observation is not an attempt deadline. Dropping the
// original POST future cancels its I/O; no detached request task is spawned.
async fn await_control<T>(
    future: impl Future<Output = Result<T, Error>>,
    stop: &AtomicBool,
) -> Result<T, Error> {
    if stop.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                if stop.load(Ordering::Acquire) {
                    return Err(Error::Cancelled);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
