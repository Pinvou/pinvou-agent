//! Local overlapped byte-pipe framing with bounded IO and drained cancellation.
//! This transport does NOT authenticate a peer: native role adapters must bind
//! process/token/image/SCM before command dispatch or using any public reply.
use super::{handle::Handle, security::NativeSecurity};
use crate::{
    error::{Error, Result},
    json,
    profile::Seed,
};
use std::{
    io::{self, Read, Write},
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, IO_STATUS_BLOCK, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
            GetNamedPipeClientSessionId, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
        },
        Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject},
    },
};
const IDLE: Duration = Duration::from_secs(15);

#[repr(C)]
#[derive(Default)]
struct PipeLocalInformation {
    pipe_type: u32,
    configuration: u32,
    maximum_instances: u32,
    current_instances: u32,
    inbound_quota: u32,
    read_available: u32,
    outbound_quota: u32,
    write_available: u32,
    state: u32,
    end: u32,
}

#[allow(unsafe_code)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        file: windows_sys::Win32::Foundation::HANDLE,
        status: *mut IO_STATUS_BLOCK,
        information: *mut core::ffi::c_void,
        length: u32,
        information_class: i32,
    ) -> i32;
}

// The native query is documented as synchronous. If a platform violates
// that contract, keep its output buffers alive and disable further queries;
// never release memory that the kernel could still be writing.
static PIPE_QUERY_LOCK: Mutex<()> = Mutex::new(());
static PIPE_QUERY_UNUSABLE: AtomicBool = AtomicBool::new(false);
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
pub(super) struct Event {
    handle: Handle,
}
impl Event {
    #[allow(unsafe_code)]
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            handle: Handle::owned(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })?,
        })
    }
    #[allow(unsafe_code)]
    pub(super) fn set(&self) -> Result<()> {
        if unsafe { SetEvent(self.handle.raw()) } == 0 {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
    #[allow(unsafe_code)]
    pub(super) fn is_set(&self) -> Result<bool> {
        match unsafe { WaitForSingleObject(self.handle.raw(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(Error::Unavailable),
        }
    }
    pub(super) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.raw()
    }
}
struct Operation<'a> {
    pipe: &'a Handle,
    event: Event,
    overlapped: Box<OVERLAPPED>,
    pending: bool,
}
impl<'a> Operation<'a> {
    fn new(pipe: &'a Handle) -> Result<Self> {
        let event = Event::new()?;
        let mut overlapped = Box::<OVERLAPPED>::default();
        overlapped.hEvent = event.raw();
        Ok(Self {
            pipe,
            event,
            overlapped,
            pending: false,
        })
    }
    #[allow(unsafe_code)]
    fn complete(
        &mut self,
        started: i32,
        error: u32,
        deadline: Option<Instant>,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<u32> {
        if started == 0 {
            match error {
                ERROR_IO_PENDING => self.pending = true,
                ERROR_BROKEN_PIPE | ERROR_NO_DATA => return Err(Error::IpcInvalid),
                _ => return Err(Error::Unavailable),
            }
        }
        if self.pending {
            let mut handles = [self.event.raw(), stop.raw(), ptr::null_mut()];
            let length = if let Some(peer) = peer {
                handles[2] = peer;
                3
            } else {
                2
            };
            let timeout = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(Error::Unavailable);
                    }
                    remaining.as_millis().min((u32::MAX - 1) as u128) as u32
                }
                None => u32::MAX, // INFINITE: stop and peer handles still cancel.
            };
            match unsafe { WaitForMultipleObjects(length, handles.as_ptr(), 0, timeout) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => return Err(Error::Unavailable),
                _ => return Err(Error::RestartCanceled),
            }
        }
        let mut bytes = 0;
        if unsafe { GetOverlappedResult(self.pipe.raw(), &*self.overlapped, &mut bytes, 0) } == 0 {
            return Err(Error::Unavailable);
        }
        self.pending = false;
        if stop.is_set()? {
            return Err(Error::RestartCanceled);
        }
        Ok(bytes)
    }
}
impl Drop for Operation<'_> {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.pending {
            // Cancel does not synchronously complete IO. Drain the SAME native
            // request before freeing OVERLAPPED/event/caller buffer. The pipe
            // and every borrowed buffer remain live through this destructor.
            unsafe {
                CancelIoEx(self.pipe.raw(), &*self.overlapped);
            }
            let mut bytes = 0;
            unsafe {
                GetOverlappedResult(self.pipe.raw(), &*self.overlapped, &mut bytes, 1);
            }
        }
    }
}
pub(super) struct Pipe {
    handle: Handle,
    server: bool,
}
/// An authenticated protocol's sensitive binary frame. The allocation is
/// cleared on both successful consumption and partial-read failure.
pub(super) struct SensitiveFrame(Vec<u8>);
impl SensitiveFrame {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.0
    }
    #[allow(unsafe_code)]
    fn clear(&mut self) {
        for byte in &mut self.0 {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}
/// Stream codec adapters are restricted to a dedicated already-authenticated
/// pipe. One reader and one writer may operate concurrently; the connection
/// owner must never attach a second reader or writer.
pub(super) struct StreamReader<'a> {
    pipe: &'a Pipe,
    stop: &'a Event,
    peer: &'a super::process::RetainedProcess,
    frame: Option<SensitiveFrame>,
    offset: usize,
}
impl Read for StreamReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self
            .frame
            .as_ref()
            .is_none_or(|frame| self.offset == frame.0.len())
        {
            self.frame = Some(
                self.pipe
                    .read_stream_packet(self.stop, self.peer.raw())
                    .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "stream closed"))?,
            );
            self.offset = 0;
        }
        let frame = self
            .frame
            .as_ref()
            .ok_or_else(|| io::Error::other("stream unavailable"))?;
        let count = output.len().min(frame.0.len() - self.offset);
        output[..count].copy_from_slice(&frame.0[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}
pub(super) struct StreamWriter<'a> {
    pipe: &'a Pipe,
    stop: &'a Event,
    peer: &'a super::process::RetainedProcess,
    frame: SensitiveFrame,
    poisoned: bool,
}
impl Write for StreamWriter<'_> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if self.poisoned
            || self
                .frame
                .0
                .len()
                .checked_add(input.len())
                .is_none_or(|n| n > 16 * 1024 + 5)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "stream frame too large",
            ));
        }
        self.frame.0.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.frame.0.is_empty() {
            return Ok(());
        }
        if self.poisoned {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream closed"));
        }
        if self
            .pipe
            .write_stream_packet(&self.frame.0, self.stop, self.peer.raw())
            .is_err()
        {
            self.poisoned = true;
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream closed"));
        }
        self.frame.clear();
        self.frame.0.clear();
        Ok(())
    }
}
impl Drop for SensitiveFrame {
    fn drop(&mut self) {
        self.clear();
    }
}
pub(super) fn for_sensitive_frames(
    units: &[u16],
    mut send: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    if units.is_empty() || units.len() > crate::ipc::CLI_ENV_MAX_UNITS {
        return Err(Error::IpcInvalid);
    }
    for chunk in units.chunks(json::FRAME_MAX / 2) {
        let mut bytes = SensitiveFrame(Vec::with_capacity(chunk.len() * 2));
        for unit in chunk {
            bytes.0.extend_from_slice(&unit.to_le_bytes());
        }
        send(&bytes.0)?;
    }
    Ok(())
}
impl Pipe {
    pub(super) fn stream_reader<'a>(
        &'a self,
        stop: &'a Event,
        peer: &'a super::process::RetainedProcess,
    ) -> StreamReader<'a> {
        StreamReader {
            pipe: self,
            stop,
            peer,
            frame: None,
            offset: 0,
        }
    }
    pub(super) fn stream_writer<'a>(
        &'a self,
        stop: &'a Event,
        peer: &'a super::process::RetainedProcess,
    ) -> StreamWriter<'a> {
        StreamWriter {
            pipe: self,
            stop,
            peer,
            frame: SensitiveFrame(Vec::new()),
            poisoned: false,
        }
    }
    /// Fixed-purpose keeper transport only. The service creates its sole server
    /// instance; the restricted child inherits an exact reduced server handle.
    /// These operations do not authenticate that child or grant writer custody.
    pub(super) fn keeper_listener(seed: &Seed) -> Result<Self> {
        super::writer_keeper::create_listener(seed)
    }
    pub(super) fn keeper_connect(seed: &Seed) -> Result<Self> {
        Self::connect_named(&super::writer_keeper::pipe_name(seed))
    }
    pub(super) fn keeper_from_owned_server(handle: Handle) -> Self {
        // Only writer_keeper's native fixed-profile constructors call this.
        Self {
            handle,
            server: true,
        }
    }
    #[allow(unsafe_code)]
    fn create_named(name: &str, first: bool, security: Option<&NativeSecurity>) -> Result<Self> {
        let name = wide(name);
        let attributes = security.map(NativeSecurity::attributes);
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX
                    | FILE_FLAG_OVERLAPPED
                    | if first {
                        FILE_FLAG_FIRST_PIPE_INSTANCE
                    } else {
                        0
                    },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                64,
                (json::FRAME_MAX + 4) as u32,
                (json::FRAME_MAX + 4) as u32,
                15000,
                attributes.as_ref().map_or(ptr::null(), |v| v as *const _),
            )
        };
        Ok(Self {
            handle: Handle::owned(handle)?,
            server: true,
        })
    }
    pub(super) fn listener(seed: &Seed, first: bool) -> Result<Self> {
        let pipe = Self::create_named(&seed.pipe_name(), first, Some(&NativeSecurity::pipe()?))?;
        super::security::inspect_pipe(pipe.raw())?;
        Ok(pipe)
    }
    #[allow(unsafe_code)]
    pub(super) fn connect(seed: &Seed) -> Result<Self> {
        Self::connect_named(&seed.pipe_name())
    }
    #[allow(unsafe_code)]
    fn connect_named(name: &str) -> Result<Self> {
        let name = wide(name);
        if unsafe { WaitNamedPipeW(name.as_ptr(), 15000) } == 0 {
            return Err(Error::Unavailable);
        }
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                0x0012_0083,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                ptr::null_mut(),
            )
        };
        Ok(Self {
            handle: Handle::owned(handle)?,
            server: false,
        })
    }
    pub(super) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.raw()
    }
    #[cfg(test)]
    pub(super) fn test_pair() -> Result<(Self, Self)> {
        let name = format!(r"\\.\pipe\Pinvou.CliWatchUnit.{}", nonce()?);
        let server = Self::create_named(&name, true, None)?;
        let client = Self::connect_named(&name)?;
        server.accept(&Event::new()?)?;
        Ok((server, client))
    }
    /// Observe the server end without consuming queued stream bytes. Only an
    /// exact connected result is usable; unknown states fail closed.
    #[allow(unsafe_code)]
    pub(super) fn client_connected(&self) -> Result<bool> {
        const FILE_PIPE_LOCAL_INFORMATION: i32 = 24;
        const STATUS_PENDING: i32 = 0x103;
        const FILE_PIPE_CONNECTED_STATE: u32 = 3;
        const FILE_PIPE_SERVER_END: u32 = 1;
        if !self.server {
            return Err(Error::IpcInvalid);
        }
        let _guard = PIPE_QUERY_LOCK.lock().map_err(|_| Error::Unavailable)?;
        if PIPE_QUERY_UNUSABLE.load(Ordering::Acquire) {
            return Err(Error::Unavailable);
        }
        let mut status = Box::<IO_STATUS_BLOCK>::default();
        let mut info = Box::<PipeLocalInformation>::default();
        // SAFETY: the pipe and both boxed outputs remain valid throughout the
        // synchronous native query; the buffer matches the SDK structure.
        let result = unsafe {
            NtQueryInformationFile(
                self.handle.raw(),
                &mut *status,
                (&mut *info as *mut PipeLocalInformation).cast(),
                std::mem::size_of::<PipeLocalInformation>() as u32,
                FILE_PIPE_LOCAL_INFORMATION,
            )
        };
        if result == STATUS_PENDING {
            PIPE_QUERY_UNUSABLE.store(true, Ordering::Release);
            let _ = Box::leak(status);
            let _ = Box::leak(info);
            return Err(Error::Unavailable);
        }
        // SAFETY: a non-pending return has completed the synchronous query,
        // which writes the Status arm of this union.
        let final_status = unsafe { status.Anonymous.Status };
        if final_status == STATUS_PENDING {
            PIPE_QUERY_UNUSABLE.store(true, Ordering::Release);
            let _ = Box::leak(status);
            let _ = Box::leak(info);
            return Err(Error::Unavailable);
        }
        if result != 0
            || final_status != 0
            || status.Information != std::mem::size_of::<PipeLocalInformation>()
            || info.end != FILE_PIPE_SERVER_END
        {
            return Err(Error::Unavailable);
        }
        Ok(info.state == FILE_PIPE_CONNECTED_STATE)
    }
    #[allow(unsafe_code)]
    pub(super) fn accept(&self, stop: &Event) -> Result<()> {
        if !self.server || stop.is_set()? {
            return Err(Error::RestartCanceled);
        }
        let mut operation = Operation::new(&self.handle)?;
        let started = unsafe { ConnectNamedPipe(self.handle.raw(), &mut *operation.overlapped) };
        let error = if started == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        if started == 0 && error == ERROR_PIPE_CONNECTED {
            return Ok(());
        }
        operation.complete(started, error, Some(Instant::now() + IDLE), stop, None)?;
        Ok(())
    }
    #[allow(unsafe_code)]
    pub(super) fn disconnect(&self) {
        if self.server {
            unsafe {
                DisconnectNamedPipe(self.handle.raw());
            }
        }
    }
    #[allow(unsafe_code)]
    pub(super) fn peer_process_id(&self) -> Result<u32> {
        let mut id = 0;
        let ok = if self.server {
            unsafe { GetNamedPipeClientProcessId(self.handle.raw(), &mut id) }
        } else {
            unsafe { GetNamedPipeServerProcessId(self.handle.raw(), &mut id) }
        };
        if ok == 0 || id == 0 {
            Err(Error::PeerRefused)
        } else {
            Ok(id)
        }
    }
    /// Native identification only, with a thread-affine unconditional revert.
    /// Called after reading the original hello. The returned facts carry no
    /// client role or permission; dispatch must still bind held process/image.
    #[allow(unsafe_code)]
    pub(super) fn identify_client(&self) -> Result<super::process::TokenFacts> {
        use windows_sys::Win32::{
            Security::{
                GetTokenInformation, RevertToSelf, SecurityIdentification, TOKEN_QUERY,
                TokenImpersonation, TokenImpersonationLevel, TokenType,
            },
            System::{
                Pipes::ImpersonateNamedPipeClient,
                Threading::{GetCurrentThread, OpenThreadToken},
            },
        };
        if !self.server {
            return Err(Error::PeerRefused);
        }
        if unsafe { ImpersonateNamedPipeClient(self.handle.raw()) } == 0 {
            return Err(Error::PeerRefused);
        }
        struct Revert(std::marker::PhantomData<std::rc::Rc<()>>);
        impl Drop for Revert {
            #[allow(unsafe_code)]
            fn drop(&mut self) {
                // Windows requires terminating if a failed revert could leave
                // this privileged worker running under an unintended token.
                if unsafe { RevertToSelf() } == 0 {
                    std::process::abort();
                }
            }
        }
        let revert = Revert(std::marker::PhantomData);
        let mut token = ptr::null_mut();
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
            return Err(Error::PeerRefused);
        }
        let token = Handle::owned(token)?;
        for (class, expected) in [
            (TokenType, TokenImpersonation),
            (TokenImpersonationLevel, SecurityIdentification),
        ] {
            let mut value = 0i32;
            let mut written = 0;
            if unsafe {
                GetTokenInformation(
                    token.raw(),
                    class,
                    (&mut value as *mut i32).cast(),
                    4,
                    &mut written,
                )
            } == 0
                || written != 4
                || value != expected
            {
                return Err(Error::PeerRefused);
            }
        }
        let facts = super::process::token_facts(&token, None)?;
        drop(token);
        drop(revert);
        Ok(facts)
    }
    #[allow(unsafe_code)]
    pub(super) fn peer_session_id(&self) -> Result<u32> {
        if !self.server {
            return Err(Error::PeerRefused);
        }
        let mut id = 0;
        if unsafe { GetNamedPipeClientSessionId(self.handle.raw(), &mut id) } == 0 {
            return Err(Error::PeerRefused);
        }
        Ok(id)
    }
    #[allow(unsafe_code)]
    fn read_exact(
        &self,
        mut bytes: &mut [u8],
        deadline: Instant,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<()> {
        self.read_exact_until(bytes, Some(deadline), stop, peer)
    }
    #[allow(unsafe_code)]
    fn read_exact_until(
        &self,
        mut bytes: &mut [u8],
        deadline: Option<Instant>,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            if stop.is_set()? {
                return Err(Error::RestartCanceled);
            }
            let mut operation = Operation::new(&self.handle)?;
            let mut immediate = 0;
            let started = unsafe {
                ReadFile(
                    self.handle.raw(),
                    bytes.as_mut_ptr(),
                    bytes.len() as u32,
                    &mut immediate,
                    &mut *operation.overlapped,
                )
            };
            // Capture immediately, before clocks/logging/other SDK work can
            // overwrite the thread's last-error or hide a pending request.
            let error = if started == 0 {
                unsafe { GetLastError() }
            } else {
                0
            };
            let count = operation.complete(started, error, deadline, stop, peer)? as usize;
            if count == 0 || count > bytes.len() {
                return Err(Error::IpcInvalid);
            }
            bytes = &mut bytes[count..];
        }
        Ok(())
    }
    #[allow(unsafe_code)]
    fn write_all(
        &self,
        mut bytes: &[u8],
        deadline: Instant,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<()> {
        self.write_all_until(bytes, Some(deadline), stop, peer)
    }
    #[allow(unsafe_code)]
    fn write_all_until(
        &self,
        mut bytes: &[u8],
        deadline: Option<Instant>,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<()> {
        while !bytes.is_empty() {
            if stop.is_set()? {
                return Err(Error::RestartCanceled);
            }
            let mut operation = Operation::new(&self.handle)?;
            let mut immediate = 0;
            let started = unsafe {
                WriteFile(
                    self.handle.raw(),
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    &mut immediate,
                    &mut *operation.overlapped,
                )
            };
            // Capture immediately, before clocks/logging/other SDK work can
            // overwrite the thread's last-error or hide a pending request.
            let error = if started == 0 {
                unsafe { GetLastError() }
            } else {
                0
            };
            let count = operation.complete(started, error, deadline, stop, peer)? as usize;
            if count == 0 || count > bytes.len() {
                return Err(Error::IpcInvalid);
            }
            bytes = &bytes[count..];
        }
        Ok(())
    }
    pub(super) fn read_frame(
        &self,
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<Vec<u8>> {
        let deadline = Instant::now() + IDLE;
        let mut header = [0u8; 4];
        self.read_exact(&mut header, deadline, stop, peer)?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > json::FRAME_MAX {
            return Err(Error::IpcInvalid);
        }
        let mut bytes = vec![0u8; length];
        self.read_exact(&mut bytes, deadline, stop, peer)?;
        // Strict Document parser/typed closed command validation follows only
        // after native authentication. UTF8 failure cannot enter dispatch.
        if std::str::from_utf8(&bytes).is_err() {
            return Err(Error::IpcInvalid);
        }
        Ok(bytes)
    }
    /// Used only after the service has authenticated a held CLI worker. A
    /// quiet worker admission may outlive any fixed interval, but stop and
    /// the actual peer process still terminate each pending operation.
    pub(super) fn read_worker_control(
        &self,
        stop: &Event,
        peer: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<Vec<u8>> {
        let mut header = [0u8; 4];
        self.read_exact_until(&mut header, None, stop, Some(peer))?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > json::FRAME_MAX {
            return Err(Error::IpcInvalid);
        }
        let mut bytes = vec![0u8; length];
        self.read_exact_until(&mut bytes, None, stop, Some(peer))?;
        if std::str::from_utf8(&bytes).is_err() {
            return Err(Error::IpcInvalid);
        }
        Ok(bytes)
    }
    /// Caller must first authenticate the live pipe peer and enter the fixed
    /// sensitive protocol state. Binary data bypasses JSON/UTF-8 conversion.
    pub(super) fn read_sensitive_frame(
        &self,
        stop: &Event,
        peer: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<SensitiveFrame> {
        let deadline = Instant::now() + IDLE;
        let mut header = [0u8; 4];
        self.read_exact(&mut header, deadline, stop, Some(peer))?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > json::FRAME_MAX {
            return Err(Error::IpcInvalid);
        }
        let mut bytes = SensitiveFrame(vec![0; length]);
        self.read_exact(&mut bytes.0, deadline, stop, Some(peer))?;
        Ok(bytes)
    }
    /// Send raw UTF-16LE in bounded frames on an already authenticated pipe.
    /// The temporary serialized bytes are never logged and are cleared after
    /// each write, including failures. No total transfer deadline is added.
    pub(super) fn write_sensitive_units(
        &self,
        units: &[u16],
        stop: &Event,
        peer: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<()> {
        for_sensitive_frames(units, |bytes| self.write_frame(bytes, stop, Some(peer)))
    }
    /// Dedicated authenticated CLI data connection. Unlike control messages,
    /// a quiet interactive or redirected stream has no overall idle deadline.
    /// The actual service-stop and peer-process handles remain cancellation
    /// inputs to each native overlapped operation.
    pub(super) fn read_stream_packet(
        &self,
        stop: &Event,
        peer: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<SensitiveFrame> {
        let mut header = [0u8; 4];
        self.read_exact_until(&mut header, None, stop, Some(peer))?;
        let length = u32::from_le_bytes(header) as usize;
        if length == 0 || length > 16 * 1024 + 5 {
            return Err(Error::IpcInvalid);
        }
        let mut frame = SensitiveFrame(vec![0; length]);
        self.read_exact_until(&mut frame.0, None, stop, Some(peer))?;
        Ok(frame)
    }
    pub(super) fn write_stream_packet(
        &self,
        bytes: &[u8],
        stop: &Event,
        peer: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<()> {
        if bytes.is_empty() || bytes.len() > 16 * 1024 + 5 {
            return Err(Error::IpcInvalid);
        }
        self.write_all_until(&(bytes.len() as u32).to_le_bytes(), None, stop, Some(peer))?;
        self.write_all_until(bytes, None, stop, Some(peer))
    }
    pub(super) fn write_frame(
        &self,
        bytes: &[u8],
        stop: &Event,
        peer: Option<windows_sys::Win32::Foundation::HANDLE>,
    ) -> Result<()> {
        if bytes.is_empty() || bytes.len() > json::FRAME_MAX {
            return Err(Error::IpcInvalid);
        }
        let deadline = Instant::now() + IDLE;
        self.write_all(&(bytes.len() as u32).to_le_bytes(), deadline, stop, peer)?;
        self.write_all(bytes, deadline, stop, peer)
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.disconnect();
    }
}
#[allow(unsafe_code)]
pub(super) fn nonce() -> Result<String> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let mut bytes = [0u8; 32];
    if unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        return Err(Error::Unavailable);
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn cli_stream_real_pipe_preserves_binary_and_terminal_ack() {
        use super::super::cli_stream_wire::{Data, Decoder, Direction, Encoder, Frame};
        use super::super::process::RetainedProcess;
        struct StopOnDrop(Arc<Event>);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                let _ = self.0.set();
            }
        }
        let name = format!(r"\\.\pipe\Pinvou.CliStreamUnit.{}", nonce().unwrap());
        let server = Pipe::create_named(&name, true, None).unwrap();
        let stop = Arc::new(Event::new().unwrap());
        let _stop = StopOnDrop(Arc::clone(&stop));
        let worker_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let peer = RetainedProcess::current().unwrap();
            server.accept(&worker_stop).unwrap();
            let mut reader = Decoder::new(
                server.stream_reader(&worker_stop, &peer),
                Direction::FrontendInput,
            );
            assert!(matches!(reader.receive().unwrap(), Some(Frame::InputEof)));
            let mut writer = Encoder::new(
                server.stream_writer(&worker_stop, &peer),
                Direction::FrontendOutput,
            );
            writer
                .send(Frame::Stdout(Data::new(vec![0xff, 0, 1]).unwrap()))
                .unwrap();
            writer.send(Frame::StdoutEof).unwrap();
            writer.send(Frame::StderrEof).unwrap();
            writer.send(Frame::OutputChallenge([b'a'; 64])).unwrap();
            writer.send(Frame::Exit(259)).unwrap();
            assert!(
                matches!(reader.receive().unwrap(), Some(Frame::OutputAck(value)) if value == [b'a'; 64])
            );
        });
        let client = Pipe::connect_named(&name).unwrap();
        let peer = RetainedProcess::current().unwrap();
        let mut writer = Encoder::new(client.stream_writer(&stop, &peer), Direction::FrontendInput);
        writer.send(Frame::InputEof).unwrap();
        let mut reader = Decoder::new(
            client.stream_reader(&stop, &peer),
            Direction::FrontendOutput,
        );
        assert!(
            matches!(reader.receive().unwrap(), Some(Frame::Stdout(data)) if data.as_slice() == [0xff, 0, 1])
        );
        assert!(matches!(reader.receive().unwrap(), Some(Frame::StdoutEof)));
        assert!(matches!(reader.receive().unwrap(), Some(Frame::StderrEof)));
        assert!(
            matches!(reader.receive().unwrap(), Some(Frame::OutputChallenge(value)) if value == [b'a'; 64])
        );
        assert!(matches!(reader.receive().unwrap(), Some(Frame::Exit(259))));
        writer.send(Frame::OutputAck([b'a'; 64])).unwrap();
        thread.join().unwrap();
    }
    #[test]
    fn sensitive_frame_clear_overwrites_its_owned_buffer() {
        let mut frame = SensitiveFrame(vec![1, 2, 3, 4]);
        frame.clear();
        assert!(frame.bytes().iter().all(|byte| *byte == 0));
    }
    #[test]
    #[allow(unsafe_code)]
    fn actual_overlapped_frame_echo_and_cancel_are_joined_without_host_authority() {
        let name = format!(
            r"\\.\pipe\Pinvou.T31.OrdinaryTransportQA.{}",
            nonce().unwrap()
        );
        let server = Pipe::create_named(&name, true, None).unwrap();
        let stop = Arc::new(Event::new().unwrap());
        let ready = stop.clone();
        let (pending_tx, pending_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            server.accept(&ready).unwrap();
            let bytes = server.read_frame(&ready, None).unwrap();
            assert_eq!(bytes, b"{\"x\":1}");
            let facts = server.identify_client().unwrap();
            assert!(
                facts.user
                    == super::super::process::RetainedProcess::current()
                        .unwrap()
                        .token
                        .user
            );
            let mut token = ptr::null_mut();
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::OpenThreadToken(
                        windows_sys::Win32::System::Threading::GetCurrentThread(),
                        windows_sys::Win32::Security::TOKEN_QUERY,
                        1,
                        &mut token,
                    )
                },
                0
            );
            assert_eq!(
                unsafe { GetLastError() },
                windows_sys::Win32::Foundation::ERROR_NO_TOKEN
            );
            server.write_frame(&bytes, &ready, None).unwrap();
            let mut byte = [0u8; 1];
            let mut operation = Operation::new(&server.handle).unwrap();
            let mut immediate = 0;
            #[allow(unsafe_code)]
            let started = unsafe {
                ReadFile(
                    server.handle.raw(),
                    byte.as_mut_ptr(),
                    1,
                    &mut immediate,
                    &mut *operation.overlapped,
                )
            };
            #[allow(unsafe_code)]
            let error = if started == 0 {
                unsafe { GetLastError() }
            } else {
                0
            };
            operation.pending = started == 0 && error == ERROR_IO_PENDING;
            assert!(operation.pending);
            pending_tx.send(()).unwrap();
            assert!(matches!(
                operation.complete(started, error, Some(Instant::now() + IDLE), &ready, None),
                Err(Error::RestartCanceled)
            ));
        });
        let client = Pipe::connect_named(&name).unwrap();
        client.write_frame(b"{\"x\":1}", &stop, None).unwrap();
        assert_eq!(client.read_frame(&stop, None).unwrap(), b"{\"x\":1}");
        pending_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        stop.set().unwrap();
        thread.join().unwrap();
        // Ordinary temporary named IO only. No protected host/role/SCM trust is
        // manufactured by this transport QA or its requested/observed peer PID.
    }
    #[test]
    fn actual_oversize_header_is_refused_before_body_allocation() {
        let name = format!(
            r"\\.\pipe\Pinvou.T31.OrdinaryTransportQA.{}",
            nonce().unwrap()
        );
        let server = Pipe::create_named(&name, true, None).unwrap();
        let stop = Arc::new(Event::new().unwrap());
        let waiter = stop.clone();
        let thread = std::thread::spawn(move || {
            server.accept(&waiter).unwrap();
            assert!(matches!(
                server.read_frame(&waiter, None),
                Err(Error::IpcInvalid)
            ));
        });
        let client = Pipe::connect_named(&name).unwrap();
        client
            .write_all(
                &((json::FRAME_MAX + 1) as u32).to_le_bytes(),
                Instant::now() + IDLE,
                &stop,
                None,
            )
            .unwrap();
        thread.join().unwrap();
    }
}
