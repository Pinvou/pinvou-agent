use std::{
    io::{Read, Write},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::path::PathBuf;

use pinvou_controller::ControllerSession;
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame, read_frame,
};

#[test]
fn controller_stream_bound_forwards_the_complete_node_event_stream_in_order() {
    let events = scripted_runtime_events();
    let expected = events
        .iter()
        .map(|event| serde_json::to_value(event).unwrap())
        .collect::<Vec<_>>();
    let (endpoint, first_observed, server) = spawn_scripted_node(events);
    let session =
        ControllerSession::with_local_node("controller-instance", endpoint, "node-instance")
            .unwrap();
    let request = IpcMessage::request(
        serde_json::json!(41),
        "chat.start",
        serde_json::json!({
            "instance_id": "controller-instance",
            "prompt": "stream everything"
        }),
    )
    .unwrap();
    let mut received = Vec::new();
    let mut first_observed = Some(first_observed);

    session
        .stream_bound(request, |message| {
            assert_eq!(message.topic(), Some("runtime.event"));
            received.push(message.payload().clone());
            if let Some(first_observed) = first_observed.take() {
                first_observed.send(()).unwrap();
            }
            Ok(())
        })
        .unwrap();

    assert_eq!(received, expected);
    assert_eq!(received.last().unwrap()["kind"], "turn.ended");
    server.join().unwrap();
}

#[cfg(debug_assertions)]
#[test]
fn scripted_controller_stream_requires_a_real_terminal_event() {
    let session = ControllerSession::with_scripted_chat(
        "controller-instance",
        vec![scripted_runtime_events().remove(0)],
    )
    .unwrap();
    let request = IpcMessage::request(
        serde_json::json!(42),
        "chat.start",
        serde_json::json!({
            "instance_id": "controller-instance",
            "prompt": "do not invent a terminal"
        }),
    )
    .unwrap();

    assert!(session.stream_bound(request, |_| Ok(())).is_err());
}

fn scripted_runtime_events() -> Vec<RuntimeEventEnvelope> {
    [
        (
            "turn.started",
            "R0",
            "control",
            serde_json::json!({"user_input_ref":"prompt"}),
        ),
        (
            "text.delta",
            "R1",
            "main",
            serde_json::json!({"role":"assistant","content":"hello","merged_count":1}),
        ),
        (
            "approval.requested",
            "R0",
            "control",
            serde_json::json!({"approval_id":"approval-1","tool":"shell","summary":"run","options":["approved","denied"],"timeout_ms":30000}),
        ),
        (
            "tool.call.started",
            "R1",
            "main",
            serde_json::json!({"tool_id":"tool-1","name":"shell","args_json":{}}),
        ),
        (
            "tool.call.completed",
            "R1",
            "main",
            serde_json::json!({"tool_id":"tool-1","result":{},"is_error":false,"exit_code":0}),
        ),
        (
            "turn.ended",
            "R0",
            "control",
            serde_json::json!({"end_reason":"completed","error":null}),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, rate_class, stream_id, payload))| {
        RuntimeEventEnvelope::from_value(serde_json::json!({
            "protocol_version": 1,
            "schema_version": 1,
            "node_id": "node-test",
            "logical_session_id": "session-test",
            "attachment_id": "attachment-test",
            "work_id": null,
            "collaborative_run_id": null,
            "stream_id": stream_id,
            "turn_id": "turn-test",
            "seq": index as u64 + 1,
            "source_span": null,
            "timestamp": "2026-08-24T00:00:00.000Z",
            "rate_class": rate_class,
            "kind": kind,
            "payload": payload
        }))
        .unwrap()
    })
    .collect()
}

fn unique_endpoint() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    #[cfg(windows)]
    return format!(
        r"\\.\pipe\pinvou-controller-stream-test-{}-{nonce}",
        std::process::id()
    );
    #[cfg(target_os = "linux")]
    return std::env::temp_dir()
        .join(format!(
            "pinvou-controller-stream-test-{}-{nonce}.sock",
            std::process::id()
        ))
        .display()
        .to_string();
    #[cfg(not(any(windows, target_os = "linux")))]
    panic!("streaming contract requires a supported local IPC platform");
}

fn spawn_scripted_node(
    events: Vec<RuntimeEventEnvelope>,
) -> (String, mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let endpoint = unique_endpoint();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (first_observed_tx, first_observed_rx) = mpsc::channel();
    let server_endpoint = endpoint.clone();
    let server = std::thread::spawn(move || {
        let mut stream = accept_node_connection(&server_endpoint, ready_tx);
        let hello: HelloClient = read_frame(&mut stream).unwrap();
        assert_eq!(hello.protocol_version(), pinvou_protocol::IPC_VERSION);
        let answer = HelloServer::new("node-instance").unwrap();
        stream.write_all(&encode_frame(&answer).unwrap()).unwrap();
        stream.flush().unwrap();
        let request: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(request.method(), Some("chat.start"));
        assert_eq!(request.payload()["instance_id"], "node-instance");
        assert_eq!(request.payload()["prompt"], "stream everything");
        for (index, event) in events.into_iter().enumerate() {
            let message =
                IpcMessage::event("runtime.event", serde_json::to_value(event).unwrap()).unwrap();
            stream.write_all(&encode_frame(&message).unwrap()).unwrap();
            stream.flush().unwrap();
            if index == 0 {
                first_observed_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("Controller must emit the first event before reading the rest");
            }
        }
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    (endpoint, first_observed_tx, server)
}

trait TestReadWrite: Read + Write {}
impl<T: Read + Write> TestReadWrite for T {}

#[cfg(target_os = "linux")]
fn accept_node_connection(endpoint: &str, ready: mpsc::Sender<()>) -> Box<dyn TestReadWrite> {
    use std::os::unix::net::UnixListener;
    let path = PathBuf::from(endpoint);
    let listener = UnixListener::bind(&path).unwrap();
    ready.send(()).unwrap();
    let stream = listener.accept().unwrap().0;
    drop(listener);
    let _ = std::fs::remove_file(path);
    Box::new(stream)
}

#[cfg(windows)]
fn accept_node_connection(endpoint: &str, ready: mpsc::Sender<()>) -> Box<dyn TestReadWrite> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile},
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        },
    };

    struct Pipe(HANDLE);
    impl Drop for Pipe {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    impl Read for Pipe {
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
    impl Write for Pipe {
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

    let wide = OsStr::new(endpoint)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    ready.send(()).unwrap();
    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if connected == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_PIPE_CONNECTED as i32)
        );
    }
    Box::new(Pipe(handle))
}
