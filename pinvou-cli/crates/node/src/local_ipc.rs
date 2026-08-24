use std::io::Write;

use pinvou_protocol::{HelloClient, IpcMessage, encode_frame, read_frame};

use crate::{NodeError, NodeSession};

#[derive(Clone, Copy, Debug)]
pub struct NodeTransportPolicy;
impl NodeTransportPolicy {
    pub const fn stage_one() -> Self {
        Self
    }
    pub const fn local_ipc(self) -> bool {
        true
    }
    pub const fn tcp(self) -> bool {
        false
    }
    pub const fn discovery(self) -> bool {
        false
    }
    pub const fn has_port(self) -> bool {
        false
    }
}

pub struct NodeLocalListener {
    inner: platform::Listener,
}
impl NodeLocalListener {
    pub fn bind(endpoint: &str) -> Result<Self, NodeError> {
        Ok(Self {
            inner: platform::Listener::bind(endpoint)?,
        })
    }
    pub fn serve_one(&mut self, session: &NodeSession) -> Result<(), NodeError> {
        let mut connection = self.inner.accept()?;
        let session = session.clone();
        std::thread::Builder::new()
            .name("pinvou-node-client".into())
            .spawn(move || {
                let _ = serve(&mut connection, &session);
            })?;
        Ok(())
    }
}

fn serve(stream: &mut platform::Connection, session: &NodeSession) -> Result<(), NodeError> {
    let hello: HelloClient = match read_frame(stream) {
        Ok(hello) => hello,
        Err(_) => {
            let error = IpcMessage::error(
                None,
                serde_json::json!({"code":3,"error":"protocol_version_mismatch"}),
            )
            .map_err(|_| NodeError::InvalidMessage)?;
            stream.write_all(&encode_frame(&error).map_err(|_| NodeError::InvalidMessage)?)?;
            stream.flush()?;
            return Err(NodeError::ProtocolMismatch);
        }
    };
    stream.verify_peer()?;
    let answer = session.accept_hello(hello)?;
    stream.write_all(&encode_frame(&answer).map_err(|_| NodeError::InvalidMessage)?)?;
    loop {
        let request: IpcMessage = read_frame(stream).map_err(|_| NodeError::InvalidMessage)?;
        if request.method() == Some("chat.start") {
            session.stream_bound(request, |event| {
                stream.write_all(&encode_frame(&event).map_err(|_| NodeError::InvalidMessage)?)?;
                stream.flush()?;
                Ok(())
            })?;
            continue;
        }
        let response = session.handle(request)?;
        stream.write_all(&encode_frame(&response).map_err(|_| NodeError::InvalidMessage)?)?;
        stream.flush()?;
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use crate::NodeError;
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::PermissionsExt,
            net::{UnixListener, UnixStream},
        },
        path::PathBuf,
    };
    pub struct Listener {
        listener: UnixListener,
        path: PathBuf,
    }
    impl Listener {
        pub fn bind(endpoint: &str) -> Result<Self, NodeError> {
            let path = PathBuf::from(endpoint);
            let parent = path.parent().ok_or(NodeError::InvalidMessage)?;
            std::fs::create_dir_all(parent)?;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            if path.exists() {
                if UnixStream::connect(&path).is_ok() {
                    return Err(NodeError::InvalidMessage);
                }
                std::fs::remove_file(&path)?;
            }
            let listener = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { listener, path })
        }
        pub fn accept(&mut self) -> Result<Connection, NodeError> {
            Ok(Connection(self.listener.accept()?.0))
        }
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    pub struct Connection(UnixStream);
    impl Connection {
        pub fn verify_peer(&self) -> Result<(), NodeError> {
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
                return Err(NodeError::Io(std::io::Error::last_os_error()));
            }
            if credentials.uid != unsafe { libc::geteuid() } {
                return Err(NodeError::InvalidMessage);
            }
            Ok(())
        }
    }
    impl Read for Connection {
        fn read(&mut self, value: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(value)
        }
    }
    impl Write for Connection {
        fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
            self.0.write(value)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;
    use crate::{NodeRuntimeEventStream, NodeRuntimeHost};
    use pinvou_protocol::{
        HelloServer, IpcMessageKind, RuntimeEventEnvelope, encode_frame, read_frame,
    };
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[derive(Debug)]
    struct ScriptedRuntime(Mutex<Option<Vec<RuntimeEventEnvelope>>>);

    impl NodeRuntimeHost for ScriptedRuntime {
        fn start_turn(
            &self,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<NodeRuntimeEventStream, NodeError> {
            Ok(Box::new(
                self.0.lock().unwrap().take().unwrap().into_iter().map(Ok),
            ))
        }
    }

    trait TestStream: std::io::Read + std::io::Write {}
    impl<T: std::io::Read + std::io::Write> TestStream for T {}

    #[cfg(windows)]
    fn open(endpoint: &str) -> std::io::Result<Box<dyn TestStream>> {
        Ok(Box::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(endpoint)?,
        ))
    }

    #[cfg(target_os = "linux")]
    fn open(endpoint: &str) -> std::io::Result<Box<dyn TestStream>> {
        Ok(Box::new(std::os::unix::net::UnixStream::connect(endpoint)?))
    }

    fn event(kind: &str, seq: u64, payload: serde_json::Value) -> RuntimeEventEnvelope {
        let control = matches!(
            kind,
            "turn.started" | "turn.ended" | "approval.requested" | "input.requested"
        );
        RuntimeEventEnvelope::from_value(serde_json::json!({
            "protocol_version":pinvou_protocol::IPC_VERSION,"schema_version":1,
            "node_id":"node","logical_session_id":"session","attachment_id":"attachment",
            "work_id":null,"collaborative_run_id":null,
            "stream_id":if control { "control" } else { "main" },"turn_id":"turn",
            "seq":seq,"source_span":null,"timestamp":"2026-08-24T00:00:00.000Z",
            "rate_class":if control { "R0" } else { "R1" },"kind":kind,"payload":payload
        }))
        .unwrap()
    }

    #[test]
    fn chat_start_writes_every_runtime_event_on_the_same_connection() {
        let events = vec![
            event(
                "turn.started",
                1,
                serde_json::json!({"user_input_ref":"prompt"}),
            ),
            event(
                "text.delta",
                2,
                serde_json::json!({"role":"assistant","content":"one","merged_count":1}),
            ),
            event(
                "text.delta",
                3,
                serde_json::json!({"role":"assistant","content":"two","merged_count":1}),
            ),
            event(
                "approval.requested",
                4,
                serde_json::json!({"approval_id":"approval-1","tool":"shell","summary":"run","options":["approved","denied"],"timeout_ms":30000}),
            ),
            event(
                "input.requested",
                5,
                serde_json::json!({"input_id":"input-1","prompt":"choose","schema":{"type":"string"}}),
            ),
            event(
                "tool.call.started",
                6,
                serde_json::json!({"tool_id":"call-1","name":"read","args_json":{}}),
            ),
            event(
                "tool.call.completed",
                7,
                serde_json::json!({"tool_id":"call-1","result":{},"is_error":false,"exit_code":0}),
            ),
            event(
                "turn.ended",
                8,
                serde_json::json!({"end_reason":"completed","error":null}),
            ),
        ];
        let unique = format!(
            "ipc-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        #[cfg(windows)]
        let endpoint = format!(r"\\.\pipe\pinvou-node-{unique}");
        #[cfg(target_os = "linux")]
        let endpoint = std::env::temp_dir()
            .join(format!("pinvou-node-{unique}/node.sock"))
            .display()
            .to_string();
        let mut listener = NodeLocalListener::bind(&endpoint).unwrap();
        let session = NodeSession::with_runtime(
            "instance",
            Arc::new(ScriptedRuntime(Mutex::new(Some(events)))),
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let client = std::thread::spawn(move || {
            let mut stream = (0..80)
                .find_map(|_| {
                    let result = open(&endpoint).ok();
                    if result.is_none() {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    result
                })
                .unwrap();
            stream
                .write_all(
                    &encode_frame(&HelloClient::new(serde_json::json!({})).unwrap()).unwrap(),
                )
                .unwrap();
            let _: HelloServer = read_frame(&mut stream).unwrap();
            let request = IpcMessage::request(
                serde_json::json!(1),
                "chat.start",
                serde_json::json!({"instance_id":"instance","prompt":"go"}),
            )
            .unwrap();
            stream.write_all(&encode_frame(&request).unwrap()).unwrap();
            for _ in 0..8 {
                tx.send(read_frame::<_, IpcMessage>(&mut stream)).unwrap();
            }
        });
        listener.serve_one(&session).unwrap();
        let messages = (0..8)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap())
            .collect::<Vec<_>>();
        client.join().unwrap();
        assert!(messages.iter().all(|message| {
            message.kind() == IpcMessageKind::Evt && message.topic() == Some("runtime.event")
        }));
        let envelopes = messages
            .into_iter()
            .map(|message| RuntimeEventEnvelope::from_value(message.payload().clone()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            envelopes
                .iter()
                .map(RuntimeEventEnvelope::kind)
                .collect::<Vec<_>>(),
            [
                "turn.started",
                "text.delta",
                "text.delta",
                "approval.requested",
                "input.requested",
                "tool.call.started",
                "tool.call.completed",
                "turn.ended"
            ]
        );
        assert_eq!(
            envelopes
                .iter()
                .map(RuntimeEventEnvelope::seq)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
    }
}

#[cfg(windows)]
mod platform {
    use crate::NodeError;
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
        pub fn bind(endpoint: &str) -> Result<Self, NodeError> {
            let name = wide(endpoint);
            Ok(Self {
                first: Some(create(&name, true)?),
                name,
            })
        }
        pub fn accept(&mut self) -> Result<Connection, NodeError> {
            let handle = match self.first.take() {
                Some(value) => value,
                None => create(&self.name, false)?,
            };
            let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
            if connected == 0
                && std::io::Error::last_os_error().raw_os_error()
                    != Some(ERROR_PIPE_CONNECTED as i32)
            {
                unsafe { CloseHandle(handle) };
                return Err(NodeError::Io(std::io::Error::last_os_error()));
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
        pub fn verify_peer(&self) -> Result<(), NodeError> {
            if crate::windows_security::peer_is_current_logon(self.0)? {
                Ok(())
            } else {
                Err(NodeError::InvalidMessage)
            }
        }
    }
    impl Drop for Connection {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    impl Read for Connection {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0;
            let ok = unsafe {
                ReadFile(
                    self.0,
                    b.as_mut_ptr(),
                    b.len().try_into().unwrap_or(u32::MAX),
                    &mut n,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }
    }
    impl Write for Connection {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let mut n = 0;
            let ok = unsafe {
                WriteFile(
                    self.0,
                    b.as_ptr(),
                    b.len().try_into().unwrap_or(u32::MAX),
                    &mut n,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn create(name: &[u16], first: bool) -> Result<HANDLE, NodeError> {
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
                65536,
                65536,
                0,
                &mut security,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(NodeError::Io(std::io::Error::last_os_error()))
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
    use crate::NodeError;
    pub struct Listener;
    impl Listener {
        pub fn bind(_: &str) -> Result<Self, NodeError> {
            Err(NodeError::UnsupportedPlatform)
        }
        pub fn accept(&mut self) -> Result<Connection, NodeError> {
            Err(NodeError::UnsupportedPlatform)
        }
    }
    pub struct Connection;
    impl Connection {
        pub fn verify_peer(&self) -> Result<(), NodeError> {
            Err(NodeError::UnsupportedPlatform)
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
