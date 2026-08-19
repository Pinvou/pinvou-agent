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
