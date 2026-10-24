use crate::{
    Error, Import, Job, NetworkConsent, ResumeCheckpointPin, Snapshot, StopRequest, journal::Phase,
    platform, store::attempt_name,
};
use pinvou_upgrade_file_http::{
    CacheFetchOutcome, CacheFetchRequest, CacheNotice, CacheObserver, CacheReceiveError,
    FileHttpError, ObserverControl, ReplacementPermission, SystemResolver, WaitPhase,
    fetch_cache_once_with_observer_on_worker,
};
use pinvou_upgrade_transfer::{SourceContinuity, plan_request};
use std::{
    cell::Cell,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

// Preserve a real cancellation already observed for this attempt when later
// pause/drop observations arrive. This is local stop bookkeeping only.
pub(crate) fn remember_stop(requested: &Cell<Option<StopRequest>>, value: StopRequest) {
    if requested.get() != Some(StopRequest::Cancel) {
        requested.set(Some(value));
    }
}

fn interruption_phase(failed: bool, requested: Option<StopRequest>) -> Option<Phase> {
    if failed {
        Some(Phase::IoUnknown)
    } else {
        requested.map(|value| match value {
            StopRequest::Pause => Phase::Paused,
            StopRequest::Cancel => Phase::Cancelled,
        })
    }
}

// Round local elapsed time upward so a fractional final millisecond cannot
// extend a credential window. Supplied time remains an ordinary observation.
fn advanced_supplied_time(supplied: u64, elapsed: Duration) -> Result<u64, Error> {
    let millis = elapsed
        .as_millis()
        .checked_add(u128::from(elapsed.subsec_nanos() % 1_000_000 != 0))
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(Error::InvalidInput)?;
    supplied
        .checked_add(millis)
        .filter(|value| *value <= crate::MAX_SAFE)
        .ok_or(Error::InvalidInput)
}

pub(crate) fn revalidate_before_get(
    manifest: &crate::manifest::Manifest,
    import: &Import,
    elapsed: Duration,
) -> Result<(), Error> {
    let now = advanced_supplied_time(import.supplied_time_millis, elapsed)?;
    manifest.imported_source_at(import, now).map(|_| ())
}

pub(crate) struct StopOnDrop(pub(crate) Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Notice {
    Started {
        snapshot: Snapshot,
    },
    Progress {
        written: u64,
    },
    Waiting {
        phase: &'static str,
    },
    /// A flushed actual prefix checkpoint; the caller must retain this digest
    /// independently. It is not a native grant or a successful download result.
    Checkpoint {
        checkpoint_sha256: String,
    },
}
struct Observer<F> {
    emit: F,
    last: Instant,
    waiting: Option<WaitPhase>,
    error: Option<Error>,
    validator: Option<Vec<u8>>,
}
impl<F: FnMut(Notice) -> Result<(), Error>> Observer<F> {
    fn emit_notice(&mut self, notice: Notice) -> Result<(), Error> {
        let result = (self.emit)(notice);
        if let Err(error) = result {
            self.error = Some(error);
        }
        result
    }
}
impl<F: FnMut(Notice) -> Result<(), Error>> CacheObserver for Observer<F> {
    fn observe(&mut self, notice: CacheNotice<'_, '_>) -> ObserverControl {
        let event = match notice {
            CacheNotice::Response(value) => {
                self.validator = value.strong_validator_bytes().map(<[u8]>::to_vec);
                None
            }
            CacheNotice::Progress(value)
                if self.last.elapsed() >= Duration::from_millis(200)
                    || value.next_offset() == value.target().size() =>
            {
                self.last = Instant::now();
                self.waiting = None;
                Some(Notice::Progress {
                    written: value.next_offset(),
                })
            }
            CacheNotice::Waiting(value) if self.waiting != Some(value.phase()) => {
                self.waiting = Some(value.phase());
                Some(Notice::Waiting {
                    phase: match value.phase() {
                        WaitPhase::Fetch => "fetch",
                        WaitPhase::Body => "body",
                    },
                })
            }
            _ => None,
        };
        if let Some(event) = event
            && self.emit_notice(event).is_err()
        {
            return ObserverControl::Stop;
        }
        ObserverControl::Continue
    }
}

/// One explicit attempt, without an automatic retry cap or a download deadline.
/// A fresh import is checked before any new network action. Unknown continuity
/// always means a fresh O_EXCL object; no public parameter can assert continuity.
/// This joins its blocking worker before returning or releasing file custody.
pub async fn run(
    path: PathBuf,
    pin: String,
    import: Import,
    consent: NetworkConsent,
    restart: bool,
    observe: impl FnMut(Notice) -> Result<(), Error> + Send + 'static,
) -> Result<Snapshot, Error> {
    run_with_resume(path, pin, import, consent, restart, None, observe).await
}

/// Resume requires an independently retained digest of a real flushed prefix.
/// None means a safe fresh file; the untrusted journal never supplies a pin.
/// Current signed source, physical prefix and HTTP validator are all rechecked.
pub async fn run_with_resume(
    path: PathBuf,
    pin: String,
    import: Import,
    consent: NetworkConsent,
    restart: bool,
    resume: Option<ResumeCheckpointPin>,
    observe: impl FnMut(Notice) -> Result<(), Error> + Send + 'static,
) -> Result<Snapshot, Error> {
    let handle = tokio::runtime::Handle::try_current().map_err(|_| Error::IoUnavailable)?;
    let stop = Arc::new(AtomicBool::new(false));
    // Dropping a caller future requests a pause, but cannot acknowledge a join.
    // The actual worker retains the OS writer lock until its own real cleanup.
    let _stop_on_drop = StopOnDrop(stop.clone());
    tokio::task::spawn_blocking(move || {
        run_on_worker(
            AttemptInput {
                path,
                pin,
                import,
                consent,
                restart,
                resume,
            },
            &handle,
            observe,
            &stop,
        )
    })
    .await
    .map_err(|_| Error::IoUnavailable)?
}
struct AttemptInput {
    path: PathBuf,
    pin: String,
    import: Import,
    consent: NetworkConsent,
    restart: bool,
    resume: Option<ResumeCheckpointPin>,
}
fn run_on_worker(
    input: AttemptInput,
    runtime: &tokio::runtime::Handle,
    observe: impl FnMut(Notice) -> Result<(), Error>,
    stop: &AtomicBool,
) -> Result<Snapshot, Error> {
    let AttemptInput {
        path,
        pin,
        import,
        consent,
        restart,
        resume,
    } = input;
    let job = Job::open(&path, &pin)?;
    run_open_job(
        job,
        import,
        consent,
        AttemptOptions { restart, resume },
        runtime,
        observe,
        stop,
    )
}
pub(crate) struct AttemptOptions {
    pub(crate) restart: bool,
    pub(crate) resume: Option<ResumeCheckpointPin>,
}
pub(crate) fn run_open_job(
    mut job: Job,
    import: Import,
    consent: NetworkConsent,
    options: AttemptOptions,
    runtime: &tokio::runtime::Handle,
    mut observe: impl FnMut(Notice) -> Result<(), Error>,
    stop: &AtomicBool,
) -> Result<Snapshot, Error> {
    let AttemptOptions { restart, resume } = options;
    if job.journal.phase == Phase::Cancelled {
        return Err(Error::Cancelled);
    }
    if !restart && job.journal.phase == Phase::BytesMatch {
        return job.check(|| stop.load(Ordering::Acquire));
    }
    consent.validate()?;
    // Includes the first full verification and all later prefix scan/copy work.
    let validation_started = Instant::now();
    let (url, source) = job.manifest.imported_source(&import)?;
    let attempt = job
        .journal
        .attempt
        .checked_add(1)
        .ok_or(Error::JournalDamaged)?;
    job.journal.append(Phase::Running, attempt, None)?;
    let run_id = job.journal.run_id.clone().ok_or(Error::JournalDamaged)?;
    // Even a resumed attempt owns a new exclusive file. The retained original
    // prefix is never truncated; legal 200 replacement affects only this file.
    let mut file = job
        .directory
        .file(attempt_name(attempt), true, true, true)?;
    let identity = platform::identity(&file)?;
    job.journal.append(Phase::Opened, attempt, Some(identity))?;
    observe(Notice::Started {
        snapshot: job.snapshot(),
    })?;
    let requested = Cell::new(None);
    let damaged = Cell::new(false);
    let mut cancelled = || {
        if stop.load(Ordering::Acquire) {
            remember_stop(&requested, StopRequest::Pause);
            true
        } else {
            match job.control(&run_id) {
                Ok(Some(value)) => {
                    remember_stop(&requested, value);
                    true
                }
                Ok(None) => false,
                Err(_) => {
                    damaged.set(true);
                    true
                }
            }
        }
    };
    let (retained, preparation_error) = if restart {
        (None, None)
    } else {
        match crate::resume::prepare(&job, resume.as_ref(), &source, &mut file, &mut cancelled) {
            Ok(prefix) => (prefix, None),
            // Started already exposed this attempt's run ID. A pause or cancel
            // while scanning/copying must use the same terminal journal path as
            // one observed during HTTP, and must never start a subsequent GET.
            // Other local prefix failures also close the recorded attempt.
            // Their exact typed error is preserved after terminal persistence.
            Err(error) => (None, Some(error)),
        }
    };
    let artifact = job.target.artifact()?;
    let plan = match &retained {
        Some(prefix) => prefix.plan(artifact)?,
        None => plan_request(artifact, None, SourceContinuity::Unproven),
    };
    let request = CacheFetchRequest::new(
        &url,
        plan,
        &consent,
        &SystemResolver,
        ReplacementPermission::Allow,
    );
    // Prefix preparation can outlive the original credential window. Repeat
    // the complete original validation immediately before the new GET. No
    // ongoing transfer deadline or automatic credential refresh is introduced.
    let credential_error = if preparation_error.is_none() {
        revalidate_before_get(&job.manifest, &import, validation_started.elapsed()).err()
    } else {
        None
    };
    let mut observer = Observer {
        emit: observe,
        last: Instant::now(),
        waiting: None,
        error: None,
        validator: None,
    };
    let transfer = if preparation_error.is_some() || credential_error.is_some() {
        Err(CacheReceiveError::OwnerStopped)
    } else {
        fetch_cache_once_with_observer_on_worker(
            runtime,
            request,
            &mut file,
            &mut cancelled,
            &mut observer,
        )
    };
    let expected_full_segment = matches!(&transfer, Ok(CacheFetchOutcome::Segment(receipt)) if receipt.has_full_byte_count());
    let refused = match &transfer {
        Ok(CacheFetchOutcome::Segment(_)) => false,
        Ok(_) => true,
        Err(CacheReceiveError::Http(
            FileHttpError::DnsFailed
            | FileHttpError::DnsTimeout
            | FileHttpError::RequestTimeout
            | FileHttpError::ConnectionFailed
            | FileHttpError::NetworkFailed,
        )) => false,
        Err(CacheReceiveError::Http(_)) => true,
        _ => false,
    };
    let write_failed = matches!(&transfer, Err(CacheReceiveError::Write(_)));
    let mut scan_error = None;
    let matched = match transfer {
        Ok(CacheFetchOutcome::Segment(receipt)) if receipt.next_offset() == artifact.size() => {
            match pinvou_upgrade_byte_check::check_file(&mut file, artifact, &mut cancelled) {
                Ok(_) => true,
                Err(error) => {
                    scan_error = Some(error);
                    false
                }
            }
        }
        _ => false,
    };
    // One last control observation after whole-file hashing prevents a late
    // local pause/cancel from being presented as a completed observed attempt.
    let _ = cancelled();
    file.sync_all().map_err(|_| Error::IoUnavailable)?;
    if platform::identity(&file)? != identity {
        return Err(Error::UnsafeObject);
    }
    let length = file.metadata().map_err(|_| Error::IoUnavailable)?.len();
    let local_io_failed =
        scan_error == Some(pinvou_upgrade_byte_check::ByteCheckError::IoUnavailable);
    let phase_for = |checkpoint_failed: bool, observer_failed: bool| {
        let failed = damaged.get()
            || local_io_failed
            || observer_failed
            || checkpoint_failed
            || preparation_error.is_some_and(|e| e != Error::Cancelled);
        if let Some(phase) = interruption_phase(failed, requested.get()) {
            phase
        } else if credential_error.is_some() {
            Phase::SourceRefused
        } else if matched {
            Phase::BytesMatch
        } else if expected_full_segment {
            Phase::BytesMismatch
        } else if refused {
            Phase::SourceRefused
        } else if write_failed {
            Phase::IoUnknown
        } else {
            Phase::NetworkUnknown
        }
    };
    let preliminary_phase = phase_for(false, observer.error.is_some());
    let mut checkpoint_error = None;
    if matches!(preliminary_phase, Phase::Paused | Phase::NetworkUnknown)
        && !stop.load(Ordering::Acquire)
    {
        // Network I/O has stopped. A prior Pause permits the bounded-buffer
        // prefix scan to finish, but a new Cancel or damaged control must still
        // stop it. Persist the single final phase only after these observations.
        let mut checkpoint_cancelled = || {
            if requested.get() == Some(StopRequest::Cancel) {
                return true;
            }
            if stop.load(Ordering::Acquire) {
                remember_stop(&requested, StopRequest::Pause);
                return true;
            }
            match job.control(&run_id) {
                Ok(Some(StopRequest::Cancel)) => {
                    remember_stop(&requested, StopRequest::Cancel);
                    true
                }
                Ok(Some(StopRequest::Pause)) => {
                    remember_stop(&requested, StopRequest::Pause);
                    false
                }
                Ok(None) => false,
                Err(_) => {
                    damaged.set(true);
                    true
                }
            }
        };
        // Transfer observations are finished. Move the opaque validator out so
        // checkpoint emission uses the same observer error latch as progress.
        let validator = observer.validator.take();
        if let Err(error) = crate::resume::publish_notice(
            &job,
            &mut file,
            &source,
            validator.as_deref(),
            &mut checkpoint_cancelled,
            &mut |notice| observer.emit_notice(notice),
        ) {
            if error != Error::Cancelled || (requested.get().is_none() && !damaged.get()) {
                checkpoint_error = Some(error);
            }
        }
    }
    // Observe controls again after checkpoint flush and its callback. A cache
    // notice is only a prefix observation, never cancellation or install rights.
    let _ = cancelled();
    let phase = phase_for(checkpoint_error.is_some(), observer.error.is_some());
    job.journal.append(phase, attempt, Some(identity))?;
    let mut result = job.snapshot();
    result.actual_length = Some(length);
    result.observed_bytes_match = matched && phase == Phase::BytesMatch;
    // Body and fetch futures have already dropped in the restricted adapter.
    // The worker owns every handle until this function returns; await joins it.
    if damaged.get() {
        return Err(Error::JournalDamaged);
    }
    if local_io_failed {
        return Err(Error::IoUnavailable);
    }
    if let Some(error) = observer.error {
        return Err(error);
    }
    if let Some(error) = preparation_error.filter(|error| *error != Error::Cancelled) {
        return Err(error);
    }
    if let Some(error) = credential_error {
        return Err(error);
    }
    if let Some(error) = checkpoint_error {
        return Err(error);
    }
    match phase {
        Phase::NetworkUnknown => Err(Error::NetworkUnknown),
        Phase::SourceRefused => Err(Error::SourceRefused),
        Phase::IoUnknown => Err(Error::IoUnavailable),
        Phase::BytesMismatch => Err(Error::ByteMismatch),
        _ => Ok(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_get_elapsed_is_checked_ceil_milliseconds_not_a_download_deadline() {
        assert_eq!(advanced_supplied_time(10, Duration::ZERO), Ok(10));
        assert_eq!(advanced_supplied_time(10, Duration::from_nanos(1)), Ok(11));
        assert_eq!(advanced_supplied_time(10, Duration::from_millis(1)), Ok(11));
        assert_eq!(advanced_supplied_time(10, Duration::new(1, 1)), Ok(1011));
        assert_eq!(
            advanced_supplied_time(crate::MAX_SAFE, Duration::ZERO),
            Ok(crate::MAX_SAFE)
        );
        for (supplied, elapsed) in [
            (crate::MAX_SAFE, Duration::from_nanos(1)),
            (u64::MAX, Duration::from_millis(1)),
            (0, Duration::MAX),
        ] {
            assert_eq!(
                advanced_supplied_time(supplied, elapsed),
                Err(Error::InvalidInput)
            );
        }
    }

    #[test]
    fn checkpoint_callback_cancelled_is_latched_as_an_observer_failure() {
        let mut calls = 0;
        {
            let mut observer = Observer {
                emit: |notice| {
                    assert!(matches!(notice, Notice::Checkpoint { .. }));
                    calls += 1;
                    Err(Error::Cancelled)
                },
                last: Instant::now(),
                waiting: None,
                error: None,
                validator: None,
            };
            assert_eq!(
                observer.emit_notice(Notice::Checkpoint {
                    checkpoint_sha256: "a".repeat(64),
                }),
                Err(Error::Cancelled)
            );
            // The runner consumes this same latch with priority over a prior Pause.
            // This is an observer contract test, not a successful HTTP download.
            assert_eq!(observer.error, Some(Error::Cancelled));
            assert!(
                interruption_phase(observer.error.is_some(), Some(StopRequest::Pause))
                    == Some(Phase::IoUnknown)
            );
            assert!(interruption_phase(false, Some(StopRequest::Pause)) == Some(Phase::Paused));
        }
        assert_eq!(calls, 1);
    }
}
