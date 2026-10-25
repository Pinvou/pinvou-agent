//! One SCM-owned startup refresh. No detached tasks, implicit retry or policy
//! defaults. Failure leaves time unavailable without removing setup-only IPC.
use super::{dispatch::Core, pipe::Event, time_owner::TimeOwner, time_transport::TimeTransport};
use crate::error::{Error, Result};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// Owned by server::serve for its complete lifetime, including every early
/// pipe/listener/thread failure. The actual Core retains its real SCM context.
pub(super) struct TimeWorker {
    stop: Arc<Event>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl TimeWorker {
    pub(super) fn start(core: Arc<Core>, stop: Arc<Event>) -> Result<Self> {
        let thread_stop = stop.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("upgrade-trusted-time".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| Error::Unavailable)?;
                runtime.block_on(async move {
                    // Stop is selected first if both branches become ready.
                    // Dropping refresh drops actual reqwest I/O, then the
                    // current-thread runtime is dropped on this owned thread.
                    tokio::select! {
                        biased;
                        stopped = observe_stop(&thread_stop, &thread_cancel) => stopped,
                        () = refresh_once(&core) => observe_stop(&thread_stop, &thread_cancel).await,
                    }
                })
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            stop,
            cancel,
            thread: Some(thread),
        })
    }

    pub(super) fn stop_and_join(&mut self) -> Result<()> {
        self.cancel.store(true, Ordering::Release);
        let stopped = self.stop.set();
        // Always join, even if native signalling itself failed. The local
        // cancellation latch prevents a valid but unsignalled handle from
        // stranding this worker. It cannot create a time result or permission.
        let joined = match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| Error::Unavailable)
                .and_then(|r| r),
            None => Ok(()),
        };
        stopped.and(joined)
    }
}

impl Drop for TimeWorker {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

async fn refresh_once(core: &Core) {
    // open authenticates existing protected state and never heals absence.
    // Its protected policy is the only source of limits/admission. No ordinary
    // environment value, request DTO or old HTTP result can supply authority.
    if let Ok(mut owner) = TimeOwner::open(&core.host) {
        let transport = TimeTransport::new();
        let _ = owner.refresh(&core.host, &transport).await;
    }
    // Missing policy/state, transient HTTP failure, integrity refusal and an
    // uncertain commit never grant Known or clear the cleanup barrier here.
    // Keep the original setup service usable. There is deliberately no retry
    // loop: the internal owner refresh entrance remains the explicit caller.
}

async fn observe_stop(stop: &Event, cancel: &AtomicBool) -> Result<()> {
    loop {
        if cancel.load(Ordering::Acquire) || stop.is_set()? {
            return Ok(());
        }
        // Scheduling for native cancellation only; neither an anchor freshness
        // policy nor an end-to-end network/deadline limit.
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
            thread: Some(thread),
        };
        worker.stop_and_join().unwrap();
        assert!(exited.load(Ordering::Acquire));
        assert!(stop.is_set().unwrap());
        worker.stop_and_join().unwrap();
    }
}
