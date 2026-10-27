//! Dedicated per-process CLI byte routing. Callers must supply endpoints
//! minted by the coordinator and an already authenticated native pipe peer.
use super::{
    cli_bridge::{FrontendEndpoint, WorkerEndpoint},
    cli_stream_wire::{Decoder, Direction, Encoder, Frame},
    pipe::{Event, Pipe},
    process::RetainedProcess,
};
use crate::error::{Error, Result};
use std::{
    io::{Read, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    thread,
    time::Duration,
};

const POLL: Duration = Duration::from_millis(20);
const COMPLETION_RECEIPT: &[u8] = b"pinvou-cli-complete-v1";

/// The service sends this only after persisting the connection's completed
/// state. A frontend must not exit successfully merely because its ACK was
/// written: that could race the service's process-exit monitor.
pub(super) fn send_completion_receipt(mut writer: impl Write) -> Result<()> {
    writer
        .write_all(COMPLETION_RECEIPT)
        .map_err(|_| Error::IpcInvalid)?;
    writer.flush().map_err(|_| Error::IpcInvalid)
}

pub(super) fn receive_completion_receipt(mut reader: impl Read) -> Result<()> {
    let mut actual = [0u8; COMPLETION_RECEIPT.len()];
    reader
        .read_exact(&mut actual)
        .map_err(|_| Error::IpcInvalid)?;
    if actual.as_slice() != COMPLETION_RECEIPT {
        return Err(Error::IpcInvalid);
    }
    Ok(())
}
struct CancelOnPanic<'a>(&'a Event);
impl Drop for CancelOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            let _ = self.0.set();
        }
    }
}

fn canceled(global: &Event, local: &Event) -> Result<bool> {
    Ok(global.is_set()? || local.is_set()?)
}
fn watch_frontend(
    global: &Event,
    local: &Event,
    pipe: &Pipe,
    peer: &RetainedProcess,
    worker: &RetainedProcess,
    bridge: &super::cli_bridge::CliBridge,
) {
    let _panic = CancelOnPanic(local);
    while matches!(local.is_set(), Ok(false)) {
        if !matches!(global.is_set(), Ok(false))
            || !matches!(peer.exited(), Ok(false))
            || !matches!(pipe.client_connected(), Ok(true))
            || (!bridge.worker_attached() && !matches!(worker.exited(), Ok(false)))
        {
            let _ = local.set();
            break;
        }
        thread::sleep(POLL);
    }
}
fn send(sender: &SyncSender<Frame>, mut frame: Frame, global: &Event, local: &Event) -> Result<()> {
    loop {
        if canceled(global, local)? {
            return Err(Error::RestartCanceled);
        }
        match sender.try_send(frame) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => return Err(Error::IpcInvalid),
            Err(TrySendError::Full(value)) => {
                frame = value;
                thread::sleep(POLL);
            }
        }
    }
}
fn receive(receiver: &Receiver<Frame>, global: &Event, local: &Event) -> Result<Frame> {
    loop {
        if canceled(global, local)? {
            return Err(Error::RestartCanceled);
        }
        match receiver.recv_timeout(POLL) {
            Ok(frame) => return Ok(frame),
            Err(RecvTimeoutError::Disconnected) => return Err(Error::IpcInvalid),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Worker-supplied exit data is not authoritative. Wait on the actual
/// retained kernel object before reading and forwarding its real exit code.
#[allow(unsafe_code)]
fn actual_exit(peer: &RetainedProcess, global: &Event, local: &Event) -> Result<i32> {
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::Threading::{GetExitCodeProcess, WaitForMultipleObjects},
    };
    let handles = [peer.raw(), global.raw(), local.raw()];
    if unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, u32::MAX) } != WAIT_OBJECT_0 {
        return Err(Error::RestartCanceled);
    }
    if canceled(global, local)? {
        return Err(Error::RestartCanceled);
    }
    let mut code = 0u32;
    if unsafe { GetExitCodeProcess(peer.raw(), &mut code) } == 0 {
        return Err(Error::HandoffOutcomeUnknown);
    }
    Ok(code as i32)
}

/// Relay one dedicated frontend connection. Input EOF does not end output;
/// output completion cancels a still-idle input read. No total timer exists.
pub(super) fn frontend(
    pipe: &Pipe,
    global: &Event,
    peer: &RetainedProcess,
    worker: &RetainedProcess,
    endpoint: FrontendEndpoint,
) -> Result<()> {
    let local = Event::new()?;
    let challenge: [u8; 64] = super::pipe::nonce()?
        .into_bytes()
        .try_into()
        .map_err(|_| Error::Unavailable)?;
    let (ack_tx, ack_rx) = mpsc::sync_channel::<()>(1);
    let bridge = endpoint.bridge();
    let (input_sender, output_receiver) = endpoint.into_parts();
    thread::scope(|scope| {
        let local = &local;
        // A full input queue prevents the pipe reader from detecting a lost
        // frontend, so observe the authenticated process independently.
        let watcher =
            scope.spawn(move || watch_frontend(global, local, pipe, peer, worker, &bridge));
        let input = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(&local);
            let result = (|| -> Result<()> {
                let mut decoder =
                    Decoder::new(pipe.stream_reader(&local, peer), Direction::FrontendInput);
                let mut input_open = true;
                loop {
                    match decoder.receive() {
                        Ok(Some(Frame::OutputAck(value))) => {
                            if value != challenge {
                                let _ = local.set();
                                return Err(Error::IpcInvalid);
                            }
                            ack_tx.send(()).map_err(|_| Error::IpcInvalid)?;
                            return Ok(());
                        }
                        Ok(Some(Frame::Error(_))) => {
                            let _ = local.set();
                            return Err(Error::IpcInvalid);
                        }
                        Ok(Some(frame)) if input_open => {
                            match send(&input_sender, frame, global, &local) {
                                Ok(()) => {}
                                Err(Error::IpcInvalid) => input_open = false,
                                Err(error) => return Err(error),
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => return Ok(()),
                        Err(_) if canceled(global, &local)? => return Ok(()),
                        Err(_) => {
                            let _ = local.set();
                            return Err(Error::IpcInvalid);
                        }
                    }
                }
            })();
            if result.is_err() {
                let _ = local.set();
            }
            result
        });
        let output = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(&local);
            let result = (|| {
                let mut encoder =
                    Encoder::new(pipe.stream_writer(&local, peer), Direction::FrontendOutput);
                let mut stdout_eof = false;
                let mut stderr_eof = false;
                loop {
                    let frame = receive(&output_receiver, global, &local)?;
                    match &frame {
                        Frame::StdoutEof => stdout_eof = true,
                        Frame::StderrEof => stderr_eof = true,
                        Frame::Exit(_) if !(stdout_eof && stderr_eof) => {
                            return Err(Error::IpcInvalid);
                        }
                        _ => {}
                    }
                    let end = matches!(frame, Frame::Exit(_) | Frame::Error(_));
                    let failed = matches!(frame, Frame::Error(_));
                    if matches!(frame, Frame::Exit(_)) {
                        encoder
                            .send(Frame::OutputChallenge(challenge))
                            .map_err(|_| Error::IpcInvalid)?;
                    }
                    encoder.send(frame).map_err(|_| Error::IpcInvalid)?;
                    if failed {
                        return Err(Error::IpcInvalid);
                    }
                    if end {
                        loop {
                            if canceled(global, &local)? {
                                return Err(Error::RestartCanceled);
                            }
                            match ack_rx.recv_timeout(POLL) {
                                Ok(()) => return Ok(()),
                                Err(RecvTimeoutError::Timeout) => {}
                                Err(RecvTimeoutError::Disconnected) => {
                                    return Err(Error::IpcInvalid);
                                }
                            }
                        }
                    }
                }
            })();
            let _ = local.set();
            result
        });
        let out = output.join();
        let _ = local.set();
        let input = input.join();
        let watcher = watcher.join();
        let out = out.map_err(|_| Error::HandoffOutcomeUnknown)?;
        let input = input.map_err(|_| Error::HandoffOutcomeUnknown)?;
        watcher.map_err(|_| Error::HandoffOutcomeUnknown)?;
        out.and(input)
    })
}

/// Relay the real admitted worker. Its announced exit is withheld until the
/// kernel process object signals and supplies the same actual code.
pub(super) fn worker(
    pipe: &Pipe,
    global: &Event,
    peer: &RetainedProcess,
    endpoint: WorkerEndpoint,
) -> Result<()> {
    let local = Event::new()?;
    let (input_receiver, output_sender) = endpoint.into_parts();
    thread::scope(|scope| {
        let local = &local;
        let watcher = scope.spawn(move || {
            let _panic = CancelOnPanic(&local);
            while matches!(local.is_set(), Ok(false)) {
                if !matches!(global.is_set(), Ok(false)) {
                    let _ = local.set();
                    break;
                }
                thread::sleep(POLL);
            }
        });
        let input = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(&local);
            let result = (|| -> Result<()> {
                let mut encoder = Encoder::new(pipe.stream_writer(&local, peer), Direction::Input);
                loop {
                    let frame = match receive(&input_receiver, global, &local) {
                        Ok(value) => value,
                        Err(_) if canceled(global, &local)? => return Ok(()),
                        Err(error) => {
                            let _ = local.set();
                            return Err(error);
                        }
                    };
                    let end = matches!(frame, Frame::InputEof | Frame::Error(_));
                    if encoder.send(frame).is_err() {
                        if canceled(global, &local)? || peer.exited()? {
                            return Ok(());
                        }
                        let _ = local.set();
                        return Err(Error::IpcInvalid);
                    }
                    if end {
                        return Ok(());
                    }
                }
            })();
            if result.is_err() {
                let _ = local.set();
            }
            result
        });
        let output = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(&local);
            let result = (|| {
                let mut decoder = Decoder::new(pipe.stream_reader(&local, peer), Direction::Output);
                let mut announced = None;
                loop {
                    match decoder.receive().map_err(|_| Error::IpcInvalid)? {
                        Some(Frame::Exit(code)) => announced = Some(code),
                        Some(frame @ Frame::Error(_)) => {
                            send(&output_sender, frame, global, &local)?;
                            return Err(Error::IpcInvalid);
                        }
                        Some(frame) => send(&output_sender, frame, global, &local)?,
                        None => break,
                    }
                }
                let actual = actual_exit(peer, global, &local)?;
                if announced != Some(actual) {
                    return Err(Error::HandoffOutcomeUnknown);
                }
                send(&output_sender, Frame::Exit(actual), global, &local)
            })();
            let _ = local.set();
            result
        });
        let out = output.join();
        let _ = local.set();
        let input = input.join();
        let watcher = watcher.join();
        let out = out.map_err(|_| Error::HandoffOutcomeUnknown)?;
        let input = input.map_err(|_| Error::HandoffOutcomeUnknown)?;
        watcher.map_err(|_| Error::HandoffOutcomeUnknown)?;
        out.and(input)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
    };

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn completion_receipt_requires_the_complete_exact_service_reply() {
        let mut bytes = Vec::new();
        send_completion_receipt(&mut bytes).unwrap();
        assert!(receive_completion_receipt(bytes.as_slice()).is_ok());
        assert!(receive_completion_receipt(&bytes[..bytes.len() - 1]).is_err());
        bytes[0] ^= 1;
        assert!(receive_completion_receipt(bytes.as_slice()).is_err());
    }

    #[test]
    fn frontend_exit_cancels_sender_blocked_by_full_input_queue() {
        let mut child = OwnedChild(
            Command::new(std::env::var_os("COMSPEC").unwrap())
                .args(["/Q", "/C", "pause >NUL"])
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let peer = RetainedProcess::open(child.0.id(), None).unwrap();
        let (server, _client) = Pipe::test_pair().unwrap();
        let worker = RetainedProcess::current().unwrap();
        let bridge = super::super::cli_bridge::CliBridge::new();
        let _worker_endpoint = bridge.worker().unwrap();
        let global = Event::new().unwrap();
        let local = Event::new().unwrap();
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender.send(Frame::InputEof).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| watch_frontend(&global, &local, &server, &peer, &worker, &bridge));
            scope.spawn(|| {
                ready_tx.send(()).unwrap();
                let result = send(&sender, Frame::InputEof, &global, &local);
                done_tx
                    .send(matches!(result, Err(Error::RestartCanceled)))
                    .unwrap();
            });
            ready_rx.recv().unwrap();
            child.0.kill().unwrap();
            let canceled = done_rx.recv_timeout(Duration::from_secs(5));
            // Always release both threads, even if the regression reappears.
            global.set().unwrap();
            assert_eq!(canceled, Ok(true));
        });
    }

    #[test]
    fn closed_pipe_with_unread_bytes_cancels_full_queue_while_peer_lives() {
        use std::io::Write;
        let (server, client) = Pipe::test_pair().unwrap();
        let peer = RetainedProcess::current().unwrap();
        let worker = RetainedProcess::current().unwrap();
        let bridge = super::super::cli_bridge::CliBridge::new();
        let _worker_endpoint = bridge.worker().unwrap();
        let global = Event::new().unwrap();
        let local = Event::new().unwrap();
        assert!(server.client_connected().unwrap());
        {
            let mut writer = client.stream_writer(&global, &peer);
            writer.write_all(b"unread").unwrap();
            writer.flush().unwrap();
        }
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender.send(Frame::InputEof).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(|| watch_frontend(&global, &local, &server, &peer, &worker, &bridge));
            scope.spawn(|| {
                ready_tx.send(()).unwrap();
                let result = send(&sender, Frame::InputEof, &global, &local);
                done_tx
                    .send(matches!(result, Err(Error::RestartCanceled)))
                    .unwrap();
            });
            ready_rx.recv().unwrap();
            let still_blocked = matches!(
                done_rx.recv_timeout(Duration::from_millis(100)),
                Err(RecvTimeoutError::Timeout)
            );
            drop(client);
            let canceled = done_rx.recv_timeout(Duration::from_secs(5));
            global.set().unwrap();
            assert!(still_blocked);
            assert_eq!(canceled, Ok(true));
        });
    }
}
