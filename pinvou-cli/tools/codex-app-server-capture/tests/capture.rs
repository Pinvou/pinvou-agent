use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use codex_app_server_capture::capture::{
    CaptureConfig, CommandSpec, JsonlRecorder, ProxyConfig, parse_client_jsonl, read_lines,
    run_capture, run_proxy,
};
use codex_app_server_capture::protocol::{CaptureChannel, CaptureRecord};

#[test]
fn recorder_writes_one_json_object_per_line_with_supplied_monotonic_time() {
    let mut bytes = Vec::new();
    let timestamps = [10_u64, 25, 40];
    let mut next = timestamps.into_iter();
    {
        let mut recorder = JsonlRecorder::new(&mut bytes, || next.next().unwrap());
        recorder
            .record(CaptureChannel::ClientToServer, r#"{"id":1}"#)
            .unwrap();
        recorder
            .record(CaptureChannel::ServerToClient, r#"{"id":1,"result":{}}"#)
            .unwrap();
        recorder
            .record(CaptureChannel::Stderr, "diagnostic")
            .unwrap();
    }

    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text.lines().count(), 3);
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<CaptureRecord>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|item| item.monotonic_ns)
            .collect::<Vec<_>>(),
        timestamps
    );
    assert_eq!(records[2].channel, CaptureChannel::Stderr);
}

#[test]
fn recorder_rejects_embedded_line_breaks() {
    let mut bytes = Vec::new();
    let mut recorder = JsonlRecorder::new(&mut bytes, || 1);

    let error = recorder
        .record(CaptureChannel::Stderr, "first\nsecond")
        .unwrap_err();

    assert!(error.to_string().contains("single line"));
    assert!(bytes.is_empty());
}

#[test]
fn command_spec_uses_codex_app_server_stdio_and_supports_executable_override() {
    let default = CommandSpec::codex(None);
    assert_eq!(default.program, OsString::from("codex"));
    assert_eq!(default.args, ["app-server", "--stdio"].map(OsString::from));

    let overridden = CommandSpec::codex(Some(OsString::from("fake-codex")));
    assert_eq!(overridden.program, OsString::from("fake-codex"));
    assert_eq!(overridden.args, default.args);
}

#[test]
fn client_input_accepts_only_json_object_frames() {
    let frames = parse_client_jsonl("{\"id\":1}\n\n{\"method\":\"initialized\"}\r\n").unwrap();
    assert_eq!(frames, [r#"{"id":1}"#, r#"{"method":"initialized"}"#]);

    assert!(parse_client_jsonl("not-json\n").is_err());
    assert!(parse_client_jsonl("[1,2,3]\n").is_err());
}

#[test]
fn stream_reader_strips_terminators_and_preserves_each_channel_line() {
    let input = std::io::Cursor::new(b"first\r\nsecond\n".to_vec());
    let mut lines = Vec::new();

    read_lines(input, |line| {
        lines.push(line.to_owned());
        Ok(())
    })
    .unwrap();

    assert_eq!(lines, ["first", "second"]);
}

#[test]
fn driver_captures_fake_process_stdin_stdout_and_stderr_separately() {
    let stem = format!("codex-capture-test-{}", std::process::id());
    let input = std::env::temp_dir().join(format!("{stem}.input.jsonl"));
    let output = std::env::temp_dir().join(format!("{stem}.capture.jsonl"));
    std::fs::write(&input, "{\"id\":1,\"method\":\"initialize\"}\n").unwrap();

    #[cfg(windows)]
    let command = CommandSpec::new(
        OsString::from("powershell.exe"),
        [
            "-NoProfile",
            "-Command",
            "$null=[Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); [Console]::Error.WriteLine('separate diagnostic')",
        ]
        .map(OsString::from),
    );
    #[cfg(target_os = "linux")]
    let command = CommandSpec::new(
        OsString::from("/bin/sh"),
        [
            OsString::from("-c"),
            OsString::from(
                "read line; printf '%s\\n' '{\"id\":1,\"result\":{}}'; printf '%s\\n' 'separate diagnostic' >&2",
            ),
        ],
    );

    run_capture(CaptureConfig {
        input: PathBuf::from(&input),
        output: PathBuf::from(&output),
        command,
    })
    .unwrap();

    let records = std::fs::read_to_string(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<CaptureRecord>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .any(|item| item.channel == CaptureChannel::ClientToServer)
    );
    assert!(
        records
            .iter()
            .any(|item| item.channel == CaptureChannel::ServerToClient)
    );
    assert!(records.iter().any(|item| {
        item.channel == CaptureChannel::Stderr && item.line == "separate diagnostic"
    }));
    assert!(
        records
            .windows(2)
            .all(|pair| pair[0].monotonic_ns <= pair[1].monotonic_ns)
    );

    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[derive(Clone, Default)]
struct SharedOutput(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct BlockingReader(std::sync::mpsc::Receiver<Vec<u8>>);

impl std::io::Read for BlockingReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self.0.recv() {
            Ok(chunk) => {
                let len = chunk.len().min(bytes.len());
                bytes[..len].copy_from_slice(&chunk[..len]);
                Ok(len)
            }
            Err(_) => Ok(0),
        }
    }
}

struct TriggeredFiniteReader {
    trigger: std::sync::mpsc::Receiver<()>,
    payload: Option<Vec<u8>>,
}

impl std::io::Read for TriggeredFiniteReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let Some(payload) = self.payload.take() else {
            return Ok(0);
        };
        self.trigger
            .recv()
            .map_err(|_| std::io::Error::other("trigger closed"))?;
        let len = payload.len().min(bytes.len());
        bytes[..len].copy_from_slice(&payload[..len]);
        Ok(len)
    }
}

struct TriggerInputOnDrop(Option<std::sync::mpsc::Sender<()>>);

impl std::io::Write for TriggerInputOnDrop {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for TriggerInputOnDrop {
    fn drop(&mut self) {
        if let Some(trigger) = self.0.take() {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = trigger.send(());
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

#[test]
fn proxy_forwards_only_server_protocol_lines_to_its_stdout() {
    let stem = format!("codex-proxy-test-{}", std::process::id());
    let output = std::env::temp_dir().join(format!("{stem}.capture.jsonl"));
    let forwarded = SharedOutput::default();

    #[cfg(windows)]
    let command = CommandSpec::new(
        OsString::from("powershell.exe"),
        [
            "-NoProfile",
            "-Command",
            "$null=[Console]::In.ReadLine(); [Console]::Out.WriteLine('{\"id\":1,\"result\":{}}'); [Console]::Error.WriteLine('must-not-reach-stdout')",
        ]
        .map(OsString::from),
    );
    #[cfg(target_os = "linux")]
    let command = CommandSpec::new(
        OsString::from("/bin/sh"),
        [
            OsString::from("-c"),
            OsString::from(
                "read line; printf '%s\\n' '{\"id\":1,\"result\":{}}'; printf '%s\\n' 'must-not-reach-stdout' >&2",
            ),
        ],
    );

    run_proxy(
        ProxyConfig {
            output: output.clone(),
            command,
        },
        std::io::Cursor::new(b"{\"id\":1,\"method\":\"initialize\"}\n".to_vec()),
        forwarded.clone(),
    )
    .unwrap();

    let forwarded = String::from_utf8(forwarded.0.lock().unwrap().clone()).unwrap();
    assert_eq!(forwarded, "{\"id\":1,\"result\":{}}\n");
    assert!(!forwarded.contains("must-not-reach-stdout"));
    std::fs::remove_file(output).unwrap();
}

#[test]
fn replay_rejects_identical_input_and_output_without_truncating_input() {
    let path = std::env::temp_dir().join(format!(
        "codex-capture-same-path-{}.jsonl",
        std::process::id()
    ));
    let original = b"{\"id\":1,\"method\":\"initialize\"}\n";
    std::fs::write(&path, original).unwrap();

    let error = run_capture(CaptureConfig {
        input: path.clone(),
        output: path.clone(),
        command: CommandSpec::codex(Some(OsString::from("must-not-launch"))),
    })
    .unwrap_err();

    assert!(error.to_string().contains("same file"));
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let alias = path
        .parent()
        .unwrap()
        .join(".")
        .join(path.file_name().unwrap());
    let alias_error = run_capture(CaptureConfig {
        input: path.clone(),
        output: alias,
        command: CommandSpec::codex(Some(OsString::from("must-not-launch"))),
    })
    .unwrap_err();
    assert!(alias_error.to_string().contains("same file"));
    assert_eq!(std::fs::read(&path).unwrap(), original);

    std::fs::remove_file(path).unwrap();
}

#[test]
fn replay_rejects_hard_link_alias_without_truncating_input() {
    let stem = format!("codex-capture-hard-link-{}", std::process::id());
    let input = std::env::temp_dir().join(format!("{stem}.input.jsonl"));
    let output = std::env::temp_dir().join(format!("{stem}.alias.jsonl"));
    let original = b"{\"id\":1,\"method\":\"initialize\"}\n";
    std::fs::write(&input, original).unwrap();
    if let Err(error) = std::fs::hard_link(&input, &output) {
        if matches!(
            error.kind(),
            std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
        ) {
            eprintln!("skipping hard-link identity test: {error}");
            std::fs::remove_file(input).unwrap();
            return;
        }
        panic!("hard-link test setup failed unexpectedly: {error}");
    }

    let error = run_capture(CaptureConfig {
        input: input.clone(),
        output: output.clone(),
        command: CommandSpec::codex(Some(OsString::from("must-not-launch"))),
    })
    .unwrap_err();

    assert!(error.to_string().contains("same file"));
    assert_eq!(std::fs::read(&input).unwrap(), original);
    assert_eq!(std::fs::read(&output).unwrap(), original);
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
}

#[test]
fn proxy_returns_when_child_exits_while_interactive_input_remains_open() {
    let output = std::env::temp_dir().join(format!(
        "codex-proxy-early-exit-{}.jsonl",
        std::process::id()
    ));
    let (keep_input_open, input) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();

    #[cfg(windows)]
    let command = CommandSpec::new(
        OsString::from("powershell.exe"),
        ["-NoProfile", "-Command", "exit 0"].map(OsString::from),
    );
    #[cfg(target_os = "linux")]
    let command = CommandSpec::new(
        OsString::from("/bin/sh"),
        [OsString::from("-c"), OsString::from("exit 0")],
    );

    let output_for_driver = output.clone();
    std::thread::spawn(move || {
        let result = run_proxy(
            ProxyConfig {
                output: output_for_driver,
                command,
            },
            BlockingReader(input),
            SharedOutput::default(),
        );
        finished_tx
            .send(result.map_err(|error| error.to_string()))
            .unwrap();
    });

    let result = finished_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("proxy remained blocked on open interactive input after child exit");
    assert!(
        result
            .unwrap_err()
            .contains("before client input completed")
    );

    drop(keep_input_open);
    std::fs::remove_file(output).unwrap();
}

#[test]
fn proxy_rejects_finite_client_frames_queued_after_child_exit() {
    let output = std::env::temp_dir().join(format!(
        "codex-proxy-unsent-input-{}.jsonl",
        std::process::id()
    ));
    let (trigger_tx, trigger_rx) = std::sync::mpsc::channel();

    #[cfg(windows)]
    let command = CommandSpec::new(
        OsString::from("powershell.exe"),
        ["-NoProfile", "-Command", "exit 0"].map(OsString::from),
    );
    #[cfg(target_os = "linux")]
    let command = CommandSpec::new(
        OsString::from("/bin/sh"),
        [OsString::from("-c"), OsString::from("exit 0")],
    );

    let result = run_proxy(
        ProxyConfig {
            output: output.clone(),
            command,
        },
        TriggeredFiniteReader {
            trigger: trigger_rx,
            payload: Some(b"{\"id\":1,\"method\":\"initialize\"}\n".to_vec()),
        },
        TriggerInputOnDrop(Some(trigger_tx)),
    );

    let error = result.expect_err("queued client frame was silently accepted after child exit");
    let message = error.to_string();
    assert!(message.contains("unsent client frame"), "{message}");
    let capture = std::fs::read_to_string(&output).unwrap();
    assert!(!capture.contains("client_to_server"));
    std::fs::remove_file(output).unwrap();
}

#[cfg(unix)]
#[test]
fn capture_output_permissions_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let stem = format!("codex-capture-mode-{}", std::process::id());
    let input = std::env::temp_dir().join(format!("{stem}.input.jsonl"));
    let output = std::env::temp_dir().join(format!("{stem}.capture.jsonl"));
    std::fs::write(&input, "{\"id\":1}\n").unwrap();
    let command = CommandSpec::new(
        OsString::from("/bin/sh"),
        [OsString::from("-c"), OsString::from("read line")],
    );

    run_capture(CaptureConfig {
        input: input.clone(),
        output: output.clone(),
        command,
    })
    .unwrap();

    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_file(input).unwrap();
    std::fs::remove_file(output).unwrap();
}
