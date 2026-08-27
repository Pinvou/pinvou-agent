use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::path::PathBuf;

use pinvou_controller::{
    ControllerPaths, ControllerSession, HostPlatform, LocalEndpoint, LocalIpcListener,
    SessionStore, WorkspacePreferences, WorkspaceStore,
};
use pinvou_protocol::{
    HelloClient, HelloServer, IpcMessage, RuntimeEventEnvelope, encode_frame, read_frame,
};
use pinvou_runtime_api::ApprovalProfile;

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

#[test]
fn persistent_controller_appends_each_event_before_exposing_it() {
    let root = std::env::temp_dir().join(format!(
        "pinvou-controller-persistent-stream-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    WorkspaceStore::open(&root)
        .unwrap()
        .save(
            &workspace,
            &WorkspacePreferences {
                runtime: Some("codex".into()),
                model_by_runtime: BTreeMap::new(),
                reasoning_level_by_runtime: BTreeMap::new(),
                approval_profile: ApprovalProfile::FullAccess,
                recent_session: None,
            },
        )
        .unwrap();
    let events = scripted_runtime_events();
    let event_count = events.len();
    let (endpoint, server) = spawn_persistent_scripted_node(events);
    let session = ControllerSession::with_local_node_and_storage(
        "controller-instance",
        endpoint,
        "node-instance",
        &root,
        &workspace,
    )
    .unwrap();
    let request = IpcMessage::request(
        serde_json::json!(43),
        "chat.start",
        serde_json::json!({
            "instance_id": "controller-instance",
            "prompt": "persist everything"
        }),
    )
    .unwrap();
    let mut exposed = 0_u64;

    session
        .stream_bound(request, |_| {
            exposed += 1;
            let store = SessionStore::open(&root).unwrap();
            let descriptor = store.list().into_iter().next().unwrap();
            let restored = store.restore(&descriptor.id).unwrap();
            assert_eq!(restored.cursor, exposed + 1);
            assert_eq!(
                restored.normalized_events[0]["payload"],
                serde_json::json!({
                    "role":"user",
                    "content":"persist everything",
                    "item_id":"controller-prompt-1"
                })
            );
            Ok(())
        })
        .unwrap();

    assert_eq!(exposed as usize, event_count);
    let store = SessionStore::open(&root).unwrap();
    let descriptor = store.list().into_iter().next().unwrap();
    let metadata = store.metadata(&descriptor.id).unwrap();
    assert_eq!(metadata.snapshot_cursor as usize, event_count + 1);
    assert_eq!(
        store.restore(&descriptor.id).unwrap().cursor as usize,
        event_count + 1
    );
    server.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
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

#[test]
fn approval_on_a_second_controller_connection_unblocks_the_live_event_stream() {
    let (node_endpoint, node_server) = spawn_approval_gated_node();
    let root = std::env::temp_dir().join(format!(
        "pinvou-controller-two-channel-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let paths = ControllerPaths::from_roots(
        HostPlatform::current().unwrap(),
        root.join("data"),
        root.join("runtime"),
        root.file_name().unwrap().to_str().unwrap(),
    )
    .unwrap();
    paths.prepare_data_root().unwrap();
    let endpoint = paths.endpoint().clone();
    let mut listener = LocalIpcListener::bind(&endpoint).unwrap();
    let session =
        ControllerSession::with_local_node("controller-instance", node_endpoint, "node-instance")
            .unwrap();
    let (approval_seen_tx, approval_seen_rx) = mpsc::channel();
    let (stream_done_tx, stream_done_rx) = mpsc::channel();
    let stream_endpoint = endpoint.clone();
    let stream_client = std::thread::spawn(move || {
        let (mut stream, instance_id) = connect_controller(&stream_endpoint);
        let request = IpcMessage::request(
            serde_json::json!(1),
            "chat.start",
            serde_json::json!({"instance_id":instance_id,"prompt":"needs approval"}),
        )
        .unwrap();
        stream.write_all(&encode_frame(&request).unwrap()).unwrap();
        stream.flush().unwrap();

        let started: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(started.payload()["kind"], "turn.started");
        let approval: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(approval.payload()["kind"], "approval.requested");
        approval_seen_tx.send(()).unwrap();
        let delta: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(delta.payload()["kind"], "text.delta");
        assert_eq!(delta.payload()["payload"]["content"], "continued");
        let ended: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(ended.payload()["kind"], "turn.ended");
        stream_done_tx.send(()).unwrap();
    });
    listener.serve_one(&session).unwrap();

    approval_seen_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("stream connection did not receive approval.requested");

    let (control_done_tx, control_done_rx) = mpsc::channel();
    let control_endpoint = endpoint.clone();
    let control_client = std::thread::spawn(move || {
        let (mut stream, instance_id) = connect_controller(&control_endpoint);
        let request = IpcMessage::request(
            serde_json::json!(2),
            "approval.resolve",
            serde_json::json!({
                "instance_id":instance_id,
                "approval_id":"approval-1",
                "accepted":true
            }),
        )
        .unwrap();
        stream.write_all(&encode_frame(&request).unwrap()).unwrap();
        stream.flush().unwrap();
        let response: IpcMessage = read_frame(&mut stream).unwrap();
        assert_eq!(response.id(), Some(&serde_json::json!(2)));
        assert_eq!(response.payload()["status"], "accepted");
        control_done_tx.send(()).unwrap();
    });
    listener.serve_one(&session).unwrap();

    control_done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("dedicated control request deadlocked");
    stream_done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("event stream did not resume after approval");

    control_client.join().unwrap();
    stream_client.join().unwrap();
    node_server.join().unwrap();
    drop(listener);
    let _ = std::fs::remove_dir_all(root);
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
        let mut stream = accept_node_connection(&server_endpoint, Some(ready_tx));
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

fn spawn_persistent_scripted_node(
    events: Vec<RuntimeEventEnvelope>,
) -> (String, std::thread::JoinHandle<()>) {
    let endpoint = unique_endpoint();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server_endpoint = endpoint.clone();
    let server = std::thread::spawn(move || {
        let mut discovery = accept_node_connection(&server_endpoint, Some(ready_tx));
        serve_node_hello(&mut *discovery);
        let runtime_list: IpcMessage = read_frame(&mut discovery).unwrap();
        assert_eq!(runtime_list.method(), Some("runtime.list"));
        let response = IpcMessage::response(
            runtime_list.id().unwrap().clone(),
            serde_json::json!({"current":"codex","runtimes":[]}),
        )
        .unwrap();
        discovery
            .write_all(&encode_frame(&response).unwrap())
            .unwrap();
        discovery.flush().unwrap();
        let model_list: IpcMessage = read_frame(&mut discovery).unwrap();
        assert_eq!(model_list.method(), Some("model.list"));
        let response = IpcMessage::response(
            model_list.id().unwrap().clone(),
            serde_json::json!({
                "catalog": {
                    "runtime_id":"codex",
                    "models":[{
                        "id":"gpt-5.6",
                        "display_name":"GPT-5.6",
                        "is_default":true,
                        "available":true
                    }],
                    "current_model":"gpt-5.6"
                }
            }),
        )
        .unwrap();
        discovery
            .write_all(&encode_frame(&response).unwrap())
            .unwrap();
        discovery.flush().unwrap();
        drop(discovery);

        let mut chat = accept_node_connection(&server_endpoint, None);
        serve_node_hello(&mut *chat);
        let request: IpcMessage = read_frame(&mut chat).unwrap();
        assert_eq!(request.method(), Some("chat.start"));
        assert_eq!(request.payload()["model_id"], "gpt-5.6");
        assert_eq!(request.payload()["approval_profile"], "full_access");
        for event in events {
            send_runtime_event(&mut *chat, event);
        }
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    (endpoint, server)
}

fn spawn_approval_gated_node() -> (String, std::thread::JoinHandle<()>) {
    let endpoint = unique_endpoint();
    let (ready_tx, ready_rx) = mpsc::channel();
    let server_endpoint = endpoint.clone();
    let server = std::thread::spawn(move || {
        serve_approval_gated_node(&server_endpoint, ready_tx);
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    (endpoint, server)
}

fn serve_node_hello(stream: &mut dyn TestReadWrite) {
    let hello: HelloClient = read_frame(&mut &mut *stream).unwrap();
    assert_eq!(hello.protocol_version(), pinvou_protocol::IPC_VERSION);
    let answer = HelloServer::new("node-instance").unwrap();
    stream.write_all(&encode_frame(&answer).unwrap()).unwrap();
    stream.flush().unwrap();
}

fn send_runtime_event(stream: &mut dyn TestReadWrite, event: RuntimeEventEnvelope) {
    let message = IpcMessage::event("runtime.event", serde_json::to_value(event).unwrap()).unwrap();
    stream.write_all(&encode_frame(&message).unwrap()).unwrap();
    stream.flush().unwrap();
}

fn gated_event(kind: &str, seq: u64, payload: serde_json::Value) -> RuntimeEventEnvelope {
    let (rate_class, stream_id) = match kind {
        "turn.started" | "approval.requested" | "turn.ended" => ("R0", "control"),
        _ => ("R1", "main"),
    };
    RuntimeEventEnvelope::from_value(serde_json::json!({
        "protocol_version":1,
        "schema_version":1,
        "node_id":"node-test",
        "logical_session_id":"session-test",
        "attachment_id":"attachment-test",
        "work_id":null,
        "collaborative_run_id":null,
        "stream_id":stream_id,
        "turn_id":"turn-gated",
        "seq":seq,
        "source_span":null,
        "timestamp":"2026-08-24T00:00:00.000Z",
        "rate_class":rate_class,
        "kind":kind,
        "payload":payload
    }))
    .unwrap()
}

fn connect_controller(endpoint: &LocalEndpoint) -> (Box<dyn TestReadWrite>, String) {
    let mut stream = connect_test_endpoint(endpoint);
    let hello = HelloClient::new(serde_json::json!({"client":"streaming-contract"})).unwrap();
    stream.write_all(&encode_frame(&hello).unwrap()).unwrap();
    stream.flush().unwrap();
    let answer: HelloServer = read_frame(&mut stream).unwrap();
    let instance_id = answer.instance_id().to_owned();
    (stream, instance_id)
}

#[cfg(target_os = "linux")]
fn serve_approval_gated_node(endpoint: &str, ready: mpsc::Sender<()>) {
    use std::os::unix::net::UnixListener;
    let path = PathBuf::from(endpoint);
    let listener = UnixListener::bind(&path).unwrap();
    ready.send(()).unwrap();
    let mut chat: Box<dyn TestReadWrite> = Box::new(listener.accept().unwrap().0);
    begin_approval_chat(&mut *chat);
    let mut control: Box<dyn TestReadWrite> = Box::new(listener.accept().unwrap().0);
    finish_approval_control(&mut *control, &mut *chat);
    drop(listener);
    let _ = std::fs::remove_file(path);
}

#[cfg(windows)]
fn serve_approval_gated_node(endpoint: &str, ready: mpsc::Sender<()>) {
    let mut chat = accept_node_connection(endpoint, Some(ready));
    begin_approval_chat(&mut *chat);
    let mut control = accept_node_connection(endpoint, None);
    finish_approval_control(&mut *control, &mut *chat);
}

fn begin_approval_chat(chat: &mut dyn TestReadWrite) {
    serve_node_hello(chat);
    let request: IpcMessage = read_frame(&mut &mut *chat).unwrap();
    assert_eq!(request.method(), Some("chat.start"));
    send_runtime_event(
        chat,
        gated_event(
            "turn.started",
            1,
            serde_json::json!({"user_input_ref":"prompt"}),
        ),
    );
    send_runtime_event(
        chat,
        gated_event(
            "approval.requested",
            2,
            serde_json::json!({
                "approval_id":"approval-1",
                "tool":"shell",
                "summary":"continue",
                "options":["approved","denied"],
                "timeout_ms":30000
            }),
        ),
    );
}

fn finish_approval_control(control: &mut dyn TestReadWrite, chat: &mut dyn TestReadWrite) {
    serve_node_hello(control);
    let request: IpcMessage = read_frame(&mut &mut *control).unwrap();
    assert_eq!(request.method(), Some("approval.resolve"));
    assert_eq!(request.payload()["approval_id"], "approval-1");
    assert_eq!(request.payload()["accepted"], true);
    let response = IpcMessage::response(
        request.id().unwrap().clone(),
        serde_json::json!({"status":"accepted","approval_id":"approval-1"}),
    )
    .unwrap();
    control
        .write_all(&encode_frame(&response).unwrap())
        .unwrap();
    control.flush().unwrap();
    send_runtime_event(
        chat,
        gated_event(
            "text.delta",
            3,
            serde_json::json!({"role":"assistant","content":"continued","merged_count":1}),
        ),
    );
    send_runtime_event(
        chat,
        gated_event(
            "turn.ended",
            4,
            serde_json::json!({"end_reason":"completed","error":null}),
        ),
    );
}

trait TestReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> TestReadWrite for T {}

#[cfg(windows)]
fn connect_test_endpoint(endpoint: &LocalEndpoint) -> Box<dyn TestReadWrite> {
    let LocalEndpoint::WindowsPipe(name) = endpoint else {
        unreachable!()
    };
    Box::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
            .unwrap(),
    )
}

#[cfg(target_os = "linux")]
fn connect_test_endpoint(endpoint: &LocalEndpoint) -> Box<dyn TestReadWrite> {
    let LocalEndpoint::UnixSocket(path) = endpoint else {
        unreachable!()
    };
    Box::new(std::os::unix::net::UnixStream::connect(path).unwrap())
}

#[cfg(target_os = "linux")]
fn accept_node_connection(
    endpoint: &str,
    ready: Option<mpsc::Sender<()>>,
) -> Box<dyn TestReadWrite> {
    use std::os::unix::net::UnixListener;
    let path = PathBuf::from(endpoint);
    let listener = UnixListener::bind(&path).unwrap();
    if let Some(ready) = ready {
        ready.send(()).unwrap();
    }
    let stream = listener.accept().unwrap().0;
    drop(listener);
    let _ = std::fs::remove_file(path);
    Box::new(stream)
}

#[cfg(windows)]
fn accept_node_connection(
    endpoint: &str,
    ready: Option<mpsc::Sender<()>>,
) -> Box<dyn TestReadWrite> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile},
        System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        },
    };

    struct Pipe(HANDLE);
    unsafe impl Send for Pipe {}
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
            2,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE);
    if let Some(ready) = ready {
        ready.send(()).unwrap();
    }
    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
    if connected == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_PIPE_CONNECTED as i32)
        );
    }
    Box::new(Pipe(handle))
}
