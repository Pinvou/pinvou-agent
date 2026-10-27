//! Bounded per-original CLI byte routing. The coordinator creates and owns
//! this bridge only for a real staged child; endpoint selection is not an IPC
//! capability and is never constructed from a PID or wire value.
use super::cli_stream_wire::{self, Frame};
use crate::error::{Error, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender},
};

pub(super) struct CliBridge {
    worker_taken: AtomicBool,
    input_sender: Mutex<Option<SyncSender<Frame>>>,
    input_receiver: Mutex<Option<Receiver<Frame>>>,
    output_sender: Mutex<Option<SyncSender<Frame>>>,
    output_receiver: Mutex<Option<Receiver<Frame>>>,
}
pub(super) struct FrontendEndpoint {
    owner: Arc<CliBridge>,
    pub(super) input: SyncSender<Frame>,
    pub(super) output: Receiver<Frame>,
}
pub(super) struct WorkerEndpoint {
    pub(super) input: Receiver<Frame>,
    pub(super) output: SyncSender<Frame>,
}
impl CliBridge {
    pub(super) fn new() -> Arc<Self> {
        let (input_sender, input_receiver) = cli_stream_wire::channel();
        let (output_sender, output_receiver) = cli_stream_wire::channel();
        Arc::new(Self {
            worker_taken: AtomicBool::new(false),
            input_sender: Mutex::new(Some(input_sender)),
            input_receiver: Mutex::new(Some(input_receiver)),
            output_sender: Mutex::new(Some(output_sender)),
            output_receiver: Mutex::new(Some(output_receiver)),
        })
    }
    pub(super) fn frontend(self: &Arc<Self>) -> Result<FrontendEndpoint> {
        let mut input = self
            .input_sender
            .lock()
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        let mut output = self
            .output_receiver
            .lock()
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        if input.is_none() || output.is_none() {
            return Err(Error::IpcConflict);
        }
        Ok(FrontendEndpoint {
            owner: Arc::clone(self),
            input: input.take().ok_or(Error::StateCorrupt)?,
            output: output.take().ok_or(Error::StateCorrupt)?,
        })
    }
    pub(super) fn worker(&self) -> Result<WorkerEndpoint> {
        let mut input = self
            .input_receiver
            .lock()
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        let mut output = self
            .output_sender
            .lock()
            .map_err(|_| Error::HandoffOutcomeUnknown)?;
        if input.is_none() || output.is_none() {
            return Err(Error::IpcConflict);
        }
        let endpoint = WorkerEndpoint {
            input: input.take().ok_or(Error::StateCorrupt)?,
            output: output.take().ok_or(Error::StateCorrupt)?,
        };
        self.worker_taken.store(true, Ordering::Release);
        Ok(endpoint)
    }
    pub(super) fn worker_attached(&self) -> bool {
        self.worker_taken.load(Ordering::Acquire)
    }
}
impl FrontendEndpoint {
    pub(super) fn belongs_to(&self, bridge: &Arc<CliBridge>) -> bool {
        Arc::ptr_eq(&self.owner, bridge)
    }
    pub(super) fn bridge(&self) -> Arc<CliBridge> {
        Arc::clone(&self.owner)
    }
    pub(super) fn into_parts(self) -> (SyncSender<Frame>, Receiver<Frame>) {
        (self.input, self.output)
    }
}
impl WorkerEndpoint {
    pub(super) fn into_parts(self) -> (Receiver<Frame>, SyncSender<Frame>) {
        (self.input, self.output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_are_single_consumer_and_disconnect_releases_sender() {
        let bridge = CliBridge::new();
        let frontend = bridge.frontend().unwrap();
        let worker = bridge.worker().unwrap();
        assert!(bridge.frontend().is_err());
        assert!(bridge.worker().is_err());
        drop(worker.input);
        assert!(frontend.input.send(Frame::InputEof).is_err());
        drop(frontend.output);
        assert!(worker.output.send(Frame::StdoutEof).is_err());
    }
}
