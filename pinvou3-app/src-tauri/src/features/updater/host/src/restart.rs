//! Opaque app restart worker lifetime. Readiness and commit come exclusively
//! from the native adapter; this guard keeps its one original session alive.
use crate::{Error, error::Result};
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread::{self, JoinHandle},
    time::Instant,
};
pub(crate) enum Control {
    Commit,
    Cancel,
}
/// Send, single-owner, non-Clone. Drop cancels/closes and joins its fixed worker.
pub struct PreparedRestart {
    control: Option<SyncSender<Control>>,
    worker: Option<JoinHandle<Result<CommittedRestart>>>,
}
/// Definite original commit ACK, with a conservative local control grace.
/// It is not a trusted deadline, installation permission, or image capability.
pub struct CommittedRestart {
    deadline: Instant,
}
impl CommittedRestart {
    pub(crate) fn checked_until(deadline: Instant) -> Result<Self> {
        if deadline <= Instant::now() {
            Err(Error::RestartCanceled)
        } else {
            Ok(Self { deadline })
        }
    }
    pub fn is_current(&self) -> bool {
        Instant::now() < self.deadline
    }
}
impl PreparedRestart {
    pub(crate) fn start(
        worker: impl FnOnce(Receiver<Control>, SyncSender<Result<()>>) -> Result<CommittedRestart>
        + Send
        + 'static,
    ) -> Result<Self> {
        let (control, receiver) = mpsc::sync_channel(1);
        let (ready, initial) = mpsc::sync_channel(1);
        let ended = ready.clone();
        let worker = thread::Builder::new()
            .name("pinvou-host-restart".into())
            .spawn(move || {
                let result = worker(receiver, ready);
                // If native setup failed before readiness, always unblock its
                // initial caller. The single-channel receiver disappears on return.
                let _ = ended.try_send(Err(result
                    .as_ref()
                    .err()
                    .copied()
                    .unwrap_or(Error::HandoffOutcomeUnknown)));
                result
            })
            .map_err(|_| Error::Unavailable)?;
        match initial.recv() {
            Ok(Ok(())) => Ok(Self {
                control: Some(control),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(Error::HandoffOutcomeUnknown)
            }
        }
    }
    pub fn commit_exit(mut self) -> Result<CommittedRestart> {
        let control = self.control.take().ok_or(Error::RestartCanceled)?;
        let sent = control.send(Control::Commit).is_ok();
        drop(control);
        let joined = self
            .worker
            .take()
            .ok_or(Error::HandoffOutcomeUnknown)?
            .join()
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        if !sent {
            return match joined {
                Err(error) => Err(error),
                Ok(_) => Err(Error::HandoffOutcomeUnknown),
            };
        }
        let marker = joined?;
        if !marker.is_current() {
            return Err(Error::RestartCanceled);
        }
        Ok(marker)
    }
}
impl Drop for PreparedRestart {
    fn drop(&mut self) {
        if let Some(control) = self.control.take() {
            let _ = control.try_send(Control::Cancel);
            drop(control);
        }
        // No shared/domain lock is held. The native worker's fixed control IO
        // either cancels this association or closes only its own pipe session.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;
    #[test]
    fn worker_guard_drop_joins_and_commit_never_forges_ack() {
        let canceled = Arc::new(AtomicBool::new(false));
        let child = canceled.clone();
        let guard = PreparedRestart::start(move |controls, ready| {
            ready.send(Ok(())).unwrap();
            assert!(matches!(controls.recv(), Ok(Control::Cancel)));
            child.store(true, Ordering::SeqCst);
            Err(Error::RestartCanceled)
        })
        .unwrap();
        drop(guard);
        assert!(canceled.load(Ordering::SeqCst));
        let guard = PreparedRestart::start(|controls, ready| {
            ready.send(Ok(())).unwrap();
            assert!(matches!(controls.recv(), Ok(Control::Commit)));
            Err(Error::HandoffOutcomeUnknown)
        })
        .unwrap();
        assert!(matches!(
            guard.commit_exit(),
            Err(Error::HandoffOutcomeUnknown)
        ));
        let guard = PreparedRestart::start(|controls, ready| {
            ready.send(Ok(())).unwrap();
            assert!(matches!(controls.recv(), Ok(Control::Commit)));
            CommittedRestart::checked_until(Instant::now() + Duration::from_secs(1))
        })
        .unwrap();
        assert!(guard.commit_exit().unwrap().is_current());
        // This is worker-lifetime QA, not a native host/session/capability test.
    }
}
