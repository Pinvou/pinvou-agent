//! Frontend-only stream pump. The authenticated transport owns framing and
//! supplies bounded channels; this module does not authenticate any peer.
//!
//! The input thread can remain blocked in an OS read. It owns only its input
//! reader and channel sender, and is deliberately not joined: this API is for
//! a short-lived CLI main process, never a library or long-lived host service.
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

const CHUNK: usize = 16 * 1024;
const QUEUE: usize = 4;

/// A frame cannot contain more than one bounded chunk. The caller must enforce
/// the same bound before allocating a frame from an untrusted transport.
pub(super) struct Bytes(Vec<u8>);
impl Bytes {
    pub(super) fn new(value: Vec<u8>) -> io::Result<Self> {
        if value.is_empty() || value.len() > CHUNK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid stream frame",
            ));
        }
        Ok(Self(value))
    }
    pub(super) fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

pub(super) enum Input {
    Data(Bytes),
    Eof,
    Failed,
    OutputAck([u8; 64]),
}

pub(super) enum Output {
    Stdout(Bytes),
    Stderr(Bytes),
    StdoutEof,
    StderrEof,
    Exited(i32),
}

/// Merged means the producer has already serialized the two aliases into the
/// stdout channel; redirecting two independently received channels to one
/// writer would not preserve their original order.
#[derive(Clone, Copy)]
pub(super) enum Layout {
    Separate,
    Merged,
}

pub(super) fn input_channel() -> (SyncSender<Input>, Receiver<Input>) {
    mpsc::sync_channel(QUEUE)
}

pub(super) fn output_channel() -> (SyncSender<Output>, Receiver<Output>) {
    mpsc::sync_channel(QUEUE)
}

/// Never retain the transport, its connection, the worker, or any join handle
/// here. A disconnected receiver wakes a sender waiting on a full queue.
pub(super) fn start_input<R: Read + Send + 'static>(
    mut input: R,
    sender: SyncSender<Input>,
    stop: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let mut buffer = [0u8; CHUNK];
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            match input.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(Input::Eof);
                    break;
                }
                Ok(n) => {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    // n is bounded by buffer.len() and nonzero here.
                    if sender
                        .send(Input::Data(Bytes(buffer[..n].to_vec())))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = sender.send(Input::Failed);
                    break;
                }
            }
        }
    });
}

/// Consume output until the worker exited and all configured output streams
/// reached EOF. An exit notification is not an output-drain notification.
/// Any output failure or transport disconnect returns an error; dropping the
/// receiver releases blocked producers rather than hiding lost output.
pub(super) fn drain<W: Write, E: Write>(
    receiver: Receiver<Output>,
    mut stdout: W,
    mut stderr: E,
    layout: Layout,
) -> io::Result<i32> {
    let mut stdout_eof = false;
    let mut stderr_eof = matches!(layout, Layout::Merged);
    let mut exit = None;
    while !(stdout_eof && stderr_eof && exit.is_some()) {
        let item = receiver
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "stream disconnected"))?;
        match item {
            Output::Stdout(data) if !stdout_eof => stdout.write_all(&data.0)?,
            Output::Stderr(data) if !stderr_eof => stderr.write_all(&data.0)?,
            Output::StdoutEof if !stdout_eof => stdout_eof = true,
            Output::StderrEof if !stderr_eof => stderr_eof = true,
            Output::Exited(code) if exit.is_none() => exit = Some(code),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid stream sequence",
                ));
            }
        }
    }
    stdout.flush()?;
    if matches!(layout, Layout::Separate) {
        stderr.flush()?;
    }
    exit.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing exit"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drains_after_exit_and_keeps_streams_separate() {
        let (tx, rx) = output_channel();
        let producer = thread::spawn(move || {
            tx.send(Output::Exited(7)).unwrap();
            tx.send(Output::Stderr(Bytes::new(b"err".to_vec()).unwrap()))
                .unwrap();
            tx.send(Output::Stdout(Bytes::new(b"out".to_vec()).unwrap()))
                .unwrap();
            tx.send(Output::StdoutEof).unwrap();
            tx.send(Output::StderrEof).unwrap();
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(drain(rx, &mut out, &mut err, Layout::Separate).unwrap(), 7);
        producer.join().unwrap();
        assert_eq!(out, b"out");
        assert_eq!(err, b"err");
    }
    #[test]
    fn merged_rejects_unmerged_stderr() {
        let (tx, rx) = output_channel();
        tx.send(Output::Stderr(Bytes::new(vec![1]).unwrap()))
            .unwrap();
        assert_eq!(
            drain(rx, Vec::new(), Vec::new(), Layout::Merged)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn input_is_bounded_and_sends_eof() {
        let (tx, rx) = input_channel();
        start_input(
            io::Cursor::new(vec![42; CHUNK + 1]),
            tx,
            Arc::new(AtomicBool::new(false)),
        );
        assert!(matches!(rx.recv().unwrap(), Input::Data(Bytes(v)) if v.len() == CHUNK));
        assert!(matches!(rx.recv().unwrap(), Input::Data(Bytes(v)) if v == vec![42]));
        assert!(matches!(rx.recv().unwrap(), Input::Eof));
        assert!(Bytes::new(vec![1; CHUNK + 1]).is_err());
    }
    #[test]
    fn disconnect_is_not_success() {
        let (tx, rx) = output_channel();
        drop(tx);
        assert_eq!(
            drain(rx, Vec::new(), Vec::new(), Layout::Merged)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    #[test]
    fn merged_preserves_producer_order_after_exit() {
        let (tx, rx) = output_channel();
        tx.send(Output::Stdout(Bytes::new(b"a".to_vec()).unwrap()))
            .unwrap();
        tx.send(Output::Exited(0)).unwrap();
        tx.send(Output::Stdout(Bytes::new(b"b".to_vec()).unwrap()))
            .unwrap();
        tx.send(Output::StdoutEof).unwrap();
        let mut out = Vec::new();
        assert_eq!(drain(rx, &mut out, Vec::new(), Layout::Merged).unwrap(), 0);
        assert_eq!(out, b"ab");
    }
    #[test]
    fn closed_output_is_reported_and_receiver_is_released() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (tx, rx) = output_channel();
        tx.send(Output::Stdout(Bytes::new(vec![1]).unwrap()))
            .unwrap();
        assert_eq!(
            drain(rx, Closed, Vec::new(), Layout::Merged)
                .unwrap_err()
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(tx.send(Output::StdoutEof).is_err());
    }
}
