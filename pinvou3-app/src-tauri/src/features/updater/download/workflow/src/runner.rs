use crate::{
    Error, Import, Job, NetworkConsent, Snapshot, StopRequest, journal::Phase, platform,
    store::attempt_name,
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

struct StopOnDrop(Arc<AtomicBool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Notice {
    Started { snapshot: Snapshot },
    Progress { written: u64 },
    Waiting { phase: &'static str },
}
struct Observer<F> {
    emit: F,
    last: Instant,
    waiting: Option<WaitPhase>,
    error: Option<Error>,
}
impl<F: FnMut(Notice) -> Result<(), Error>> CacheObserver for Observer<F> {
    fn observe(&mut self, notice: CacheNotice<'_, '_>) -> ObserverControl {
        let event = match notice {
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
            && let Err(error) = (self.emit)(event)
        {
            self.error = Some(error);
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
    // This library entry may be polled by a non-Tokio caller. Missing runtime
    // support is a typed failure before opening a job, never a caller panic.
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
}
fn run_on_worker(
    input: AttemptInput,
    runtime: &tokio::runtime::Handle,
    mut observe: impl FnMut(Notice) -> Result<(), Error>,
    stop: &AtomicBool,
) -> Result<Snapshot, Error> {
    let AttemptInput {
        path,
        pin,
        import,
        consent,
        restart,
    } = input;
    let mut job = Job::open(&path, &pin)?;
    if job.journal.phase == Phase::Cancelled {
        return Err(Error::Cancelled);
    }
    if !restart && job.journal.phase == Phase::BytesMatch {
        return job.check(|| stop.load(Ordering::Acquire));
    }
    consent.validate()?;
    let url = job.manifest.imported_url(&import)?;
    let attempt = job
        .journal
        .attempt
        .checked_add(1)
        .ok_or(Error::JournalDamaged)?;
    job.journal.append(Phase::Running, attempt, None)?;
    let run_id = job.journal.run_id.clone().ok_or(Error::JournalDamaged)?;
    // No replacement permission for an older object: the only writable file is
    // newly created by this invocation. Existing partial and complete files stay.
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
            requested.set(Some(StopRequest::Pause));
            true
        } else {
            match job.control(&run_id) {
                Ok(Some(value)) => {
                    requested.set(Some(value));
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
    let artifact = job.target.artifact()?;
    let plan = plan_request(artifact, None, SourceContinuity::Unproven);
    let request = CacheFetchRequest::new(
        &url,
        plan,
        &consent,
        &SystemResolver,
        ReplacementPermission::Allow,
    );
    let mut observer = Observer {
        emit: observe,
        last: Instant::now(),
        waiting: None,
        error: None,
    };
    let transfer = fetch_cache_once_with_observer_on_worker(
        runtime,
        request,
        &mut file,
        &mut cancelled,
        &mut observer,
    );
    let expected_full_segment = matches!(&transfer, Ok(CacheFetchOutcome::Segment(receipt)) if receipt.has_full_byte_count());
    let refused = match &transfer {
        Ok(_) => !expected_full_segment,
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
    let phase = if damaged.get() || local_io_failed || observer.error.is_some() {
        Phase::IoUnknown
    } else if let Some(stop) = requested.get() {
        match stop {
            StopRequest::Pause => Phase::Paused,
            StopRequest::Cancel => Phase::Cancelled,
        }
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
    };
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
    match phase {
        Phase::NetworkUnknown => Err(Error::NetworkUnknown),
        Phase::SourceRefused => Err(Error::SourceRefused),
        Phase::IoUnknown => Err(Error::IoUnavailable),
        Phase::BytesMismatch => Err(Error::ByteMismatch),
        _ => Ok(result),
    }
}
