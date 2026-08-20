#[cfg(debug_assertions)]
use std::ffi::OsString;
use std::process::Command;
use std::time::Duration;

#[cfg(debug_assertions)]
use codex_app_server_capture::protocol::CaptureChannel;
use codex_app_server_capture::protocol::CaptureRecord;
#[cfg(debug_assertions)]
use codex_app_server_capture::runner::{S2RunConfig, run_s2_for_test};
use serde_json::Value;

fn temp_output(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "codex-s2-runner-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn run_fake(
    mode: &str,
    output: &std::path::Path,
    scenario_timeout_ms: u64,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            &scenario_timeout_ms.to_string(),
            "--global-timeout-ms",
            "10000",
        ])
        .env("S2_FAKE_MODE", mode)
        .output()
        .unwrap()
}

#[test]
#[cfg(debug_assertions)]
fn run_s2_orchestrates_a_through_d_and_writes_sanitized_artifacts() {
    let output = temp_output("success");
    let outcome = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        model: None,
        scenario_timeout: Duration::from_secs(2),
        global_timeout: Duration::from_secs(10),
    })
    .unwrap();
    assert!(outcome.report.valid);

    for name in [
        "capture.jsonl",
        "evidence.json",
        "validation-report.json",
        "summary.txt",
    ] {
        assert!(output.join(name).is_file(), "missing {name}");
    }
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["scenarios"].as_array().unwrap().len(), 4);
    assert_eq!(evidence["scenarios"][3]["terminal_state"], "interrupted");
    assert_eq!(evidence["performance"]["merge_window_ms"], 50);
    assert_eq!(evidence["performance"]["event_sizes"]["samples"], 40);
    assert!(evidence["candidate_percentiles"]["interrupt_response_latency_ms"].is_number());
    assert!(evidence["candidate_percentiles"]["interrupt_terminal_latency_ms"].is_number());
    let summary = std::fs::read_to_string(output.join("summary.txt")).unwrap();
    assert!(summary.contains("interrupt_response_latency_ms="));
    assert!(summary.contains("interrupt_terminal_latency_ms="));
    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["valid"], true);
    let sanitized = format!(
        "{}{}",
        std::fs::read_to_string(output.join("evidence.json")).unwrap(),
        std::fs::read_to_string(output.join("summary.txt")).unwrap()
    );
    assert!(!sanitized.contains("private@example.invalid"));
    assert!(!sanitized.contains("chatgpt"));
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn command_execution_output_deltas_contribute_only_when_correlated() {
    let output = temp_output("command-output-success");
    let outcome = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        model: None,
        scenario_timeout: Duration::from_secs(2),
        global_timeout: Duration::from_secs(10),
    })
    .unwrap();
    assert!(outcome.report.valid);
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["performance"]["event_sizes"]["samples"], 40);
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn scenario_c_uses_read_only_on_request_and_exact_marker_approval() {
    let output = temp_output("spaces & semicolon ; safe");
    let workspace = output.join("workspace");
    run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        model: None,
        scenario_timeout: Duration::from_secs(2),
        global_timeout: Duration::from_secs(10),
    })
    .unwrap();
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    let records = capture
        .lines()
        .filter_map(|line| serde_json::from_str::<CaptureRecord>(line).ok())
        .filter_map(|record| {
            serde_json::from_str::<Value>(&record.line)
                .ok()
                .map(|frame| (record.channel, frame))
        })
        .collect::<Vec<_>>();
    let thread_start = records
        .iter()
        .map(|(_, frame)| frame)
        .find(|frame| {
            frame["method"] == "thread/start" && frame["params"]["approvalPolicy"] == "on-request"
        })
        .unwrap();
    assert_eq!(thread_start["params"]["sandbox"], "read-only");
    let turn_start = records
        .iter()
        .map(|(_, frame)| frame)
        .find(|frame| {
            frame["method"] == "turn/start"
                && frame
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
                    .is_some_and(|prompt| prompt.contains("S2-C"))
        })
        .unwrap();
    assert_eq!(turn_start["params"]["approvalPolicy"], "on-request");
    assert_eq!(turn_start["params"]["sandboxPolicy"]["type"], "readOnly");
    for scenario in ["S2-A", "S2-B", "S2-D"] {
        let prompt = records
            .iter()
            .map(|(_, frame)| frame)
            .filter(|frame| frame["method"] == "turn/start")
            .filter_map(|frame| {
                frame
                    .pointer("/params/input/0/text")
                    .and_then(Value::as_str)
            })
            .find(|prompt| prompt.contains(scenario))
            .unwrap();
        assert!(prompt.contains("Execute exactly this command once"));
        assert!(prompt.contains("COMMAND_BEGIN"));
        assert!(!prompt.contains(output.to_string_lossy().as_ref()));
    }
    let approvals = records
        .iter()
        .map(|(_, frame)| frame)
        .filter(|frame| frame["method"] == "item/commandExecution/requestApproval")
        .collect::<Vec<_>>();
    assert_eq!(approvals.len(), 1);
    let approval = approvals[0];
    let command = approval
        .pointer("/params/command")
        .and_then(Value::as_str)
        .unwrap();
    assert!(!command.contains("spaces & semicolon ; safe"));
    assert!(!command.contains('&'));
    assert!(!command.contains(';'));
    assert!(command.contains(".codex-s2-approval-marker"));
    let response = records
        .iter()
        .find(|(channel, frame)| {
            *channel == CaptureChannel::ClientToServer
                && frame["id"] == approval["id"]
                && frame["result"]["decision"] == "accept"
        })
        .unwrap();
    assert_eq!(response.1["result"]["decision"], "accept");
    assert!(workspace.join("cmd.exe").exists());
    #[cfg(windows)]
    {
        let executable = command
            .strip_prefix('"')
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let executable = std::path::Path::new(executable);
        assert!(executable.is_absolute());
        assert!(
            executable
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("system32\\cmd.exe")
        );
        assert_ne!(executable, workspace.join("cmd.exe"));
    }
    #[cfg(unix)]
    assert!(command.starts_with("/bin/sh "));
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(all(debug_assertions, windows))]
fn scenario_c_accepts_only_the_exact_observed_pwsh_wrapper() {
    let output = temp_output("wrapper-approval");
    let outcome = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        model: None,
        scenario_timeout: Duration::from_secs(2),
        global_timeout: Duration::from_secs(10),
    })
    .unwrap();
    assert!(outcome.report.valid);
    std::fs::remove_dir_all(output).unwrap();

    for mode in [
        "wrapper-command-mutated",
        "wrapper-extra-action",
        "wrapper-wrong-pwsh",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 1_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(capture.contains(r#"\"decision\":\"cancel\""#));
        assert!(!capture.contains(r#"\"decision\":\"accept\""#));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
#[cfg(debug_assertions)]
fn version_preflight_and_app_server_share_one_global_deadline() {
    let output = temp_output("version-budget");
    let executable = output.join("fake-version-budget.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-app-server"), &executable).unwrap();
    let start = std::time::Instant::now();
    let result = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(executable.into_os_string()),
        model: None,
        scenario_timeout: Duration::from_secs(5),
        global_timeout: Duration::from_millis(1200),
    });
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_millis(1800));
    std::fs::remove_dir_all(output).unwrap();
}

#[cfg(windows)]
fn process_has_exited(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return true;
    }
    let exited = unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0;
    unsafe { CloseHandle(handle) };
    exited
}

#[cfg(windows)]
fn write_fake_codex_cmd(directory: &std::path::Path, name: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let script = directory.join(name);
    let fake = env!("CARGO_BIN_EXE_fake-app-server");
    std::fs::write(
        &script,
        format!(
            "@echo off\r\nif defined S2_CMDLINE_MARKER echo %cmdcmdline% > \"%S2_CMDLINE_MARKER%\"\r\n\"{fake}\" %*\r\n"
        ),
    )
    .unwrap();
    script
}

#[cfg(windows)]
fn path_with_planted_pwsh(root: &std::path::Path) -> (std::path::PathBuf, std::ffi::OsString) {
    let planted_dir = root.join("planted-pwsh");
    std::fs::create_dir_all(&planted_dir).unwrap();
    let planted = planted_dir.join("pwsh.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-app-server"), &planted).unwrap();
    let mut paths = vec![planted_dir];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    (planted, std::env::join_paths(paths).unwrap())
}

#[test]
#[cfg(windows)]
fn windows_streaming_stimuli_ignore_path_planted_pwsh() {
    let root = temp_output("protected-streaming-shell");
    let output = root.join("output");
    let (planted, path) = path_with_planted_pwsh(&root);
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            "2000",
            "--global-timeout-ms",
            "10000",
        ])
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "production thresholds must stay strict"
    );
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    let planted = planted.to_string_lossy();
    let prompts = capture
        .lines()
        .filter_map(|line| serde_json::from_str::<CaptureRecord>(line).ok())
        .filter_map(|record| serde_json::from_str::<Value>(&record.line).ok())
        .filter(|frame| frame["method"] == "turn/start")
        .filter_map(|frame| {
            frame
                .pointer("/params/input/0/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .filter(|prompt| {
            ["S2-A", "S2-B", "S2-D"]
                .iter()
                .any(|name| prompt.contains(name))
        })
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 3);
    assert!(
        prompts
            .iter()
            .all(|prompt| !prompt.contains(planted.as_ref()))
    );
    assert!(prompts.iter().all(|prompt| {
        prompt
            .to_ascii_lowercase()
            .contains(r"windowspowershell\v1.0\powershell.exe")
    }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn scenario_c_rejects_path_planted_pwsh_wrapper() {
    let root = temp_output("untrusted-wrapper-shell");
    let output = root.join("output");
    let (_planted, path) = path_with_planted_pwsh(&root);
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            "2000",
            "--global-timeout-ms",
            "10000",
        ])
        .env("PATH", path)
        .env("S2_FAKE_MODE", "wrapper-approval")
        .output()
        .unwrap();
    assert!(!result.status.success());
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    assert!(capture.contains(r#"\"decision\":\"cancel\""#));
    assert!(!capture.contains(r#"\"decision\":\"accept\""#));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn default_windows_resolution_prefers_working_codex_cmd_over_extensionless_shim() {
    let root = temp_output("windows-cmd-path");
    let bin = root.join("bin");
    let output = root.join("output");
    let command_line_marker = root.join("cmdline.txt");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("codex"), b"#!/bin/sh\nexit 99\n").unwrap();
    write_fake_codex_cmd(&bin, "codex.cmd");
    let pwsh_dir = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .find(|directory| directory.join("pwsh.exe").is_file())
        .unwrap();
    let isolated_path = std::env::join_paths([bin.as_path(), pwsh_dir.as_path()]).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--scenario-timeout-ms",
            "2000",
            "--global-timeout-ms",
            "10000",
        ])
        .env("PATH", isolated_path)
        .env("S2_CMDLINE_MARKER", &command_line_marker)
        .output()
        .unwrap();

    assert!(
        !result.status.success(),
        "production thresholds should remain strict"
    );
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap_or_else(|error| {
        panic!(
            "capture missing: {error}; stderr={}",
            format!(
                "{}; cmdline={}",
                String::from_utf8_lossy(&result.stderr),
                std::fs::read_to_string(&command_line_marker).unwrap_or_default()
            )
        )
    });
    assert!(capture.contains("initialize"));
    assert!(capture.contains("thread/start"));
    let command_line = std::fs::read_to_string(&command_line_marker).unwrap();
    assert!(command_line.contains("codex.cmd"));
    assert!(command_line.contains("app-server --stdio"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn explicit_cmd_path_with_command_metacharacters_is_rejected_before_execution() {
    let root = temp_output("windows-cmd-metachar");
    let unsafe_dir = root.join("unsafe&dir");
    let output = root.join("output");
    let script = write_fake_codex_cmd(&unsafe_dir, "codex.cmd");

    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args(["--executable"])
        .arg(&script)
        .args([
            "--scenario-timeout-ms",
            "500",
            "--global-timeout-ms",
            "3000",
        ])
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(!output.join("capture.jsonl").exists());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("unsafe command path"),
        "stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn contained_process_kills_version_and_immediate_app_descendants() {
    for mode in ["version-descendant", "immediate-child"] {
        let output = temp_output(mode);
        let marker = output.join("descendant.pid");
        let start = std::time::Instant::now();
        let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
            .args(["run-s2", "--output-dir"])
            .arg(&output)
            .args([
                "--executable",
                env!("CARGO_BIN_EXE_fake-app-server"),
                "--scenario-timeout-ms",
                "300",
                "--global-timeout-ms",
                "3000",
            ])
            .env("S2_FAKE_MODE", mode)
            .env("S2_FAKE_MARKER", &marker)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "{mode} cleanup hung"
        );
        let pid = std::fs::read_to_string(&marker)
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assert!(
            process_has_exited(pid),
            "{mode} descendant escaped containment"
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn production_thresholds_reject_short_a_and_tiny_b() {
    let output = temp_output("production-thresholds");
    let result = run_fake("success", &output, 250);
    assert!(!result.status.success());
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["scenarios"][0]["r1_sufficient"], false);
    assert_eq!(evidence["scenarios"][1]["r1_sufficient"], false);
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
fn run_s2_fails_closed_on_auth_and_quota_preflight_without_leaking_account_data() {
    for (mode, counter) in [
        ("auth", "auth_errors"),
        ("quota", "quota_errors"),
        ("bad-init", "protocol_errors"),
        ("bad-account", "protocol_errors"),
        ("bad-limits", "protocol_errors"),
        ("quota-float", "quota_errors"),
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 1_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
        assert_eq!(evidence[counter], 1);
        let report: Value =
            serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
                .unwrap();
        assert_eq!(report["valid"], false);
        assert!(report["pass_percentiles"].is_null());
        assert_eq!(report["baseline_update_allowed"], false);
        assert!(
            !std::fs::read_to_string(output.join("summary.txt"))
                .unwrap()
                .contains("all F1-F3 gates passed")
        );
        let public = format!(
            "{}{}{}",
            String::from_utf8_lossy(&result.stderr),
            std::fs::read_to_string(output.join("summary.txt")).unwrap(),
            std::fs::read_to_string(output.join("evidence.json")).unwrap()
        );
        assert!(!public.contains("private@example.invalid"));
        assert!(!public.contains("chatgpt"));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn version_preflight_requires_exact_pinned_cli_before_capture() {
    for mode in [
        "version-mismatch",
        "version-malformed",
        "version-nonzero",
        "version-timeout",
    ] {
        let output = temp_output(mode);
        let start = std::time::Instant::now();
        let result = run_fake(mode, &output, 500);
        assert!(!result.status.success());
        assert!(start.elapsed() < Duration::from_secs(12));
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
        assert_eq!(evidence["protocol_errors"], 1);
        assert!(!output.join("capture.jsonl").exists());
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn run_s2_rejects_missing_approval_interrupt_and_terminal_evidence() {
    for mode in [
        "missing-approval",
        "missing-interrupt-response",
        "missing-interrupted-terminal",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 250);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let report: Value =
            serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
                .unwrap();
        assert_eq!(report["valid"], false);
        assert!(report["pass_percentiles"].is_null());
        assert_eq!(report["baseline_update_allowed"], false);
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn run_s2_rejects_unexpected_approval_method_instead_of_auto_approving_it() {
    for mode in [
        "unexpected-approval",
        "unexpected-command",
        "outside-approval",
        "wrong-approval-thread",
        "wrong-approval-turn",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 1_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(capture.contains(r#"\"decision\":\"cancel\""#));
        assert!(!capture.contains(r#"\"decision\":\"accept\""#));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn run_s2_rejects_approval_from_in_workspace_child_cwd() {
    let output = temp_output("child-approval");
    let result = run_fake("child-approval", &output, 1_000);
    assert!(!result.status.success());
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    assert!(capture.contains(r#"\"decision\":\"cancel\""#));
    assert!(!capture.contains(r#"\"decision\":\"accept\""#));
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn interrupt_timings_start_at_the_correlated_request_write() {
    let output = temp_output("timing");
    run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        model: None,
        scenario_timeout: Duration::from_secs(2),
        global_timeout: Duration::from_secs(10),
    })
    .unwrap();
    let records = std::fs::read_to_string(output.join("capture.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<CaptureRecord>(line).unwrap())
        .collect::<Vec<_>>();
    let interrupt = records
        .iter()
        .enumerate()
        .find_map(|(index, record)| {
            let frame: Value = serde_json::from_str(&record.line).ok()?;
            (record.channel == CaptureChannel::ClientToServer
                && frame["method"] == "turn/interrupt")
                .then(|| (index, record.monotonic_ns, frame["id"].clone()))
        })
        .unwrap();
    let d_deltas = records[..interrupt.0]
        .iter()
        .filter_map(|record| serde_json::from_str::<Value>(&record.line).ok())
        .filter(|frame| {
            frame["method"] == "item/agentMessage/delta"
                && frame.pointer("/params/threadId") == Some(&Value::String("thread-3".to_owned()))
        })
        .collect::<Vec<_>>();
    assert!(d_deltas.len() >= 2);
    assert!(
        d_deltas
            .iter()
            .filter_map(|frame| frame.pointer("/params/delta").and_then(Value::as_str))
            .map(str::len)
            .sum::<usize>()
            >= 64
    );
    let response_ns = records
        .iter()
        .find_map(|record| {
            let frame: Value = serde_json::from_str(&record.line).ok()?;
            (record.channel == CaptureChannel::ServerToClient
                && frame.get("id") == Some(&interrupt.2)
                && frame.get("method").is_none())
            .then_some(record.monotonic_ns)
        })
        .unwrap();
    let terminal_ns = records
        .iter()
        .find_map(|record| {
            let frame: Value = serde_json::from_str(&record.line).ok()?;
            (record.channel == CaptureChannel::ServerToClient
                && frame["method"] == "turn/completed"
                && frame.pointer("/params/turn/status")
                    == Some(&Value::String("interrupted".to_owned())))
            .then_some(record.monotonic_ns)
        })
        .unwrap();
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    let response_ms = evidence["candidate_percentiles"]["interrupt_response_latency_ms"]
        .as_f64()
        .unwrap();
    let terminal_ms = evidence["candidate_percentiles"]["interrupt_terminal_latency_ms"]
        .as_f64()
        .unwrap();
    assert!((response_ms - (response_ns - interrupt.1) as f64 / 1_000_000.0).abs() < 0.01);
    assert!((terminal_ms - (terminal_ns - interrupt.1) as f64 / 1_000_000.0).abs() < 0.01);
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
fn one_scenario_deadline_covers_thread_turn_and_drive() {
    let output = temp_output("slow-phases");
    let start = std::time::Instant::now();
    let result = run_fake("slow-phases", &output, 200);
    assert!(!result.status.success());
    assert!(start.elapsed() < Duration::from_secs(8));
    let timestamps = std::fs::read_to_string(output.join("capture.jsonl"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<CaptureRecord>(line)
                .unwrap()
                .monotonic_ns
        })
        .collect::<Vec<_>>();
    let captured_span = timestamps.last().unwrap() - timestamps.first().unwrap();
    assert!(captured_span < 350_000_000, "scenario deadline was reset");
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
fn inherited_pipe_descendant_and_noise_flood_fail_with_bounded_return() {
    for mode in ["descendant-pipes", "noise-flood"] {
        let output = temp_output(mode);
        let start = std::time::Instant::now();
        let result = run_fake(mode, &output, 250);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "{mode} cleanup hung"
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn run_s2_timeout_is_bounded_and_produces_invalid_artifacts() {
    let output = temp_output("timeout");
    let start = std::time::Instant::now();
    let result = run_fake("timeout", &output, 100);
    assert!(!result.status.success());
    assert!(start.elapsed() < std::time::Duration::from_secs(8));
    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["valid"], false);
    assert!(
        std::fs::read_to_string(output.join("summary.txt"))
            .unwrap()
            .contains("timeout")
    );
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
fn run_s2_malformed_server_frame_is_a_protocol_error() {
    let output = temp_output("malformed");
    let result = run_fake("malformed", &output, 1_000);
    assert!(!result.status.success());
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["protocol_errors"], 1);
    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["valid"], false);
    assert!(report["pass_percentiles"].is_null());
    std::fs::remove_dir_all(output).unwrap();
}
