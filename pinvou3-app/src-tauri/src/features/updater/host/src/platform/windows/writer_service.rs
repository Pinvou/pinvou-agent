//! Owned maintenance for native writer custody, independent of client pipes.
//! Disconnects never retire a writer; only the coordinator's native terminal
//! observations and protected readbacks can reclaim a slot.
use super::{dispatch::Core, pipe::Event};
use crate::error::{Error, Result};
use std::{sync::Arc, thread::JoinHandle, time::Duration};

pub(super) struct WriterWorker {
    stop: Arc<Event>,
    thread: Option<JoinHandle<()>>,
}
impl WriterWorker {
    pub(super) fn start(core: Arc<Core>, stop: Arc<Event>) -> Result<Self> {
        let worker_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("upgrade-writer-custody".into())
            .spawn(move || {
                while matches!(worker_stop.is_set(), Ok(false)) {
                    // Errors retain the same owned custody. No error, clock or
                    // timeout becomes an empty census or a replacement keeper.
                    let _ = core.writers.maintenance();
                    std::thread::sleep(Duration::from_millis(500));
                }
            })
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for WriterWorker {
    fn drop(&mut self) {
        let _ = self.stop.set();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
