//! One owned worker: startup once, then only explicit authenticated App requests.
//! A single pending/running latch rejects overlap; no delayed retry queue.
use super::{dispatch::Core, pipe::Event, time_owner::TimeOwner, time_transport::TimeTransport};
use crate::{
    TrustedTimeState, TrustedTimeStatus,
    error::{Error, Result},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pending,
    Running,
    Idle,
    Stopped,
}
struct Activity {
    started: bool,
    phase: Phase,
    last_failure: Option<Error>,
}
/// Activity only. This never stores or authenticates a time result.
pub(super) struct RefreshControl {
    state: Mutex<Activity>,
}
impl RefreshControl {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(Activity {
                started: false,
                phase: Phase::Pending,
                last_failure: None,
            }),
        }
    }
    fn start(&self) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| Error::Unavailable)?;
        if s.phase == Phase::Stopped {
            return Err(Error::Unavailable);
        }
        if s.started {
            return Err(Error::Busy);
        }
        s.started = true;
        Ok(())
    }
    pub(super) fn request(&self) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| Error::Unavailable)?;
        if !s.started || s.phase == Phase::Stopped {
            return Err(Error::Unavailable);
        }
        if s.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        s.phase = Phase::Pending;
        Ok(())
    }
    fn claim(&self) -> Result<bool> {
        let mut s = self.state.lock().map_err(|_| Error::Unavailable)?;
        if !s.started || s.phase == Phase::Stopped {
            return Err(Error::Unavailable);
        }
        match s.phase {
            Phase::Pending => {
                s.phase = Phase::Running;
                Ok(true)
            }
            Phase::Idle => Ok(false),
            _ => Err(Error::Unavailable),
        }
    }
    fn finish(&self, result: Result<()>) -> Result<()> {
        let mut s = self.state.lock().map_err(|_| Error::Unavailable)?;
        // A late completion cannot resurrect a worker that was already stopped.
        if s.phase == Phase::Stopped {
            return Ok(());
        }
        if s.phase != Phase::Running {
            return Err(Error::Unavailable);
        }
        s.last_failure = result.err();
        s.phase = Phase::Idle;
        Ok(())
    }
    fn stop(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.phase = Phase::Stopped;
        }
        // Poisoning is unavailable in every reader/requester, never Idle.
    }
    pub(super) fn diagnostic(&self, native: TrustedTimeState) -> Result<TrustedTimeStatus> {
        let s = self.state.lock().map_err(|_| Error::Unavailable)?;
        let refreshing = s.started && matches!(s.phase, Phase::Pending | Phase::Running);
        // Native storage is always read first by Core. Only fixed activity or
        // the last actual failure can refine Unknown/unavailability; successful
        // HTTP alone cannot create CleanupPending or an integrity diagnosis.
        let state = if matches!(
            native,
            TrustedTimeState::Unsupported
                | TrustedTimeState::SetupRequired
                | TrustedTimeState::MissingPolicy
                | TrustedTimeState::IntegrityFault
        ) {
            native
        } else if !s.started || s.phase == Phase::Stopped {
            TrustedTimeState::SourceUnavailable
        } else if native == TrustedTimeState::Unknown
            && matches!(
                s.last_failure,
                Some(Error::SourceUnavailable | Error::Unavailable)
            )
        {
            TrustedTimeState::SourceUnavailable
        } else {
            native
        };
        Ok(TrustedTimeStatus::new(state, refreshing))
    }
}
struct WorkerExit(Arc<RefreshControl>);
impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// Owned by server::serve including every early listener/thread failure.
pub(super) struct TimeWorker {
    stop: Arc<Event>,
    cancel: Arc<AtomicBool>,
    control: Arc<RefreshControl>,
    thread: Option<JoinHandle<Result<()>>>,
}
impl TimeWorker {
    pub(super) fn start(core: Arc<Core>, stop: Arc<Event>) -> Result<Self> {
        let control = core.time.clone();
        if stop.is_set()? {
            control.stop();
            return Err(Error::Unavailable);
        }
        control.start()?;
        let thread_stop = stop.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("upgrade-trusted-time".into())
            .spawn(move || {
                // Construct before runtime creation. Build errors, early returns and
                // unwinding all permanently close this worker's request entrance.
                let _exit = WorkerExit(core.time.clone());
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| Error::Unavailable)?;
                // Declared AFTER runtime so its Drop closes requests BEFORE runtime
                // teardown, including unwind while the worker happened to be Idle.
                // The outer guard separately covers runtime construction failure.
                let _runtime_exit = WorkerExit(core.time.clone());
                runtime.block_on(async move {
                    loop {
                        tokio::select! {
                            biased;
                            stopped=observe_stop(&thread_stop,&thread_cancel)=>return stopped,
                            claimed=await_request(&core.time)=>claimed?,
                        }
                        let result = tokio::select! {
                            biased;
                            stopped=observe_stop(&thread_stop,&thread_cancel)=>return stopped,
                            result=refresh_once(&core)=>result,
                        };
                        core.time.finish(result)?;
                    }
                })
            })
            .map_err(|_| {
                control.stop();
                Error::Unavailable
            })?;
        Ok(Self {
            stop,
            cancel,
            control,
            thread: Some(thread),
        })
    }
    pub(super) fn stop_and_join(&mut self) -> Result<()> {
        self.cancel.store(true, Ordering::Release);
        self.control.stop(); // Reject App requests before attempting signalling.
        let stopped = self.stop.set();
        let joined = match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| Error::Unavailable)
                .and_then(|r| r),
            None => Ok(()),
        };
        // Signalling failure does not skip join; local cancellation is separate.
        stopped.and(joined)
    }
}
impl Drop for TimeWorker {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}
async fn refresh_once(core: &Core) -> Result<()> {
    // Actual stored state is re-opened for each controlled attempt. It preserves
    // floor/known deny/budgets/barrier; absence is never initialized or healed.
    let mut owner = TimeOwner::open(&core.host)?;
    owner.refresh(&core.host, &TimeTransport::new()).await
}
async fn await_request(control: &RefreshControl) -> Result<()> {
    loop {
        if control.claim()? {
            return Ok(());
        }
        // Reuse the native-stop scheduling interval only. No timer creates an
        // HTTP request: a startup reservation or explicit App latch is required.
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
async fn observe_stop(stop: &Event, cancel: &AtomicBool) -> Result<()> {
    loop {
        if cancel.load(Ordering::Acquire) || stop.is_set()? {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_stop_event_is_observed_and_signalling_thread_is_joined() {
        let event = Arc::new(Event::new().unwrap());
        let signal = event.clone();
        let thread = std::thread::spawn(move || signal.set().unwrap());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime
            .block_on(observe_stop(&event, &AtomicBool::new(false)))
            .unwrap();
        thread.join().unwrap();
        // This verifies the real native cancellation primitive only. It does
        // not construct a fake StartedServiceContext, HTTP result or time owner.
    }

    #[test]
    fn owned_worker_stop_latches_cancellation_and_joins_before_returning() {
        let stop = Arc::new(Event::new().unwrap());
        let cancel = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let thread_exited = exited.clone();
        // Lifecycle-only owned thread. This does not construct or substitute
        // a service context/time source. A finite test loop also joins on a
        // cancellation regression instead of stranding a test-owned thread.
        let thread = std::thread::spawn(move || {
            for _ in 0..1_000 {
                if thread_cancel.load(Ordering::Acquire) {
                    thread_exited.store(true, Ordering::Release);
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(Error::Unavailable)
        });
        let mut worker = TimeWorker {
            stop: stop.clone(),
            cancel,
            control: Arc::new(RefreshControl::new()),
            thread: Some(thread),
        };
        worker.stop_and_join().unwrap();
        assert!(exited.load(Ordering::Acquire));
        assert!(stop.is_set().unwrap());
        worker.stop_and_join().unwrap();
    }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    #[test]
    fn startup_and_concurrent_clicks_share_one_pending_running_slot() {
        let c = Arc::new(RefreshControl::new());
        c.start().unwrap();
        assert_eq!(c.request(), Err(Error::Busy));
        assert!(c.claim().unwrap());
        assert_eq!(c.request(), Err(Error::Busy));
        c.finish(Err(Error::SourceUnavailable)).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let mut threads = Vec::new();
        for _ in 0..16 {
            let c = c.clone();
            let b = barrier.clone();
            threads.push(std::thread::spawn(move || {
                b.wait();
                c.request()
            }));
        }
        barrier.wait();
        let mut accepted = 0;
        for t in threads {
            match t.join().unwrap() {
                Ok(()) => accepted += 1,
                Err(Error::Busy) => {}
                other => panic!("unexpected fixed result: {other:?}"),
            }
        }
        assert_eq!(accepted, 1);
        assert!(c.claim().unwrap());
        assert_eq!(c.request(), Err(Error::Busy));
        c.finish(Ok(())).unwrap();
        assert!(!c.claim().unwrap());
        // Successful activity does not manufacture successful protected time.
        assert_eq!(
            c.diagnostic(TrustedTimeState::Unknown).unwrap().state,
            TrustedTimeState::Unknown
        );
    }
    #[test]
    fn stop_and_worker_exit_never_resurrect_idle_or_accept_into_a_dead_worker() {
        let before = RefreshControl::new();
        before.stop();
        assert_eq!(before.start(), Err(Error::Unavailable));
        let c = Arc::new(RefreshControl::new());
        c.start().unwrap();
        assert!(c.claim().unwrap());
        let worker = c.clone();
        let _ = std::panic::catch_unwind(move || {
            let _exit = WorkerExit(worker);
            panic!("fixed lifecycle control");
        });
        assert_eq!(c.request(), Err(Error::Unavailable));
        c.finish(Ok(())).unwrap();
        assert_eq!(c.request(), Err(Error::Unavailable));
        assert!(!c.diagnostic(TrustedTimeState::Unknown).unwrap().refreshing);
        assert_eq!(
            c.diagnostic(TrustedTimeState::Unknown).unwrap().state,
            TrustedTimeState::SourceUnavailable
        );
        assert_eq!(
            c.diagnostic(TrustedTimeState::IntegrityFault)
                .unwrap()
                .state,
            TrustedTimeState::IntegrityFault
        );
    }
    #[test]
    fn early_worker_error_closes_admission_without_turning_offline_into_integrity_fault() {
        fn failed_runtime(control: Arc<RefreshControl>) -> Result<()> {
            let _exit = WorkerExit(control);
            Err(Error::Unavailable)
        }
        let c = Arc::new(RefreshControl::new());
        c.start().unwrap();
        assert_eq!(failed_runtime(c.clone()), Err(Error::Unavailable));
        assert_eq!(c.request(), Err(Error::Unavailable));
        let live = RefreshControl::new();
        live.start().unwrap();
        assert!(live.claim().unwrap());
        live.finish(Err(Error::SourceUnavailable)).unwrap();
        assert_eq!(
            live.diagnostic(TrustedTimeState::Unknown).unwrap().state,
            TrustedTimeState::SourceUnavailable
        );
        assert_eq!(
            live.diagnostic(TrustedTimeState::MissingPolicy)
                .unwrap()
                .state,
            TrustedTimeState::MissingPolicy
        );
        assert_eq!(
            live.diagnostic(TrustedTimeState::CleanupPending)
                .unwrap()
                .state,
            TrustedTimeState::CleanupPending
        );
    }
}
