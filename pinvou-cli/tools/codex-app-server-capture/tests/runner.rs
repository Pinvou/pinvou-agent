use std::process::Command;

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
fn run_s2_orchestrates_a_through_d_and_writes_sanitized_artifacts() {
    let output = temp_output("success");
    let result = run_fake("success", &output, 2_000);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

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
fn run_s2_fails_closed_on_auth_and_quota_preflight_without_leaking_account_data() {
    for (mode, counter) in [("auth", "auth_errors"), ("quota", "quota_errors")] {
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
fn run_s2_timeout_is_bounded_and_produces_invalid_artifacts() {
    let output = temp_output("timeout");
    let start = std::time::Instant::now();
    let result = run_fake("timeout", &output, 100);
    assert!(!result.status.success());
    assert!(start.elapsed() < std::time::Duration::from_secs(3));
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
