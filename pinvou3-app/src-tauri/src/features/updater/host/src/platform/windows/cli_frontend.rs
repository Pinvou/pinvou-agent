//! The first supported CLI forwarding profile: three non-console standard
//! streams, with distinct stdout and stderr kernel objects. No native handle
//! or selected PID is sent to the privileged service.
use super::{
    cli_spawn::{EnvironmentReceiver, UserEnvironment},
    installed::StartedServiceContext,
    peer::Peer,
    pipe::{Event, Pipe},
};
use crate::error::{Error, Result};
use windows_sys::Win32::{
    Foundation::{CompareObjectHandles, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType},
    System::Console::{
        GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    },
};

pub(super) struct FrontendStreams {
    input: HANDLE,
    output: HANDLE,
    error: HANDLE,
}

/// Per-authenticated-connection pending input. Neither request data nor the
/// sensitive environment is an authority to launch a process.
pub(super) struct PendingCli {
    pub(super) arguments: Vec<String>,
    pub(super) cwd: std::path::PathBuf,
    receiver: Option<EnvironmentReceiver>,
    environment: Option<UserEnvironment>,
}
impl PendingCli {
    pub(super) fn begin(
        host: &StartedServiceContext,
        peer: &Peer,
        pipe: &Pipe,
        body: crate::ipc::CliBegin,
    ) -> Result<Self> {
        use std::os::windows::ffi::OsStringExt;
        peer.recheck(pipe)?;
        if peer.role != crate::handoff::Role::CliRequester
            || body.arguments.len() > 4096
            || body.cwd_utf16.is_empty()
            || body.cwd_utf16.len() >= 32767
            || body.cwd_utf16.contains(&0)
        {
            return Err(Error::IpcInvalid);
        }
        super::cli_spawn::validate_arguments(host, &body.arguments)?;
        let cwd = std::path::PathBuf::from(std::ffi::OsString::from_wide(&body.cwd_utf16));
        if !cwd.is_absolute() {
            return Err(Error::IpcInvalid);
        }
        let receiver = EnvironmentReceiver::new(body.environment_units as usize)?;
        peer.recheck(pipe)?;
        Ok(Self {
            arguments: body.arguments,
            cwd,
            receiver: Some(receiver),
            environment: None,
        })
    }
    pub(super) fn receive(&mut self, peer: &Peer, pipe: &Pipe, stop: &Event) -> Result<()> {
        let receiver = self.receiver.take().ok_or(Error::IpcConflict)?;
        self.environment = Some(super::cli_spawn::receive_environment(
            peer, pipe, stop, receiver,
        )?);
        Ok(())
    }
    pub(super) fn into_parts(
        mut self,
    ) -> Result<(Vec<String>, std::path::PathBuf, UserEnvironment)> {
        if self.receiver.is_some() {
            return Err(Error::IpcConflict);
        }
        let environment = self.environment.take().ok_or(Error::IpcConflict)?;
        Ok((self.arguments, self.cwd, environment))
    }
}
impl FrontendStreams {
    #[allow(unsafe_code)]
    pub(super) fn capture() -> Result<Self> {
        let value = Self {
            input: unsafe { GetStdHandle(STD_INPUT_HANDLE) },
            output: unsafe { GetStdHandle(STD_OUTPUT_HANDLE) },
            error: unsafe { GetStdHandle(STD_ERROR_HANDLE) },
        };
        value.validate()?;
        Ok(value)
    }
    #[allow(unsafe_code)]
    fn validate(&self) -> Result<()> {
        for handle in [self.input, self.output, self.error] {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(Error::CliStreamsUnsupported);
            }
            let mut mode = 0;
            if unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
                return Err(Error::CliStreamsUnsupported);
            }
            let kind = unsafe { GetFileType(handle) };
            if kind != FILE_TYPE_DISK && kind != FILE_TYPE_PIPE {
                return Err(Error::CliStreamsUnsupported);
            }
        }
        if unsafe { CompareObjectHandles(self.output, self.error) } != 0 {
            return Err(Error::CliStreamsUnsupported);
        }
        Ok(())
    }
    /// Recheck before starting local pumps. The handles remain local to the
    /// caller; another thread closing and reusing one is not identity proof.
    #[allow(unsafe_code)]
    pub(super) fn recheck(&self) -> Result<()> {
        self.validate()?;
        for (original, which) in [
            (self.input, STD_INPUT_HANDLE),
            (self.output, STD_OUTPUT_HANDLE),
            (self.error, STD_ERROR_HANDLE),
        ] {
            let current = unsafe { GetStdHandle(which) };
            if current.is_null()
                || current == INVALID_HANDLE_VALUE
                || unsafe { CompareObjectHandles(original, current) } == 0
            {
                return Err(Error::CliStreamsUnsupported);
            }
        }
        Ok(())
    }
}

/// Authenticate this actual frontend, validate the selected first-wave
/// standard-stream profile, and prepare the same service connection. It does
/// not run the worker; connection drop cancels any prepared child.
pub(super) fn prepare_current_frontend() -> Result<(super::client::Client, FrontendStreams)> {
    use std::os::windows::ffi::OsStrExt;
    let streams = FrontendStreams::capture()?;
    let mut arguments = Vec::new();
    for value in std::env::args_os().skip(1) {
        arguments.push(value.into_string().map_err(|_| Error::IpcInvalid)?);
    }
    let cwd = std::env::current_dir().map_err(|_| Error::Unavailable)?;
    let cwd_utf16: Vec<u16> = cwd.as_os_str().encode_wide().collect();
    let environment = super::cli_spawn::capture_current_environment()?;
    let units = u32::try_from(environment.len()).map_err(|_| Error::IpcInvalid)?;
    let mut client = super::client::Client::connect(crate::handoff::Role::CliRequester)?;
    streams.recheck()?;
    let _: crate::ipc::Empty = client.request(
        "cli-begin",
        &crate::ipc::CliBegin {
            arguments,
            cwd_utf16,
            environment_units: units,
        },
    )?;
    super::cli_spawn::send_environment(&client, &environment)?;
    drop(environment);
    streams.recheck()?;
    let _: crate::ipc::Empty = client.request("cli-commit", &crate::ipc::Empty {})?;
    Ok((client, streams))
}

/// Run only in the short-lived CLI main frontend. Detached local stdio threads
/// hold no service capability and may be ended by process exit if an external
/// synchronous input/output driver never returns.
pub(crate) fn run_current_frontend() -> Result<i32> {
    use super::{
        cli_stream::{self, Bytes, Input, Layout, Output},
        cli_stream_wire::{Data, Decoder, Direction, Encoder, Frame},
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, RecvTimeoutError, TrySendError},
        },
        thread,
        time::Duration,
    };
    struct CancelOnPanic<'a>(&'a super::client::Client);
    impl Drop for CancelOnPanic<'_> {
        fn drop(&mut self) {
            if std::thread::panicking() {
                let _ = self.0.cancel_stream();
            }
        }
    }
    fn send_output(
        sender: &std::sync::mpsc::SyncSender<Output>,
        mut item: Output,
        client: &super::client::Client,
        poll: Duration,
    ) -> Result<()> {
        loop {
            match sender.try_send(item) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(Error::IpcInvalid),
                Err(TrySendError::Full(value)) => {
                    if client.stream_stopped()? {
                        return Err(Error::RestartCanceled);
                    }
                    item = value;
                    thread::sleep(poll);
                }
            }
        }
    }
    let (client, streams) = prepare_current_frontend()?;
    streams.recheck()?;
    let stopped = Arc::new(AtomicBool::new(false));
    let (input_tx, input_rx) = cli_stream::input_channel();
    let (output_tx, output_rx) = cli_stream::output_channel();
    let (challenge_tx, challenge_rx) = mpsc::sync_channel::<[u8; 64]>(1);
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    cli_stream::start_input(std::io::stdin(), input_tx.clone(), Arc::clone(&stopped));
    // The local output consumer may block in an external driver. It owns no
    // client handle and is not joined on service failure; main exits instead.
    thread::spawn(move || {
        let result = cli_stream::drain(
            output_rx,
            std::io::stdout(),
            std::io::stderr(),
            Layout::Separate,
        );
        let _ = finished_tx.send(result);
    });
    let poll = Duration::from_millis(20);
    thread::scope(|scope| -> Result<i32> {
        let client_ref = &client;
        let writer = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(client_ref);
            let result = (|| {
                let mut encoder =
                    Encoder::new(client_ref.stream_writer(), Direction::FrontendInput);
                loop {
                    if client_ref.stream_stopped()? {
                        return Err(Error::RestartCanceled);
                    }
                    let item = match input_rx.recv_timeout(poll) {
                        Ok(value) => value,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => return Err(Error::IpcInvalid),
                    };
                    let failed = matches!(&item, Input::Failed);
                    let (frame, done) = match item {
                        Input::Data(bytes) => (
                            Frame::Input(
                                Data::new(bytes.as_slice().to_vec())
                                    .map_err(|_| Error::IpcInvalid)?,
                            ),
                            false,
                        ),
                        Input::Eof => (Frame::InputEof, false),
                        Input::Failed => (Frame::Error(1), true),
                        Input::OutputAck(value) => (Frame::OutputAck(value), true),
                    };
                    encoder.send(frame).map_err(|_| Error::IpcInvalid)?;
                    if failed {
                        return Err(Error::IpcInvalid);
                    }
                    if done {
                        return Ok(());
                    }
                }
            })();
            if result.is_err() {
                let _ = client_ref.cancel_stream();
            }
            result
        });
        let client_ref = &client;
        let reader = scope.spawn(move || -> Result<()> {
            let _panic = CancelOnPanic(client_ref);
            let result = (|| {
                let mut decoder =
                    Decoder::new(client_ref.stream_reader(), Direction::FrontendOutput);
                let mut challenge = None;
                loop {
                    let frame = decoder
                        .receive()
                        .map_err(|_| Error::IpcInvalid)?
                        .ok_or(Error::IpcInvalid)?;
                    let item = match frame {
                        Frame::Stdout(bytes) => Output::Stdout(
                            Bytes::new(bytes.as_slice().to_vec()).map_err(|_| Error::IpcInvalid)?,
                        ),
                        Frame::Stderr(bytes) => Output::Stderr(
                            Bytes::new(bytes.as_slice().to_vec()).map_err(|_| Error::IpcInvalid)?,
                        ),
                        Frame::StdoutEof => Output::StdoutEof,
                        Frame::StderrEof => Output::StderrEof,
                        Frame::OutputChallenge(value) => {
                            challenge = Some(value);
                            continue;
                        }
                        Frame::Exit(code) => {
                            challenge_tx
                                .send(challenge.take().ok_or(Error::IpcInvalid)?)
                                .map_err(|_| Error::IpcInvalid)?;
                            send_output(&output_tx, Output::Exited(code), client_ref, poll)?;
                            return Ok(());
                        }
                        _ => return Err(Error::IpcInvalid),
                    };
                    send_output(&output_tx, item, client_ref, poll)?;
                }
            })();
            if result.is_err() {
                let _ = client_ref.cancel_stream();
            }
            result
        });
        let result = loop {
            match finished_rx.recv_timeout(poll) {
                Ok(Ok(code)) => break Ok(code),
                Ok(Err(_)) => break Err(Error::IpcInvalid),
                Err(RecvTimeoutError::Disconnected) => break Err(Error::IpcInvalid),
                Err(RecvTimeoutError::Timeout) => match client.stream_stopped() {
                    Ok(true) => break Err(Error::RestartCanceled),
                    Ok(false) => {}
                    Err(error) => break Err(error),
                },
            }
        };
        stopped.store(true, Ordering::Release);
        let result = match result {
            Ok(code) => match challenge_rx.try_recv() {
                Ok(challenge) => {
                    let mut ack = Input::OutputAck(challenge);
                    loop {
                        match input_tx.try_send(ack) {
                            Ok(()) => break Ok(code),
                            Err(TrySendError::Disconnected(_)) => break Err(Error::IpcInvalid),
                            Err(TrySendError::Full(value)) => {
                                match client.stream_stopped() {
                                    Ok(true) => break Err(Error::RestartCanceled),
                                    Ok(false) => {}
                                    Err(error) => break Err(error),
                                }
                                ack = value;
                                thread::sleep(poll);
                            }
                        }
                    }
                }
                Err(_) => Err(Error::IpcInvalid),
            },
            Err(error) => Err(error),
        };
        if result.is_err() {
            let _ = client.cancel_stream();
        }
        let writer = writer.join().map_err(|_| Error::HandoffOutcomeUnknown)?;
        let reader = reader.join().map_err(|_| Error::HandoffOutcomeUnknown)?;
        let code = result.and(writer).and(reader).and(result)?;
        // Keep this authenticated frontend alive until the service has marked
        // the acknowledged result complete, so normal exit cannot race abort.
        super::cli_transport::receive_completion_receipt(client.stream_reader())?;
        Ok(code)
    })
}

#[cfg(test)]
mod tests {
    use super::super::handle::Handle;
    use super::*;
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
        System::{Pipes::CreatePipe, Threading::GetCurrentProcess},
    };

    #[test]
    #[allow(unsafe_code)]
    fn actual_pipe_streams_allow_distinct_and_reject_alias_or_missing() {
        fn pipe() -> (Handle, Handle) {
            let mut read = ptr::null_mut();
            let mut write = ptr::null_mut();
            assert_ne!(
                unsafe { CreatePipe(&mut read, &mut write, ptr::null(), 0) },
                0
            );
            (Handle::owned(read).unwrap(), Handle::owned(write).unwrap())
        }
        let (input, _input_write) = pipe();
        let (_out_read, output) = pipe();
        let (_err_read, error) = pipe();
        let distinct = FrontendStreams {
            input: input.raw(),
            output: output.raw(),
            error: error.raw(),
        };
        assert!(distinct.validate().is_ok());
        let mut alias = ptr::null_mut();
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    output.raw(),
                    GetCurrentProcess(),
                    &mut alias,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let alias = Handle::owned(alias).unwrap();
        let merged = FrontendStreams {
            input: input.raw(),
            output: output.raw(),
            error: alias.raw(),
        };
        assert!(matches!(
            merged.validate(),
            Err(Error::CliStreamsUnsupported)
        ));
        let missing = FrontendStreams {
            input: ptr::null_mut(),
            output: output.raw(),
            error: error.raw(),
        };
        assert!(matches!(
            missing.validate(),
            Err(Error::CliStreamsUnsupported)
        ));
    }
}
