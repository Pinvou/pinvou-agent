use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use pinvou_protocol::{FrameError, HelloClient, HelloServer, IpcMessage, encode_frame, read_frame};

use crate::{ControllerError, ControllerSession, HostPlatform, LocalEndpoint, RollingLog};

#[derive(Clone, Copy, Debug)]
pub struct LocalIpcPolicy {
    platform: HostPlatform,
}

impl LocalIpcPolicy {
    pub const fn for_platform(platform: HostPlatform) -> Self {
        Self { platform }
    }
    pub const fn rejects_remote_clients(self) -> bool {
        matches!(self.platform, HostPlatform::Windows)
    }
    pub const fn first_pipe_instance(self) -> bool {
        matches!(self.platform, HostPlatform::Windows)
    }
    pub const fn requires_logon_session_acl(self) -> bool {
        matches!(self.platform, HostPlatform::Windows)
    }
    pub const fn requires_peer_identity(self) -> bool {
        true
    }
    pub const fn has_tcp_listener(self) -> bool {
        false
    }
    pub const fn parent_mode(self) -> Option<u32> {
        if matches!(self.platform, HostPlatform::Linux) {
            Some(0o700)
        } else {
            None
        }
    }
    pub const fn socket_mode(self) -> Option<u32> {
        if matches!(self.platform, HostPlatform::Linux) {
            Some(0o600)
        } else {
            None
        }
    }
    pub const fn allows_abstract_socket(self) -> bool {
        false
    }
}

pub struct LocalIpcListener {
    inner: platform::Listener,
}

impl LocalIpcListener {
    pub fn bind(endpoint: &LocalEndpoint) -> Result<Self, ControllerError> {
        Ok(Self {
            inner: platform::Listener::bind(endpoint)?,
        })
    }

    pub fn serve_one(&mut self, session: &ControllerSession) -> Result<(), ControllerError> {
        self.spawn_worker(session, None)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn serve_one_blocking(
        &mut self,
        session: &ControllerSession,
    ) -> Result<(), ControllerError> {
        let mut connection = self.inner.accept()?;
        serve_connection(&mut connection, session)
    }

    pub(crate) fn serve_one_logged(
        &mut self,
        session: &ControllerSession,
        log: Arc<Mutex<RollingLog>>,
    ) -> Result<(), ControllerError> {
        self.spawn_worker(session, Some(log))
    }

    fn spawn_worker(
        &mut self,
        session: &ControllerSession,
        log: Option<Arc<Mutex<RollingLog>>>,
    ) -> Result<(), ControllerError> {
        let mut connection = self.inner.accept()?;
        let session = session.clone();
        std::thread::Builder::new()
            .name("pinvou-controller-client".into())
            .spawn(move || {
                if let Err(error) = serve_connection(&mut connection, &session)
                    && let Some(log) = log
                    && let Ok(mut log) = log.lock()
                {
                    let _ = writeln!(log, "client connection failed: {error}");
                    let _ = log.flush();
                }
            })?;
        Ok(())
    }
}

fn serve_connection(
    stream: &mut platform::Connection,
    session: &ControllerSession,
) -> Result<(), ControllerError> {
    let hello: HelloClient = match read_frame(stream) {
        Ok(hello) => hello,
        Err(error) => {
            let response = IpcMessage::error(
                None,
                serde_json::json!({"code": 3, "error": "protocol_version_mismatch"}),
            )
            .map_err(|_| ControllerError::InvalidMessage)?;
            stream.write_all(
                &encode_frame(&response).map_err(|_| ControllerError::InvalidMessage)?,
            )?;
            stream.flush()?;
            return Err(map_hello_error(error));
        }
    };
    // No request is acted on until the connected OS peer has been authenticated.
    stream.verify_peer()?;
    let answer: HelloServer = session.accept_hello(hello)?;
    stream.write_all(&encode_frame(&answer).map_err(|_| ControllerError::InvalidMessage)?)?;
    let mut handled_requests = false;
    loop {
        let request: IpcMessage = match read_frame(stream) {
            Ok(request) => request,
            Err(FrameError::Io) if handled_requests => return Ok(()),
            Err(_) => return Err(ControllerError::InvalidMessage),
        };
        let responses = session.handle_bound_many(request)?;
        for response in responses {
            stream.write_all(
                &encode_frame(&response).map_err(|_| ControllerError::InvalidMessage)?,
            )?;
        }
        stream.flush()?;
        handled_requests = true;
    }
}

fn map_hello_error(_: pinvou_protocol::FrameError) -> ControllerError {
    ControllerError::ProtocolMismatch
}

#[cfg(target_os = "linux")]
mod platform {
    use crate::{ControllerError, LocalEndpoint};
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::Path,
    };

    pub struct Listener {
        listener: UnixListener,
        path: std::path::PathBuf,
    }

    impl Listener {
        pub fn bind(endpoint: &LocalEndpoint) -> Result<Self, ControllerError> {
            let LocalEndpoint::UnixSocket(path) = endpoint else {
                return Err(ControllerError::UnsupportedPlatform);
            };
            let parent = path.parent().ok_or(ControllerError::PathUnavailable)?;
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            if path.exists() {
                remove_stale_socket(path)?;
            }
            let listener = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self {
                listener,
                path: path.clone(),
            })
        }

        pub fn accept(&self) -> Result<Connection, ControllerError> {
            let (stream, _) = self.listener.accept()?;
            Ok(Connection(stream))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub struct Connection(UnixStream);
    impl Connection {
        pub fn verify_peer(&self) -> Result<(), ControllerError> {
            use std::{mem::size_of, os::fd::AsRawFd};
            let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
            let mut len = size_of::<libc::ucred>() as libc::socklen_t;
            let result = unsafe {
                libc::getsockopt(
                    self.0.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    (&mut credentials as *mut libc::ucred).cast(),
                    &mut len,
                )
            };
            if result != 0 {
                return Err(ControllerError::Io(std::io::Error::last_os_error()));
            }
            if credentials.uid != unsafe { libc::geteuid() } {
                return Err(ControllerError::InvalidMessage);
            }
            Ok(())
        }
    }
    impl Read for Connection {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buffer)
        }
    }
    impl Write for Connection {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.write(buffer)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }

    fn remove_stale_socket(path: &Path) -> Result<(), ControllerError> {
        match UnixStream::connect(path) {
            Ok(_) => Err(ControllerError::AlreadyRunning),
            Err(_) => {
                std::fs::remove_file(path)?;
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use crate::{ControllerError, LocalEndpoint};
    use std::{
        ffi::OsStr,
        io::{Read, Write},
        os::windows::ffi::OsStrExt,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
        },
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_TYPE_BYTE, PIPE_WAIT,
        },
    };

    pub struct Listener {
        name: Vec<u16>,
        first: Option<HANDLE>,
    }
    impl Listener {
        pub fn bind(endpoint: &LocalEndpoint) -> Result<Self, ControllerError> {
            let LocalEndpoint::WindowsPipe(name) = endpoint else {
                return Err(ControllerError::UnsupportedPlatform);
            };
            let name = wide(name);
            let handle = create(&name, true)?;
            Ok(Self {
                name,
                first: Some(handle),
            })
        }
        pub fn accept(&mut self) -> Result<Connection, ControllerError> {
            let handle = match self.first.take() {
                Some(handle) => handle,
                None => create(&self.name, false)?,
            };
            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            if connected == 0
                && std::io::Error::last_os_error().raw_os_error()
                    != Some(ERROR_PIPE_CONNECTED as i32)
            {
                unsafe { CloseHandle(handle) };
                return Err(ControllerError::Io(std::io::Error::last_os_error()));
            }
            Ok(Connection(handle))
        }
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            if let Some(handle) = self.first.take() {
                unsafe { CloseHandle(handle) };
            }
        }
    }

    pub struct Connection(HANDLE);
    unsafe impl Send for Connection {}
    impl Connection {
        pub fn verify_peer(&self) -> Result<(), ControllerError> {
            if crate::windows_security::peer_is_current_logon(self.0)? {
                Ok(())
            } else {
                Err(ControllerError::InvalidMessage)
            }
        }
    }
    impl Drop for Connection {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    impl Read for Connection {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let mut read = 0;
            let ok = unsafe {
                ReadFile(
                    self.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len().try_into().unwrap_or(u32::MAX),
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(read as usize)
            }
        }
    }
    impl Write for Connection {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            let mut written = 0;
            let ok = unsafe {
                WriteFile(
                    self.0,
                    buffer.as_ptr().cast(),
                    buffer.len().try_into().unwrap_or(u32::MAX),
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(written as usize)
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn create(name: &[u16], first: bool) -> Result<HANDLE, ControllerError> {
        let mut descriptor = crate::windows_security::SecurityDescriptor::for_current_logon()?;
        let mut security = descriptor.attributes();
        let access = PIPE_ACCESS_DUPLEX
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            };
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                access,
                mode,
                255,
                64 * 1024,
                64 * 1024,
                0,
                &mut security,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(ControllerError::Io(std::io::Error::last_os_error()))
        } else {
            Ok(handle)
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use crate::{ControllerError, LocalEndpoint};
    pub struct Listener;
    impl Listener {
        pub fn bind(_: &LocalEndpoint) -> Result<Self, ControllerError> {
            Err(ControllerError::UnsupportedPlatform)
        }
        pub fn accept(&mut self) -> Result<Connection, ControllerError> {
            Err(ControllerError::UnsupportedPlatform)
        }
    }
    pub struct Connection;
    impl Connection {
        pub fn verify_peer(&self) -> Result<(), ControllerError> {
            Err(ControllerError::UnsupportedPlatform)
        }
    }
    impl std::io::Read for Connection {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("unsupported"))
        }
    }
    impl std::io::Write for Connection {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("unsupported"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
