#![cfg(feature = "distributed")]

use std::{
    io::Write,
    process::Command,
    sync::Mutex,
    time::{Duration, Instant},
};

use pinvou_cli::distributed::{
    CHAT_TEXT_FRAME, ControllerWire, DistributedCommand, ProjectionAction, TerminalProjection,
    map_error_causes,
};
use pinvou_cli::{CliCommand, ExitCode, execute, parse_args};
use pinvou_controller::{ControllerPaths, ControllerSession, LocalIpcListener};
use pinvou_protocol::{
    ExitCause, IpcMessage, RuntimeEventEnvelope, StableExitCode, decode_frame, encode_frame,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct FakeDuplex {
    inbound: std::io::Cursor<Vec<u8>>,
    outbound: Vec<u8>,
}

impl FakeDuplex {
    fn with_responses(responses: impl IntoIterator<Item = IpcMessage>) -> Self {
        let inbound = responses
            .into_iter()
            .flat_map(|message| encode_frame(&message).unwrap())
            .collect();
        Self {
            inbound: std::io::Cursor::new(inbound),
            outbound: Vec::new(),
        }
    }

    fn requests(&self) -> Vec<IpcMessage> {
        let mut bytes = self.outbound.as_slice();
        let mut requests = Vec::new();
        while !bytes.is_empty() {
            let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            requests.push(decode_frame(&bytes[..4 + len]).unwrap());
            bytes = &bytes[4 + len..];
        }
        requests
    }
}

impl std::io::Read for FakeDuplex {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.inbound, buffer)
    }
}

impl std::io::Write for FakeDuplex {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.outbound.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn event(kind: &str, rate: &str, stream: &str, payload: serde_json::Value) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope::from_value(serde_json::json!({
        "protocol_version": 1,
        "schema_version": 1,
        "node_id": "local-node",
        "logical_session_id": "session-1",
        "attachment_id": "attachment-1",
        "work_id": null,
        "collaborative_run_id": null,
        "stream_id": stream,
        "turn_id": "turn-1",
        "seq": 1,
        "source_span": null,
        "timestamp": "2026-08-21T00:00:00Z",
        "rate_class": rate,
        "kind": kind,
        "payload": payload
    }))
    .unwrap()
}

#[test]
fn no_arguments_prints_stage_one_help_without_starting_a_daemon() {
    let parsed = parse_args(["pinvou"]).expect("no arguments are valid in stage one");
    assert_eq!(parsed.command(), &CliCommand::Help);
    let outcome = execute(parsed).unwrap();
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("pinvou chat"));
    assert!(outcome.stdout.contains("pinvou runtime detect"));
}

#[test]
fn distributed_commands_are_explicit_and_unknown_shapes_are_usage_errors() {
    assert_eq!(
        parse_args(["pinvou", "chat"]).unwrap().command(),
        &CliCommand::Distributed(DistributedCommand::Chat)
    );
    assert_eq!(
        parse_args(["pinvou", "runtime", "detect"])
            .unwrap()
            .command(),
        &CliCommand::Distributed(DistributedCommand::RuntimeDetect)
    );
    for args in [
        vec!["pinvou", "runtime"],
        vec!["pinvou", "runtime", "unknown"],
        vec!["pinvou", "chat", "unexpected"],
    ] {
        assert_eq!(parse_args(args).unwrap_err().exit_code(), ExitCode::Usage);
    }
}

#[test]
fn interactive_chat_rejects_non_tty_before_controller_startup() {
    let error = pinvou_cli::distributed::require_interactive_terminal(false, true).unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 2);
    assert!(error.to_string().contains("interactive terminal"));
    assert!(pinvou_cli::distributed::require_interactive_terminal(true, true).is_ok());
}

#[test]
fn text_projection_uses_one_fifty_millisecond_frame_without_a_second_window() {
    let start = Instant::now();
    let mut projection = TerminalProjection::new(start);
    let first = event(
        "text.delta",
        "R1",
        "main",
        serde_json::json!({"role":"assistant","content":"pin"}),
    );
    let second = event(
        "text.delta",
        "R1",
        "main",
        serde_json::json!({"role":"assistant","content":"vou"}),
    );
    assert_eq!(
        projection.push(&first, start).unwrap(),
        ProjectionAction::WriteText("pin".into())
    );
    assert_eq!(
        projection
            .push(&second, start + CHAT_TEXT_FRAME - Duration::from_millis(1))
            .unwrap(),
        ProjectionAction::None
    );
    assert_eq!(
        projection.flush_due(start + CHAT_TEXT_FRAME),
        Some("vou".into())
    );
    assert!(projection.flush_due(start + CHAT_TEXT_FRAME * 2).is_none());
}

#[test]
fn approval_and_turn_end_events_drive_terminal_actions() {
    let now = Instant::now();
    let mut projection = TerminalProjection::new(now);
    let approval = event(
        "approval.requested",
        "R0",
        "control",
        serde_json::json!({
            "approval_id":"approval-1",
            "tool":"command",
            "summary":"run tests",
            "options":["allow","deny"]
        }),
    );
    assert_eq!(
        projection.push(&approval, now).unwrap(),
        ProjectionAction::AskApproval {
            approval_id: "approval-1".into(),
            prompt: "run tests [y/N] ".into()
        }
    );
    assert_eq!(projection.parse_approval("y\n"), Some(true));
    assert_eq!(projection.parse_approval("N\n"), Some(false));
    assert_eq!(projection.parse_approval("later\n"), None);

    let ended = event(
        "turn.ended",
        "R0",
        "control",
        serde_json::json!({"end_reason":"interrupted"}),
    );
    assert_eq!(
        projection.push(&ended, now).unwrap(),
        ProjectionAction::TurnEnded(StableExitCode::Cancelled)
    );
}

#[test]
fn causal_first_error_mapping_covers_all_stable_exit_codes() {
    let cases = [
        (ExitCause::Internal, 1),
        (ExitCause::Usage, 2),
        (ExitCause::ControllerUnavailable, 3),
        (ExitCause::BlockedAuth, 4),
        (ExitCause::RuntimeFailed, 5),
        (ExitCause::Cancelled, 6),
        (ExitCause::ResourceExhausted, 7),
        (ExitCause::DataCorruption, 8),
        (ExitCause::Unmapped, 1),
    ];
    for (cause, expected) in cases {
        assert_eq!(map_error_causes([cause]).as_i32(), expected);
    }
    assert_eq!(
        map_error_causes([ExitCause::ControllerUnavailable, ExitCause::BlockedAuth]),
        StableExitCode::ControllerUnavailable
    );
}

#[test]
fn cli_uses_only_formal_controller_methods_and_binds_the_instance_challenge() {
    let responses = (1..=5).map(|id| {
        IpcMessage::response(serde_json::json!(id), serde_json::json!({"ok":true})).unwrap()
    });
    let mut client = ControllerWire::from_authenticated(
        FakeDuplex::with_responses(responses),
        "controller-instance",
    );
    client.chat_start("hello").unwrap();
    client.resolve_approval("approval-1", true).unwrap();
    client.resolve_input("input-1", "answer").unwrap();
    client.interrupt_turn("turn-1").unwrap();
    client.runtime_detect().unwrap();

    let requests = client.into_inner().requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method().unwrap())
            .collect::<Vec<_>>(),
        [
            "chat.start",
            "approval.resolve",
            "input.resolve",
            "turn.interrupt",
            "runtime.detect"
        ]
    );
    assert!(requests.iter().all(|request| {
        request.payload()["instance_id"] == serde_json::json!("controller-instance")
    }));
    assert_eq!(requests[0].payload()["prompt"], "hello");
    assert_eq!(requests[1].payload()["accepted"], true);
    assert_eq!(requests[2].payload()["value"], "answer");
    assert_eq!(requests[3].payload()["turn_id"], "turn-1");
}

#[test]
fn runtime_detect_binary_uses_the_real_controller_ipc_wire() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let unique = format!(
        "pinvou-cli-runtime-detect-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(&unique);
    std::fs::create_dir_all(&root).unwrap();
    let previous_local = std::env::var_os("LOCALAPPDATA");
    let previous_home = std::env::var_os("HOME");
    let previous_xdg_data = std::env::var_os("XDG_DATA_HOME");
    let previous_xdg_runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let previous_scope = std::env::var_os("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &root);
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_RUNTIME_DIR", root.join("runtime"));
        std::env::set_var("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique);
    }
    std::fs::create_dir_all(root.join("runtime")).unwrap();
    let paths = ControllerPaths::discover().unwrap();
    #[cfg(windows)]
    let expected_paths = ControllerPaths::from_roots(
        pinvou_controller::HostPlatform::current().unwrap(),
        root.join("pinvou"),
        root.join("pinvou"),
        &unique,
    )
    .unwrap();
    #[cfg(target_os = "linux")]
    let expected_paths = ControllerPaths::from_roots(
        pinvou_controller::HostPlatform::current().unwrap(),
        root.join("data").join("pinvou"),
        root.join("runtime"),
        "unused",
    )
    .unwrap();
    assert_eq!(paths.endpoint(), expected_paths.endpoint());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
        ready_tx.send(()).unwrap();
        listener
            .serve_one(&ControllerSession::new("binary-controller").unwrap())
            .unwrap();
    });
    ready_rx.recv().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["--output", "json", "runtime", "detect"])
        .env("LOCALAPPDATA", &root)
        .env("HOME", &root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique)
        .output()
        .unwrap();
    server.join().unwrap();
    restore_env("LOCALAPPDATA", previous_local);
    restore_env("HOME", previous_home);
    restore_env("XDG_DATA_HOME", previous_xdg_data);
    restore_env("XDG_RUNTIME_DIR", previous_xdg_runtime);
    restore_env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", previous_scope);
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "unavailable");
    assert_eq!(value["runtime"], "local-node");
    assert_eq!(value["protocol_version"], pinvou_protocol::IPC_VERSION);
}

#[test]
fn chat_binary_projects_scripted_controller_events_over_real_ipc() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let unique = format!(
        "pinvou-cli-chat-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(&unique);
    std::fs::create_dir_all(root.join("runtime")).unwrap();
    let previous_local = std::env::var_os("LOCALAPPDATA");
    let previous_home = std::env::var_os("HOME");
    let previous_xdg_data = std::env::var_os("XDG_DATA_HOME");
    let previous_xdg_runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let previous_scope = std::env::var_os("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST");
    unsafe {
        std::env::set_var("LOCALAPPDATA", &root);
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_RUNTIME_DIR", root.join("runtime"));
        std::env::set_var("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique);
    }
    let paths = ControllerPaths::discover().unwrap();
    let text = event(
        "text.delta",
        "R1",
        "main",
        serde_json::json!({"role":"assistant","content":"scripted answer","merged_count":1}),
    );
    let ended = event(
        "turn.ended",
        "R0",
        "control",
        serde_json::json!({"end_reason":"completed","error":null}),
    );
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut listener = LocalIpcListener::bind(paths.endpoint()).unwrap();
        ready_tx.send(()).unwrap();
        listener
            .serve_one(
                &ControllerSession::with_scripted_chat("binary-controller", vec![text, ended])
                    .unwrap(),
            )
            .unwrap();
    });
    ready_rx.recv().unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .args(["chat"])
        .env("LOCALAPPDATA", &root)
        .env("HOME", &root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", &unique)
        .env("PINVOU_ASSUME_INTERACTIVE_TTY_FOR_TEST", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"hello scripted controller\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    restore_env("LOCALAPPDATA", previous_local);
    restore_env("HOME", previous_home);
    restore_env("XDG_DATA_HOME", previous_xdg_data);
    restore_env("XDG_RUNTIME_DIR", previous_xdg_runtime);
    restore_env("PINVOU_CONTROLLER_SESSION_SCOPE_FOR_TEST", previous_scope);
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "You: scripted answer\n"
    );
}

fn restore_env(name: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
