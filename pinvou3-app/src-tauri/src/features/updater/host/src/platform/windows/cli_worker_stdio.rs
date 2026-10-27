//! Worker-local standard stream capture for an authenticated CLI worker.
//! Native reads preserve completed bytes when stop races completion.
use super::{
    app_writer::AppWriterAdmission,
    cli_stream_wire::{self, Data, Decoder, Direction, Encoder, Frame},
    client::Client,
    handle::Handle,
    pipe::{Event, nonce},
};
use crate::{handoff::Role, ipc};
use std::{
    io::{self, Write},
    ptr,
    sync::{Arc, Mutex, mpsc::SyncSender},
    thread::{self, JoinHandle},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED,
        GetLastError, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
    },
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, CreatePipe, GetNamedPipeClientProcessId,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, PeekNamedPipe,
        },
        Threading::{GetCurrentProcessId, ResetEvent, WaitForMultipleObjects},
    },
};

#[link(name = "kernel32")]
#[allow(unsafe_code)]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> HANDLE;
    fn SetStdHandle(which: u32, handle: HANDLE) -> i32;
}
const STD_INPUT: u32 = (-10i32) as u32;
const STD_OUTPUT: u32 = (-11i32) as u32;
const STD_ERROR: u32 = (-12i32) as u32;

fn unavailable() -> io::Error {
    io::Error::other("worker stream unavailable")
}

/// The caller is the admitted worker, before any CLI business code starts.
/// The dedicated Client is authenticated by the native service and switched
/// to binary mode only after the worker attach operation succeeds.
pub(super) struct WorkerStdio {
    stop: Arc<Event>,
    input_read: Option<Handle>,
    output_write: Option<Handle>,
    error_write: Option<Handle>,
    previous: [usize; 3],
    output_threads: Vec<JoinHandle<io::Result<()>>>,
    sender: Option<SyncSender<Frame>>,
    transmit: Option<JoinHandle<io::Result<()>>>,
    installed: bool,
}
static WORKER: Mutex<Option<WorkerStdio>> = Mutex::new(None);

/// This entry is called only after actual worker admission. No DTO, path or
/// user-provided identity constructs an authenticated Client.
pub(crate) fn start_current_cli_worker(admission: &AppWriterAdmission) -> crate::error::Result<()> {
    let mut state = WORKER
        .lock()
        .map_err(|_| crate::Error::HandoffOutcomeUnknown)?;
    if state.is_some() {
        return Err(crate::Error::IpcConflict);
    }
    *state = Some(WorkerStdio::start(admission).map_err(|_| crate::Error::Unavailable)?);
    Ok(())
}

/// Must be called explicitly before main invokes process::exit. The output
/// owner is taken once; any failure remains failure, never a replayed grant.
pub(crate) fn finish_current_cli_worker(code: i32) -> crate::error::Result<i32> {
    let owner = WORKER
        .lock()
        .map_err(|_| crate::Error::HandoffOutcomeUnknown)?
        .take()
        .ok_or(crate::Error::PrerequisiteUnavailable)?;
    owner
        .finish(code)
        .map_err(|_| crate::Error::HandoffOutcomeUnknown)?;
    Ok(code)
}

#[allow(unsafe_code)]
impl WorkerStdio {
    pub(super) fn start(admission: &AppWriterAdmission) -> io::Result<Self> {
        admission.recheck().map_err(|_| unavailable())?;
        let mut client = Client::connect(Role::Cli).map_err(|_| unavailable())?;
        let _: ipc::Empty = client
            .request("cli-worker-attach", &ipc::Empty {})
            .map_err(|_| unavailable())?;
        let client = Arc::new(client);
        let (out_read, out_write) = local_output()?;
        let (err_read, err_write) = local_output()?;
        let (input_read, input_write) = local_input()?;
        let stop = Arc::new(Event::new().map_err(|_| unavailable())?);
        let (sender, receiver) = cli_stream_wire::channel();
        let send_client = Arc::clone(&client);
        let transmit = thread::spawn(move || {
            let mut encoder = Encoder::new(send_client.stream_writer(), Direction::Output);
            while let Ok(frame) = receiver.recv() {
                encoder.send(frame)?;
            }
            Ok(())
        });
        let mut output_threads = Vec::new();
        for (read, is_err) in [(out_read, false), (err_read, true)] {
            let stop = Arc::clone(&stop);
            let sender = sender.clone();
            output_threads.push(thread::spawn(move || {
                output_pump(read, stop, sender, is_err)
            }));
        }
        let input_client = Arc::clone(&client);
        // A blocked stdin write never blocks finish; it owns only this client
        // reference and its private write handle until process termination.
        thread::spawn(move || {
            let _ = input_pump(input_client, input_write);
        });
        let previous = unsafe {
            [
                GetStdHandle(STD_INPUT) as usize,
                GetStdHandle(STD_OUTPUT) as usize,
                GetStdHandle(STD_ERROR) as usize,
            ]
        };
        let mut owner = Self {
            stop,
            input_read: Some(input_read),
            output_write: Some(out_write),
            error_write: Some(err_write),
            previous,
            output_threads,
            sender: Some(sender),
            transmit: Some(transmit),
            installed: false,
        };
        let new = [
            owner.input_read.as_ref().ok_or_else(unavailable)?.raw(),
            owner.output_write.as_ref().ok_or_else(unavailable)?.raw(),
            owner.error_write.as_ref().ok_or_else(unavailable)?.raw(),
        ];
        for (index, which) in [STD_INPUT, STD_OUTPUT, STD_ERROR].into_iter().enumerate() {
            if unsafe { SetStdHandle(which, new[index]) } == 0 {
                for (old_index, old_which) in [STD_INPUT, STD_OUTPUT, STD_ERROR]
                    .into_iter()
                    .enumerate()
                    .take(index)
                {
                    unsafe {
                        SetStdHandle(old_which, previous[old_index] as HANDLE);
                    }
                }
                return Err(unavailable());
            }
        }
        owner.installed = true;
        Ok(owner)
    }

    pub(super) fn finish(mut self, code: i32) -> io::Result<()> {
        io::stdout().flush()?;
        io::stderr().flush()?;
        // Disable this process's output handles before establishing the stop
        // cutoff. Descendant writers may survive, but cannot extend snapshots.
        for which in [STD_INPUT, STD_OUTPUT, STD_ERROR] {
            if unsafe { SetStdHandle(which, ptr::null_mut()) } == 0 {
                return Err(unavailable());
            }
        }
        self.installed = false;
        self.input_read.take();
        self.output_write.take();
        self.error_write.take();
        self.stop.set().map_err(|_| unavailable())?;
        let mut failure = false;
        for handle in self.output_threads.drain(..) {
            if !matches!(handle.join(), Ok(Ok(()))) {
                failure = true;
            }
        }
        if failure {
            self.sender.take();
            return Err(unavailable());
        }
        self.sender
            .take()
            .ok_or_else(unavailable)?
            .send(Frame::Exit(code))
            .map_err(|_| unavailable())?;
        match self.transmit.take().ok_or_else(unavailable)?.join() {
            Ok(value) => value,
            Err(_) => Err(unavailable()),
        }
    }
}
#[allow(unsafe_code)]
impl Drop for WorkerStdio {
    fn drop(&mut self) {
        if self.installed {
            for (index, which) in [STD_INPUT, STD_OUTPUT, STD_ERROR].into_iter().enumerate() {
                unsafe {
                    SetStdHandle(which, self.previous[index] as HANDLE);
                }
            }
        }
        let _ = self.stop.set();
    }
}

#[allow(unsafe_code)]
fn local_input() -> io::Result<(Handle, Handle)> {
    let mut read = ptr::null_mut();
    let mut write = ptr::null_mut();
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 65536) } == 0 {
        return Err(unavailable());
    }
    let read = Handle::owned(read).map_err(|_| unavailable());
    let write = Handle::owned(write).map_err(|_| unavailable());
    let read = read?;
    let write = write?;
    // A child must not inherit the private writer, otherwise it keeps its own
    // stdin alive and never observes EOF after the frontend closes input.
    if unsafe { SetHandleInformation(write.raw(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(unavailable());
    }
    Ok((read, write))
}

#[allow(unsafe_code)]
fn local_output() -> io::Result<(Handle, Handle)> {
    let name = format!(
        "\\\\.\\pipe\\pinvou-cli-local-{}",
        nonce().map_err(|_| unavailable())?
    );
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let read = Handle::owned(unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            ptr::null(),
        )
    })
    .map_err(|_| unavailable())?;
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let write = Handle::owned(unsafe {
        CreateFileW(
            wide.as_ptr(),
            0x40000000,
            0,
            &attrs,
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        )
    })
    .map_err(|_| unavailable())?;
    let mut pid = 0;
    if unsafe { GetNamedPipeClientProcessId(read.raw(), &mut pid) } == 0
        || pid != unsafe { GetCurrentProcessId() }
    {
        return Err(unavailable());
    }
    // The handle is overlapped, so even this already-connected path supplies
    // valid native operation storage; drain any unexpected pending request.
    let ready = Event::new().map_err(|_| unavailable())?;
    let mut operation = Box::<OVERLAPPED>::default();
    operation.hEvent = ready.raw();
    if unsafe { ConnectNamedPipe(read.raw(), &mut *operation) } == 0 {
        let error = unsafe { GetLastError() };
        if error == ERROR_IO_PENDING {
            unsafe { CancelIoEx(read.raw(), &*operation) };
            let mut ignored = 0;
            unsafe { GetOverlappedResult(read.raw(), &*operation, &mut ignored, 1) };
            return Err(unavailable());
        }
        if error != ERROR_PIPE_CONNECTED {
            return Err(unavailable());
        }
    }
    Ok((read, write))
}

fn output_pump(
    read: Handle,
    stop: Arc<Event>,
    sender: SyncSender<Frame>,
    stderr: bool,
) -> io::Result<()> {
    let mut operation = OutputRead::new(&read)?;
    let send = |bytes: &[u8]| -> io::Result<()> {
        let data = Data::new(bytes.to_vec())?;
        sender
            .send(if stderr {
                Frame::Stderr(data)
            } else {
                Frame::Stdout(data)
            })
            .map_err(|_| unavailable())
    };
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match operation.read(&mut buffer, &stop)? {
            ReadResult::Bytes(n) => {
                send(&buffer[..n])?;
                if stop.is_set().map_err(|_| unavailable())? {
                    operation.drain_snapshot(&send)?;
                    break;
                }
            }
            ReadResult::Stopped => {
                operation.drain_snapshot(&send)?;
                break;
            }
            ReadResult::Eof => break,
        }
    }
    sender
        .send(if stderr {
            Frame::StderrEof
        } else {
            Frame::StdoutEof
        })
        .map_err(|_| unavailable())
}

#[allow(unsafe_code)]
fn input_pump(client: Arc<Client>, write: Handle) -> io::Result<()> {
    let mut decoder = Decoder::new(client.stream_reader(), Direction::Input);
    while let Some(frame) = decoder.receive()? {
        match frame {
            Frame::Input(data) => {
                let mut bytes = data.as_slice();
                while !bytes.is_empty() {
                    let mut written = 0;
                    if unsafe {
                        WriteFile(
                            write.raw(),
                            bytes.as_ptr(),
                            bytes.len() as u32,
                            &mut written,
                            ptr::null_mut(),
                        )
                    } == 0
                        || written == 0
                        || written as usize > bytes.len()
                    {
                        return Err(unavailable());
                    }
                    bytes = &bytes[written as usize..];
                }
            }
            Frame::InputEof => return Ok(()),
            _ => return Err(unavailable()),
        }
    }
    Ok(())
}

/// `Stopped` means no bytes were completed for this operation. `Bytes` may be
/// returned after stop was signalled and must still be forwarded before EOF.
enum ReadResult {
    Bytes(usize),
    Stopped,
    Eof,
}

struct OutputRead<'a> {
    pipe: &'a Handle,
    ready: Event,
    overlapped: Box<OVERLAPPED>,
    pending: bool,
}
impl<'a> OutputRead<'a> {
    fn new(pipe: &'a Handle) -> io::Result<Self> {
        let ready = Event::new().map_err(|_| io::Error::other("event unavailable"))?;
        let mut overlapped = Box::<OVERLAPPED>::default();
        overlapped.hEvent = ready.raw();
        Ok(Self {
            pipe,
            ready,
            overlapped,
            pending: false,
        })
    }

    /// A finish cutoff is taken only by the sole reader of a pipe that this
    /// process created with FILE_FLAG_OVERLAPPED. No external handle is an
    /// acceptable input. The caller drains exactly this many bytes and never
    /// waits for descendants to close their write handles.
    #[allow(unsafe_code)]
    fn remaining_snapshot(&self) -> io::Result<u32> {
        if self.pending {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "read pending"));
        }
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                self.pipe.raw(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            let error = unsafe { GetLastError() };
            if error == ERROR_BROKEN_PIPE {
                return Ok(0);
            }
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        Ok(available)
    }

    /// Drain only the bytes observed at the cutoff. Later writes are not part
    /// of the main command's output and do not extend this loop.
    fn drain_snapshot(&mut self, mut send: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<()> {
        let mut remaining = self.remaining_snapshot()? as usize;
        let never_stop = Event::new().map_err(|_| io::Error::other("event unavailable"))?;
        let mut buffer = [0u8; 16 * 1024];
        while remaining != 0 {
            let length = remaining.min(buffer.len());
            match self.read(&mut buffer[..length], &never_stop)? {
                ReadResult::Bytes(count) if count != 0 && count <= length => {
                    send(&buffer[..count])?;
                    remaining -= count;
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "snapshot incomplete",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Caller must pass a dedicated overlapped pipe handle, and keep the
    /// buffer alive until completion or cancellation has actually completed.
    #[allow(unsafe_code)]
    fn read(&mut self, buffer: &mut [u8], stop: &Event) -> io::Result<ReadResult> {
        if buffer.is_empty() || buffer.len() > 16 * 1024 || self.pending {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid read"));
        }
        if stop
            .is_set()
            .map_err(|_| io::Error::other("event unavailable"))?
        {
            return Ok(ReadResult::Stopped);
        }
        if unsafe { ResetEvent(self.ready.raw()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = 0;
        let started = unsafe {
            ReadFile(
                self.pipe.raw(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut bytes,
                &mut *self.overlapped,
            )
        };
        if started != 0 {
            return Ok(if bytes == 0 {
                ReadResult::Eof
            } else {
                ReadResult::Bytes(bytes as usize)
            });
        }
        match unsafe { GetLastError() } {
            ERROR_BROKEN_PIPE => return Ok(ReadResult::Eof),
            ERROR_IO_PENDING => self.pending = true,
            error => return Err(io::Error::from_raw_os_error(error as i32)),
        }
        let handles = [self.ready.raw(), stop.raw()];
        let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) };
        if wait != WAIT_OBJECT_0 && wait != WAIT_OBJECT_0 + 1 {
            unsafe {
                CancelIoEx(self.pipe.raw(), &*self.overlapped);
            }
            unsafe {
                GetOverlappedResult(self.pipe.raw(), &*self.overlapped, &mut bytes, 1);
            }
            self.pending = false;
            return Err(io::Error::other("read wait failed"));
        }
        if wait == WAIT_OBJECT_0 + 1 {
            // Cancellation is only a request. The result below is authoritative;
            // a racing successful completion must not be discarded.
            unsafe {
                CancelIoEx(self.pipe.raw(), &*self.overlapped);
            }
        }
        let result =
            unsafe { GetOverlappedResult(self.pipe.raw(), &*self.overlapped, &mut bytes, 1) };
        let error = if result == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        self.pending = false;
        match (result != 0, error) {
            (true, _) if bytes != 0 => Ok(ReadResult::Bytes(bytes as usize)),
            (true, _) => Ok(ReadResult::Eof),
            (false, ERROR_OPERATION_ABORTED) => Ok(ReadResult::Stopped),
            (false, ERROR_BROKEN_PIPE) => Ok(ReadResult::Eof),
            (false, error) => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
}
impl Drop for OutputRead<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.pending {
            unsafe {
                CancelIoEx(self.pipe.raw(), &*self.overlapped);
            }
            let mut bytes = 0;
            // Drain the exact request before its OVERLAPPED and event are freed.
            unsafe {
                GetOverlappedResult(self.pipe.raw(), &*self.overlapped, &mut bytes, 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(unsafe_code)]
    fn output_finish_drains_snapshot_without_waiting_for_open_writer() {
        let (read, write) = local_output().unwrap();
        let stop = Arc::new(Event::new().unwrap());
        let (sender, receiver) = cli_stream_wire::channel();
        let bytes = b"last bytes";
        let mut written = 0;
        assert_ne!(
            unsafe {
                WriteFile(
                    write.raw(),
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(written as usize, bytes.len());
        stop.set().unwrap();
        let pump_stop = Arc::clone(&stop);
        let pump = thread::spawn(move || output_pump(read, pump_stop, sender, false));
        assert!(
            matches!(receiver.recv().unwrap(), Frame::Stdout(data) if data.as_slice() == bytes)
        );
        assert!(matches!(receiver.recv().unwrap(), Frame::StdoutEof));
        pump.join().unwrap().unwrap();
        // The real writer was deliberately still open at the finish cutoff.
        drop(write);
    }
    #[test]
    #[allow(unsafe_code)]
    fn stdin_read_end_is_inheritable_but_private_writer_is_not() {
        use windows_sys::Win32::Foundation::GetHandleInformation;
        let (read, write) = local_input().unwrap();
        let mut read_flags = 0;
        let mut write_flags = 0;
        assert_ne!(
            unsafe { GetHandleInformation(read.raw(), &mut read_flags) },
            0
        );
        assert_ne!(
            unsafe { GetHandleInformation(write.raw(), &mut write_flags) },
            0
        );
        assert_ne!(read_flags & HANDLE_FLAG_INHERIT, 0);
        assert_eq!(write_flags & HANDLE_FLAG_INHERIT, 0);
    }
}
