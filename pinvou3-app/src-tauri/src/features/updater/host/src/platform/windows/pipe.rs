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
    ptr,
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
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
            GetNamedPipeClientSessionId, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
            PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
        },
        Threading::{CreateEventW, SetEvent, WaitForMultipleObjects, WaitForSingleObject},
    },
};
const IDLE: Duration = Duration::from_secs(15);
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
        deadline: Instant,
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
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Unavailable);
            }
            match unsafe {
                WaitForMultipleObjects(
                    length,
                    handles.as_ptr(),
                    0,
                    remaining.as_millis().min(u32::MAX as u128) as u32,
                )
            } {
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
impl Pipe {
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
        operation.complete(started, error, Instant::now() + IDLE, stop, None)?;
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
                operation.complete(started, error, Instant::now() + IDLE, &ready, None),
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
