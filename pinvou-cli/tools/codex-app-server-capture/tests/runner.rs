#[cfg(debug_assertions)]
use std::ffi::OsString;
use std::io::Write;
use std::process::Command;
use std::time::Duration;

#[cfg(debug_assertions)]
use codex_app_server_capture::protocol::CaptureChannel;
use codex_app_server_capture::protocol::CaptureRecord;
#[cfg(debug_assertions)]
use codex_app_server_capture::runner::{
    S2RunConfig, run_marker_helper_process_for_test, run_s2_for_test,
};
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
            "60000",
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
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: Vec::new(),
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
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    assert!(capture.contains(r#"\"method\":\"thread/started\""#));
    assert!(capture.contains(r#"\"type\":\"userMessage\""#));
    assert!(capture.contains(r#"\"method\":\"rawResponseItem/completed\""#));
    assert!(capture.contains(r#"\"method\":\"mcpServer/startupStatus/updated\""#));
    assert!(capture.contains(r#"\"method\":\"warning\""#));
    assert!(capture.contains(r#"\"status\":\"ready\""#));
    assert!(capture.contains(r#"\"status\":\"failed\""#));
    assert!(capture.contains(r#"\"phase\":\"final_answer\""#));
    assert!(capture.contains(r#"\"phase\":null"#));
    let server_frames = capture
        .lines()
        .filter_map(|line| serde_json::from_str::<CaptureRecord>(line).ok())
        .filter(|record| record.channel == CaptureChannel::ServerToClient)
        .filter_map(|record| serde_json::from_str::<Value>(&record.line).ok())
        .collect::<Vec<_>>();
    let frame_index = |method: &str, name: Option<&str>, status: Option<&str>| {
        server_frames
            .iter()
            .position(|frame| {
                frame["method"] == method
                    && frame.pointer("/params/threadId").and_then(Value::as_str) == Some("thread-0")
                    && name.is_none_or(|value| {
                        frame.pointer("/params/name").and_then(Value::as_str) == Some(value)
                    })
                    && status.is_none_or(|value| {
                        frame.pointer("/params/status").and_then(Value::as_str) == Some(value)
                    })
            })
            .expect("missing expected thread-scoped MCP fixture frame")
    };
    let starting = frame_index(
        "mcpServer/startupStatus/updated",
        Some("fixture-failed"),
        Some("starting"),
    );
    let turn_started = frame_index("turn/started", None, None);
    let failed = frame_index(
        "mcpServer/startupStatus/updated",
        Some("fixture-failed"),
        Some("failed"),
    );
    assert!(starting < turn_started && turn_started < failed);
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
fn run_s2_isolates_startup_commands_only_for_app_server_invocation() {
    let output = temp_output("argv spaces & semicolon ; user-input");
    let argv_log = output.join("fake-argv.jsonl");
    run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: vec![(
            OsString::from("S2_FAKE_ARGV_LOG"),
            argv_log.clone().into_os_string(),
        )],
    })
    .unwrap();
    let invocations = std::fs::read_to_string(&argv_log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        invocations,
        vec![
            vec!["--version"],
            vec![
                "app-server",
                "--strict-config",
                "--disable",
                "hooks",
                "--disable",
                "plugins",
                "--disable",
                "apps",
                "--disable",
                "shell_snapshot",
                "--disable",
                "memories",
                "-c",
                "notify=[]",
                "-c",
                "project_root_markers=['.codex-s2-root']",
                "-c",
                "project_doc_max_bytes=0",
                "-c",
                "skills.include_instructions=false",
                "-c",
                "skills.bundled.enabled=false",
                "-c",
                "analytics.enabled=false",
                "-c",
                "otel.exporter='none'",
                "-c",
                "otel.trace_exporter='none'",
                "-c",
                "otel.metrics_exporter='none'",
                "--stdio",
            ],
        ]
    );
    assert!(
        !invocations.iter().flatten().any(|arg| arg == "codex_hooks"),
        "internal/legacy feature name must never be used as the CLI feature key"
    );
    assert!(
        invocations
            .iter()
            .flatten()
            .all(|arg| { !arg.contains("user-input") && !arg.contains('&') && !arg.contains(';') })
    );
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn run_s2_uses_a_fresh_minimal_codex_home_and_cleans_up_copied_auth() {
    const AUTH_FIXTURE: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"test-only"}}"#;
    const MINIMAL_CONFIG: &str = concat!(
        "cli_auth_credentials_store = \"file\"\n",
        "\n",
        "[analytics]\n",
        "enabled = false\n",
        "\n",
        "[otel]\n",
        "exporter = \"none\"\n",
        "trace_exporter = \"none\"\n",
        "metrics_exporter = \"none\"\n",
        "\n",
        "[skills]\n",
        "include_instructions = false\n",
        "\n",
        "[skills.bundled]\n",
        "enabled = false\n",
    );

    let root = temp_output("isolated-codex-home");
    let original_home = root.join("original-home");
    let output = root.join("output");
    let audit = root.join("home-audit.json");
    std::fs::create_dir(&original_home).unwrap();
    std::fs::write(original_home.join("auth.json"), AUTH_FIXTURE).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            original_home.join("auth.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    std::fs::write(
        original_home.join("config.toml"),
        concat!(
            "notify = [\"S2_UNTRUSTED_NOTIFY_MARKER\"]\n",
            "experimental_thread_config_endpoint = \"https://marker.invalid\"\n",
            "[mcp_servers.marker]\ncommand = \"S2_UNTRUSTED_MCP_MARKER\"\n",
            "[analytics]\nenabled = true\n",
            "[otel]\nexporter = \"statsig\"\n",
            "[features]\nmemories = true\nplugins = true\napps = true\n",
        ),
    )
    .unwrap();
    std::fs::create_dir(original_home.join("plugins")).unwrap();
    std::fs::create_dir(original_home.join("skills")).unwrap();
    std::fs::create_dir_all(original_home.join(".agents/skills/private-marker")).unwrap();
    std::fs::write(
        original_home.join(".agents/skills/private-marker/SKILL.md"),
        "private marker",
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            "10000",
            "--global-timeout-ms",
            "60000",
        ])
        .env("CODEX_HOME", &original_home)
        .env("HOME", &original_home)
        .env("USERPROFILE", &original_home)
        .env("S2_FAKE_HOME_AUDIT", &audit)
        .env("S2_FAKE_ORIGINAL_HOME", &original_home)
        .env("S2_FAKE_EXPECTED_AUTH", AUTH_FIXTURE)
        .env("S2_FAKE_REFRESH_AUTH", "valid")
        .env("OPENAI_API_KEY", "must-not-reach-child")
        .env("CODEX_API_KEY", "must-not-reach-child")
        .env("CODEX_ACCESS_TOKEN", "must-not-reach-child")
        .env("CODEX_EXEC_SERVER_URL", "https://must-not-reach.invalid")
        .env(
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "https://must-not-reach.invalid",
        )
        .env("TRACEPARENT", "must-not-reach-child")
        .env("CODEX_SQLITE_HOME", &original_home)
        .env("CODEX_ROLLOUT_TRACE_ROOT", &original_home)
        .env("CODEX_APP_SERVER_MANAGED_CONFIG_PATH", &original_home)
        .env("CODEX_TUI_SESSION_LOG_PATH", &original_home)
        .env("HTTPS_PROXY", "http://proxy.invalid:8443")
        .env("CODEX_CA_CERTIFICATE", "test-ca-marker")
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "production gates must remain strict"
    );
    let audit: Value = serde_json::from_slice(&std::fs::read(&audit).unwrap()).unwrap();
    assert_eq!(audit["isolated"], true);
    assert_eq!(audit["auth_matches"], true);
    assert_eq!(audit["config"], MINIMAL_CONFIG);
    assert_eq!(audit["auth_refresh_succeeded"], true);
    assert_eq!(audit["risky_env_absent"], true);
    assert_eq!(audit["proxy_preserved"], true);
    assert_eq!(audit["ca_preserved"], true);
    assert_eq!(audit["neutral_marker"], true);
    assert_eq!(audit["no_project_inputs"], true);
    assert_eq!(audit["real_home_skill_visible"], false);
    let isolated_home = std::path::PathBuf::from(audit["home"].as_str().unwrap());
    let neutral_root = std::path::PathBuf::from(audit["current_dir"].as_str().unwrap());
    assert_eq!(isolated_home.parent(), Some(neutral_root.as_path()));
    assert_eq!(
        std::path::Path::new(audit["home_env"].as_str().unwrap()),
        neutral_root
    );
    #[cfg(windows)]
    {
        assert_eq!(
            std::path::Path::new(audit["userprofile_env"].as_str().unwrap()),
            neutral_root
        );
        let rendered = neutral_root.to_string_lossy();
        assert_eq!(audit["homedrive_env"], &rendered[..2]);
        assert_eq!(audit["homepath_env"], &rendered[2..]);
    }
    assert!(!neutral_root.starts_with(&output));
    assert!(
        !isolated_home.exists(),
        "credential copy must be cleaned up"
    );
    assert!(
        !neutral_root.exists(),
        "neutral run root must be cleaned up"
    );
    let sanitized = format!(
        "{}{}{}",
        std::fs::read_to_string(output.join("evidence.json")).unwrap(),
        std::fs::read_to_string(output.join("validation-report.json")).unwrap(),
        std::fs::read_to_string(output.join("summary.txt")).unwrap(),
    );
    assert!(!sanitized.contains(AUTH_FIXTURE));
    assert!(!sanitized.contains(&isolated_home.to_string_lossy().into_owned()));
    assert!(!sanitized.contains("S2_UNTRUSTED"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn run_s2_rejects_missing_auth_generically_before_raw_capture() {
    let root = temp_output("isolated-home-missing-auth");
    let original_home = root.join("private-original-home");
    let output = root.join("output");
    std::fs::create_dir(&original_home).unwrap();
    std::fs::write(
        original_home.join("config.toml"),
        "[mcp_servers.marker]\ncommand = \"private-marker\"\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            "1000",
            "--global-timeout-ms",
            "10000",
        ])
        .env("CODEX_HOME", &original_home)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!output.join("capture.jsonl").exists());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("isolated Codex home preparation failed"));
    assert!(!stderr.contains(&original_home.to_string_lossy().into_owned()));
    assert!(!stderr.contains("private-marker"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_preflight_rejects_layers_requirements_and_side_effects_before_threads() {
    for mode in [
        "config-managed-layer",
        "config-side-effect",
        "config-malformed",
        "config-requirements",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 1_000);
        assert!(!result.status.success(), "{mode} unexpectedly succeeded");
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(capture.contains("config/read"));
        assert!(!capture.contains("thread/start"));
        let summary = std::fs::read_to_string(output.join("summary.txt")).unwrap();
        assert!(summary.contains("protocol/scenario precondition failed"));
        assert!(!summary.contains("enterpriseManaged"));
        assert!(!summary.contains("mcp_servers"));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
#[cfg(debug_assertions)]
fn invalid_refreshed_auth_fails_closed_during_post_run_validation() {
    let output = temp_output("invalid-refreshed-auth");
    let audit = output.join("home-audit.json");
    let result = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: vec![
            (
                OsString::from("S2_FAKE_HOME_AUDIT"),
                audit.clone().into_os_string(),
            ),
            (
                OsString::from("S2_FAKE_ORIGINAL_HOME"),
                OsString::from("original"),
            ),
            (
                OsString::from("S2_FAKE_EXPECTED_AUTH"),
                OsString::from("{}"),
            ),
            (
                OsString::from("S2_FAKE_REFRESH_AUTH"),
                OsString::from("invalid"),
            ),
        ],
    });
    let error = result.unwrap_err().to_string();
    assert!(error.contains("S2 artifacts:"), "unexpected error: {error}");
    let summary = std::fs::read_to_string(output.join("summary.txt")).unwrap();
    assert!(!summary.contains("not-json"));
    assert!(summary.contains("protocol/scenario precondition failed"));
    let home: Value = serde_json::from_slice(&std::fs::read(audit).unwrap()).unwrap();
    assert!(!std::path::Path::new(home["home"].as_str().unwrap()).exists());
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn isolated_home_files_are_locked_or_identity_tamper_fails_closed() {
    let output = temp_output("isolated-home-tamper");
    let audit = output.join("home-audit.json");
    let result = run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: vec![
            (
                OsString::from("S2_FAKE_HOME_AUDIT"),
                audit.clone().into_os_string(),
            ),
            (
                OsString::from("S2_FAKE_ORIGINAL_HOME"),
                OsString::from("not-the-isolated-home"),
            ),
            (
                OsString::from("S2_FAKE_EXPECTED_AUTH"),
                OsString::from("{}"),
            ),
            (OsString::from("S2_FAKE_TAMPER_HOME"), OsString::from("1")),
        ],
    });
    let audit: Value = serde_json::from_slice(&std::fs::read(&audit).unwrap()).unwrap();
    assert_eq!(
        audit["auth_write_blocked"], false,
        "the isolated auth file must remain writable for token refresh"
    );
    #[cfg(windows)]
    {
        assert_eq!(audit["config_replace_blocked"], true);
        assert!(result.is_ok(), "{result:?}");
    }
    #[cfg(unix)]
    {
        assert_eq!(audit["config_replace_blocked"], false);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("isolated Codex home cleanup failed"));
    }
    let isolated_home = std::path::PathBuf::from(audit["home"].as_str().unwrap());
    assert!(!isolated_home.exists());
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn agent_only_lifecycle_is_stateful_and_rejects_malformed_or_tool_raw_items() {
    for mode in [
        "agent-thread-wrong-id",
        "agent-thread-missing",
        "agent-thread-duplicate",
        "agent-thread-malformed",
        "agent-user-wrong-ids",
        "agent-user-duplicate",
        "agent-user-duplicate-completed",
        "agent-user-out-of-order",
        "agent-user-prompt-mismatch",
        "agent-user-malformed",
        "agent-user-item-id-mismatch",
        "agent-user-missing-completed",
        "agent-user-raw-missing",
        "agent-user-raw-late",
        "agent-user-raw-duplicate",
        "agent-user-raw-wrong-thread",
        "agent-user-raw-wrong-turn",
        "agent-user-raw-role",
        "agent-user-raw-text",
        "agent-user-raw-multipart",
        "agent-user-raw-malformed",
        "agent-raw-wrong-ids",
        "agent-raw-wrong-turn",
        "agent-raw-duplicate",
        "agent-raw-out-of-order",
        "agent-raw-role-user",
        "agent-raw-function-call",
        "agent-raw-local-shell",
        "agent-raw-web-search",
        "agent-raw-computer",
        "agent-raw-tool-output",
        "agent-raw-custom-tool",
        "agent-raw-unknown",
        "agent-raw-malformed",
    ] {
        let output = temp_output(mode);
        let result = run_s2_for_test(S2RunConfig {
            output_dir: Some(output.clone()),
            executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
            trusted_approval_wrapper: None,
            model: None,
            scenario_timeout: Duration::from_secs(10),
            global_timeout: Duration::from_secs(60),
            test_child_env: vec![(OsString::from("S2_FAKE_MODE"), OsString::from(mode))],
        });
        assert!(result.is_err(), "{mode} unexpectedly passed");
        let execution_error = format!("{:#}", result.unwrap_err());
        let report: Value =
            serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
                .unwrap();
        assert_eq!(report["valid"], false, "{mode}");
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
        assert!(
            evidence["protocol_errors"]
                .as_u64()
                .is_some_and(|count| count > 0),
            "{mode} did not increment protocol_errors: {evidence}"
        );
        let expected_rejection = match mode {
            "agent-thread-wrong-id" | "agent-thread-duplicate" | "agent-thread-malformed" => {
                "malformed, duplicate, or out-of-order thread/started"
            }
            "agent-thread-missing" => "duplicate or out-of-order turn/started",
            "agent-user-wrong-ids" => "malformed item lifecycle notification",
            "agent-user-duplicate" | "agent-user-raw-missing" | "agent-user-raw-late" => {
                "duplicate or out-of-order user message start"
            }
            "agent-user-duplicate-completed"
            | "agent-user-out-of-order"
            | "agent-user-item-id-mismatch" => "mismatched or out-of-order user message completion",
            "agent-user-prompt-mismatch" | "agent-user-malformed" => {
                "malformed or mismatched user message"
            }
            "agent-user-missing-completed" => "agent item arrived before user lifecycle completed",
            "agent-user-raw-duplicate" => {
                "non-user raw item arrived before user lifecycle completed"
            }
            "agent-user-raw-wrong-thread"
            | "agent-user-raw-wrong-turn"
            | "agent-raw-wrong-ids"
            | "agent-raw-wrong-turn" => "raw response item had mismatched identity or order",
            "agent-user-raw-role"
            | "agent-user-raw-text"
            | "agent-user-raw-multipart"
            | "agent-user-raw-malformed" => "malformed or mismatched leading user raw item",
            "agent-raw-duplicate" => "duplicate raw response item",
            "agent-raw-out-of-order" => "raw response item was out of order or duplicate",
            "agent-raw-role-user"
            | "agent-raw-function-call"
            | "agent-raw-local-shell"
            | "agent-raw-web-search"
            | "agent-raw-computer"
            | "agent-raw-tool-output"
            | "agent-raw-custom-tool"
            | "agent-raw-unknown"
            | "agent-raw-malformed" => "malformed or tool raw response item is forbidden",
            _ => unreachable!("unmapped fake mode {mode}"),
        };
        assert!(
            execution_error.contains(expected_rejection),
            "{mode} did not execute target rejection {expected_rejection}: {execution_error}"
        );
        let expected_method = if mode.starts_with("agent-thread-") {
            if mode == "agent-thread-missing" {
                "turn/started"
            } else {
                "thread/started"
            }
        } else if mode == "agent-user-raw-missing" {
            "item/started"
        } else if mode.starts_with("agent-user-raw-") || mode.starts_with("agent-raw-") {
            "rawResponseItem/completed"
        } else {
            "item/started"
        };
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(
            capture.contains(&format!(r#"\"method\":\"{expected_method}\""#)),
            "{mode} capture lacked the offending {expected_method} frame"
        );
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
#[cfg(debug_assertions)]
fn mcp_startup_status_wiring_rejects_bad_shape_identity_error_and_transitions() {
    for mode in [
        "mcp-startup-extra",
        "mcp-startup-null-thread",
        "mcp-startup-wrong-thread",
        "mcp-startup-empty-name",
        "mcp-startup-status",
        "mcp-startup-starting-error",
        "mcp-startup-ready-error",
        "mcp-startup-failed-null",
        "mcp-startup-terminal-first",
        "mcp-startup-duplicate-start",
        "mcp-startup-duplicate-terminal",
        "mcp-startup-conflicting-terminal",
    ] {
        let output = temp_output(mode);
        let result = run_s2_for_test(S2RunConfig {
            output_dir: Some(output.clone()),
            executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
            trusted_approval_wrapper: None,
            model: None,
            scenario_timeout: Duration::from_secs(10),
            global_timeout: Duration::from_secs(60),
            test_child_env: vec![(OsString::from("S2_FAKE_MODE"), OsString::from(mode))],
        });
        let error = format!(
            "{:#}",
            result.expect_err("bad MCP status unexpectedly passed")
        );
        let expected = match mode {
            "mcp-startup-wrong-thread" => {
                "MCP startup status had mismatched thread identity or order"
            }
            "mcp-startup-terminal-first"
            | "mcp-startup-duplicate-start"
            | "mcp-startup-duplicate-terminal"
            | "mcp-startup-conflicting-terminal" => "invalid MCP startup status transition",
            _ => "malformed MCP startup status notification",
        };
        assert!(
            error.contains(expected),
            "{mode} did not execute target rejection {expected}: {error}"
        );
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
        assert!(
            evidence["protocol_errors"]
                .as_u64()
                .is_some_and(|count| count > 0)
        );
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(capture.contains(r#"\"method\":\"mcpServer/startupStatus/updated\""#));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
#[cfg(debug_assertions)]
fn method_named_error_server_request_is_rejected_with_a_correlated_error_response() {
    let output = temp_output("agent-error-request");
    let result = run_fake("agent-error-request", &output, 10_000);
    assert!(!result.status.success());
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    let response_seen = capture
        .lines()
        .filter_map(|line| serde_json::from_str::<CaptureRecord>(line).ok())
        .filter(|record| record.channel == CaptureChannel::ClientToServer)
        .filter_map(|record| serde_json::from_str::<Value>(&record.line).ok())
        .any(|frame| frame["id"] == 902 && frame["error"]["code"] == -32601);
    assert!(response_seen, "method-named error request was not answered");
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn command_execution_output_deltas_are_rejected_in_agent_only_scenarios() {
    let output = temp_output("command-output-success");
    let outcome = run_fake("command-output-success", &output, 10_000);
    assert!(!outcome.status.success());
    let report: Value =
        serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["valid"], false);
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn agent_only_scenarios_fail_closed_on_tools_requests_and_non_delta_output() {
    for mode in [
        "agent-tool-command",
        "agent-tool-file",
        "agent-tool-mcp",
        "agent-tool-dynamic",
        "agent-tool-web",
        "agent-tool-collab",
        "agent-hook-started",
        "agent-tool-unknown",
        "agent-server-request",
        "agent-aggregated-only",
        "agent-wrong-ids",
        "agent-empty-delta",
        "agent-early-complete",
        "agent-d-early-complete",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 10_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        if mode == "agent-hook-started" {
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(
                stderr
                    .contains("protocol error: tool activity is forbidden in agent-only scenario"),
                "hook/started did not reach the explicit fail-closed branch: {stderr}"
            );
            let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
            assert!(capture.contains(r#"\"method\":\"hook/started\""#));
        }
        let report: Value =
            serde_json::from_slice(&std::fs::read(output.join("validation-report.json")).unwrap())
                .unwrap();
        assert_eq!(report["valid"], false, "{mode}");
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
#[cfg(debug_assertions)]
fn scenario_c_uses_read_only_on_request_and_exact_marker_approval() {
    let output = temp_output("spaces & semicolon ; safe");
    run_s2_for_test(S2RunConfig {
        output_dir: Some(output.clone()),
        executable: Some(OsString::from(env!("CARGO_BIN_EXE_fake-app-server"))),
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: Vec::new(),
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
    let scenario_cwds = records
        .iter()
        .map(|(_, frame)| frame)
        .filter(|frame| frame["method"] == "thread/start")
        .filter_map(|frame| frame.pointer("/params/cwd").and_then(Value::as_str))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(scenario_cwds.len(), 4);
    let workspace_root = std::path::PathBuf::from(scenario_cwds.iter().next().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(!workspace_root.starts_with(&output));
    let config_read = records
        .iter()
        .map(|(_, frame)| frame)
        .find(|frame| frame["method"] == "config/read")
        .unwrap();
    assert_eq!(config_read["params"]["includeLayers"], true);
    let requirements_read = records
        .iter()
        .map(|(_, frame)| frame)
        .find(|frame| frame["method"] == "configRequirements/read")
        .unwrap();
    assert!(
        requirements_read.get("params").is_none(),
        "0.139 requires configRequirements/read params to be omitted"
    );
    let request_index = |method: &str| {
        records
            .iter()
            .position(|(channel, frame)| {
                *channel == CaptureChannel::ClientToServer && frame["method"] == method
            })
            .unwrap()
    };
    assert!(request_index("config/read") < request_index("account/read"));
    assert!(request_index("configRequirements/read") < request_index("account/read"));
    let config_cwd = config_read
        .pointer("/params/cwd")
        .and_then(Value::as_str)
        .unwrap();
    let expected_root = workspace_root.parent().unwrap().to_string_lossy();
    #[cfg(windows)]
    assert_eq!(
        config_cwd.strip_prefix(r"\\?\").unwrap_or(config_cwd),
        expected_root
            .strip_prefix(r"\\?\")
            .unwrap_or(expected_root.as_ref())
    );
    #[cfg(not(windows))]
    assert_eq!(config_cwd, expected_root);
    for (name, tag) in [("a", "S2-A"), ("b", "S2-B"), ("c", "S2-C"), ("d", "S2-D")] {
        let expected = workspace_root.join(name);
        let expected = expected.to_str().unwrap();
        let thread = records
            .iter()
            .map(|(_, frame)| frame)
            .find(|frame| {
                frame["method"] == "thread/start"
                    && frame.pointer("/params/cwd").and_then(Value::as_str) == Some(expected)
            })
            .unwrap_or_else(|| panic!("missing exact canonical thread/start cwd for {tag}"));
        assert_eq!(thread["params"]["ephemeral"], true);
        assert!(thread["params"].get("model").is_none());
        assert_eq!(
            thread["params"]["approvalPolicy"],
            if name == "c" { "on-request" } else { "never" }
        );
        assert_eq!(
            thread["params"]["sandbox"],
            if name == "c" {
                "read-only"
            } else {
                "workspace-write"
            }
        );

        let turn = records
            .iter()
            .map(|(_, frame)| frame)
            .find(|frame| {
                frame["method"] == "turn/start"
                    && frame
                        .pointer("/params/input/0/text")
                        .and_then(Value::as_str)
                        .is_some_and(|prompt| prompt.contains(tag))
            })
            .unwrap_or_else(|| panic!("missing turn/start for {tag}"));
        assert_eq!(
            turn.pointer("/params/cwd").and_then(Value::as_str),
            Some(expected)
        );
    }
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
        assert!(prompt.contains("final response body only"));
        assert!(prompt.contains("fixed 64-character ASCII seed"));
        assert!(prompt.contains("Use no tools, no commands, no files"));
        assert!(!prompt.contains("COMMAND_BEGIN"));
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
    assert_eq!(command.matches('&').count(), 2);
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
    let workspace = std::path::PathBuf::from(
        turn_start
            .pointer("/params/cwd")
            .and_then(Value::as_str)
            .unwrap(),
    );
    assert!(
        !workspace.exists(),
        "the private neutral run root must be removed after validation"
    );
    for artifact in ["evidence.json", "validation-report.json", "summary.txt"] {
        let sanitized = std::fs::read_to_string(output.join(artifact)).unwrap();
        assert!(!sanitized.contains("S2_APPROVED"));
        assert!(!sanitized.contains(".codex-s2-approval-marker"));
        assert!(!sanitized.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKL"));
    }
    #[cfg(windows)]
    {
        let executable = command
            .strip_prefix("& \"")
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
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: Vec::new(),
    })
    .unwrap();
    assert!(outcome.report.valid);
    std::fs::remove_dir_all(output).unwrap();

    for mode in [
        "wrapper-command-mutated",
        "wrapper-extra-action",
        "wrapper-wrong-pwsh",
        "wrapper-outer-backslashes",
        "wrapper-path-separator-single",
        "wrapper-path-separator-triple",
        "wrapper-extra-argv",
        "wrapper-outer-backslash-0",
        "wrapper-outer-backslash-1",
        "wrapper-outer-backslash-2",
        "wrapper-outer-backslash-3",
        "wrapper-inner-backslash-missing",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 10_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(
            capture.contains(r#"\"decision\":\"cancel\""#),
            "{mode} did not cancel"
        );
        assert!(
            !capture.contains(r#"\"decision\":\"accept\""#),
            "{mode} unexpectedly accepted"
        );
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
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(5),
        global_timeout: Duration::from_millis(1200),
        test_child_env: Vec::new(),
    });
    assert!(result.is_err());
    assert!(start.elapsed() < Duration::from_millis(1800));
    std::fs::remove_dir_all(output).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn marker_helper_uses_global_deadline_and_reaps_its_contained_tree() {
    let root = temp_output("marker-helper-timeout");
    for attempt in 0..3 {
        let workspace = root.join(format!("workspace-{attempt}"));
        std::fs::create_dir_all(&workspace).unwrap();
        let started = std::time::Instant::now();
        let result = run_marker_helper_process_for_test(
            &workspace,
            OsString::from(env!("CARGO_BIN_EXE_fake-app-server")),
            vec![
                OsString::from("--marker-helper-fixture"),
                OsString::from("stall"),
            ],
            "absent",
            Duration::from_secs(10),
            Duration::from_secs(3),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(6));
        let pids = std::fs::read_to_string(workspace.join(".codex-s2-helper-test-pids"))
            .unwrap()
            .lines()
            .map(|line| line.parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(pids.len(), 2);
        assert!(
            process_has_exited(pids[0]),
            "the direct helper child must be reaped, not left as a zombie"
        );
        assert!(descendant_has_terminated(pids[1]));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(debug_assertions)]
fn marker_helper_rejects_malformed_and_oversize_output() {
    for mode in ["malformed", "oversize"] {
        let workspace = temp_output(&format!("marker-helper-{mode}"));
        let result = run_marker_helper_process_for_test(
            &workspace,
            OsString::from(env!("CARGO_BIN_EXE_fake-app-server")),
            vec![
                OsString::from("--marker-helper-fixture"),
                OsString::from(mode),
            ],
            "verify",
            Duration::from_secs(3),
            Duration::from_secs(3),
        );
        assert!(result.is_err(), "{mode} unexpectedly passed");
        std::fs::remove_dir_all(workspace).unwrap();
    }
}

#[test]
fn marker_helper_subcommand_is_hidden_and_accepts_only_fixed_operations() {
    let help = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("marker-helper"));

    let workspace = temp_output("marker-helper-cli");
    let mut absent = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .arg("__marker-helper")
        .current_dir(&workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    absent.stdin.take().unwrap().write_all(b"absent\n").unwrap();
    let absent = absent.wait_with_output().unwrap();
    assert!(absent.status.success());
    assert_eq!(absent.stdout, b"ABSENT\n");
    assert!(absent.stderr.is_empty());

    std::fs::write(workspace.join(".codex-s2-approval-marker"), b"S2_APPROVED").unwrap();
    let mut verify = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .arg("__marker-helper")
        .current_dir(&workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    verify.stdin.take().unwrap().write_all(b"verify\n").unwrap();
    let verify = verify.wait_with_output().unwrap();
    assert!(verify.status.success());
    assert_eq!(verify.stdout, b"VERIFIED\n");
    assert!(verify.stderr.is_empty());

    let invalid = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .arg("__marker-helper")
        .current_dir(&workspace)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.take().unwrap().write_all(b"read arbitrary\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(
        !String::from_utf8_lossy(&invalid.stderr).contains(workspace.to_string_lossy().as_ref())
    );
    std::fs::remove_dir_all(workspace).unwrap();
}

#[test]
#[cfg(all(debug_assertions, target_os = "linux"))]
fn linux_helper_survives_replacement_of_the_original_runner_path() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_output("linux-helper-image-replaced");
    let runner = root.join("capture-runner");
    let output = root.join("output");
    std::fs::copy(env!("CARGO_BIN_EXE_codex-app-server-capture"), &runner).unwrap();
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = Command::new(&runner)
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args([
            "--executable",
            env!("CARGO_BIN_EXE_fake-app-server"),
            "--scenario-timeout-ms",
            "10000",
            "--global-timeout-ms",
            "60000",
        ])
        .env("S2_FAKE_MODE", "replace-runner-image")
        .env("S2_FAKE_RUNNER_PATH", &runner)
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "production thresholds must remain strict"
    );
    let evidence: Value =
        serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
    assert_eq!(evidence["protocol_errors"], 0);
    assert_eq!(evidence["scenarios"][2]["turn_completed"], true);
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    assert!(capture.contains(r#"\"decision\":\"accept\""#));
    std::fs::remove_dir_all(root).unwrap();
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

#[cfg(unix)]
fn process_has_exited(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    false
}

#[cfg(all(debug_assertions, windows))]
fn descendant_has_terminated(pid: u32) -> bool {
    process_has_exited(pid)
}

#[cfg(all(debug_assertions, unix))]
fn descendant_has_terminated(pid: u32) -> bool {
    if process_has_exited(pid) {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            return stat
                .rsplit_once(") ")
                .and_then(|(_, fields)| fields.chars().next())
                == Some('Z');
        }
    }
    false
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
            "10000",
            "--global-timeout-ms",
            "60000",
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
fn explicit_trusted_wrapper_accepts_only_its_exact_canonical_command() {
    for mode in ["wrapper-approval", "wrapper-command-mutated"] {
        let root = temp_output(mode);
        let output = root.join("output");
        let (explicit, path) = path_with_planted_pwsh(&root);
        let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
            .args(["run-s2", "--output-dir"])
            .arg(&output)
            .args(["--executable", env!("CARGO_BIN_EXE_fake-app-server")])
            .arg("--trusted-approval-wrapper")
            .arg(&explicit)
            .args([
                "--scenario-timeout-ms",
                "10000",
                "--global-timeout-ms",
                "60000",
            ])
            .env("PATH", path)
            .env("S2_FAKE_MODE", mode)
            .output()
            .unwrap();
        assert!(
            !result.status.success(),
            "production thresholds must stay strict"
        );
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        if mode == "wrapper-approval" {
            assert!(capture.contains(r#"\"decision\":\"accept\""#));
            let sanitized = ["evidence.json", "validation-report.json", "summary.txt"]
                .into_iter()
                .map(|name| std::fs::read_to_string(output.join(name)).unwrap())
                .collect::<String>();
            assert!(!sanitized.contains(explicit.to_string_lossy().as_ref()));
        } else {
            assert!(capture.contains(r#"\"decision\":\"cancel\""#));
            assert!(!capture.contains(r#"\"decision\":\"accept\""#));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    let root = temp_output("different-explicit-wrapper");
    let output = root.join("output");
    let (_request_wrapper, path) = path_with_planted_pwsh(&root);
    let other_dir = root.join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let explicit = other_dir.join("pwsh.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-app-server"), &explicit).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args(["--executable", env!("CARGO_BIN_EXE_fake-app-server")])
        .arg("--trusted-approval-wrapper")
        .arg(&explicit)
        .args([
            "--scenario-timeout-ms",
            "10000",
            "--global-timeout-ms",
            "60000",
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
fn explicit_trusted_wrapper_validation_is_generic_and_precedes_capture() {
    let root = temp_output("invalid-explicit-wrapper");
    let absent = root.join("absent").join("pwsh.exe");
    let non_file = root.join("directory-pwsh.exe");
    std::fs::create_dir_all(&non_file).unwrap();
    let wrong_name = root.join("wrong.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-app-server"), &wrong_name).unwrap();
    let unsafe_dir = root.join("unsafe&wrapper");
    std::fs::create_dir_all(&unsafe_dir).unwrap();
    let unsafe_path = unsafe_dir.join("pwsh.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_fake-app-server"), &unsafe_path).unwrap();

    for (index, candidate) in [absent, non_file, wrong_name, unsafe_path]
        .into_iter()
        .enumerate()
    {
        let output = root.join(format!("output-{index}"));
        let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
            .args(["run-s2", "--output-dir"])
            .arg(&output)
            .args(["--executable", env!("CARGO_BIN_EXE_fake-app-server")])
            .arg("--trusted-approval-wrapper")
            .arg(&candidate)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!output.join("capture.jsonl").exists());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("trusted approval wrapper validation failed"));
        assert!(!stderr.contains(candidate.to_string_lossy().as_ref()));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn explicit_trusted_wrapper_handle_blocks_write_for_the_entire_run() {
    let root = temp_output("wrapper-write-attempt");
    let output = root.join("output");
    let result_marker = root.join("write-result.txt");
    let (explicit, path) = path_with_planted_pwsh(&root);
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args(["--executable", env!("CARGO_BIN_EXE_fake-app-server")])
        .arg("--trusted-approval-wrapper")
        .arg(&explicit)
        .args([
            "--scenario-timeout-ms",
            "10000",
            "--global-timeout-ms",
            "60000",
        ])
        .env("PATH", path)
        .env("S2_FAKE_MODE", "wrapper-write-attempt")
        .env("S2_FAKE_WRAPPER_TARGET", &explicit)
        .env("S2_FAKE_WRAPPER_RESULT", &result_marker)
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "production thresholds must stay strict"
    );
    assert_eq!(std::fs::read_to_string(result_marker).unwrap(), "blocked");
    let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
    assert!(capture.contains(r#"\"decision\":\"accept\""#));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(windows)]
fn explicit_trusted_wrapper_rejects_a_preexisting_writer_before_capture() {
    let root = temp_output("wrapper-existing-writer");
    let output = root.join("output");
    let (explicit, _path) = path_with_planted_pwsh(&root);
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&explicit)
        .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_codex-app-server-capture"))
        .args(["run-s2", "--output-dir"])
        .arg(&output)
        .args(["--executable", env!("CARGO_BIN_EXE_fake-app-server")])
        .arg("--trusted-approval-wrapper")
        .arg(&explicit)
        .args([
            "--scenario-timeout-ms",
            "10000",
            "--global-timeout-ms",
            "60000",
        ])
        .output()
        .unwrap();
    drop(writer);
    assert!(!result.status.success());
    assert!(!output.join("capture.jsonl").exists());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("trusted approval wrapper validation failed"));
    assert!(!stderr.contains(explicit.to_string_lossy().as_ref()));
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
            "10000",
            "--global-timeout-ms",
            "60000",
        ])
        .env("PATH", isolated_path)
        .env("S2_TEST_VERSION_PREFLIGHT_TIMEOUT_MS", "15000")
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
    assert!(command_line.contains("app-server --strict-config --disable hooks"));
    assert!(command_line.contains("-c notify=[]"));
    assert!(command_line.contains("-c skills.include_instructions=false"));
    assert!(command_line.contains("-c otel.metrics_exporter='none' --stdio"));
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
                "2500",
                "--global-timeout-ms",
                "5000",
            ])
            .env("S2_FAKE_MODE", mode)
            .env("S2_FAKE_MARKER", &marker)
            .env("S2_FAKE_CHILD_READY_DELAY_MS", "1200")
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
        "direct-extra-action",
        "direct-mismatched-action",
        "direct-malformed-action",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 10_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
        assert!(capture.contains(r#"\"decision\":\"cancel\""#));
        assert!(!capture.contains(r#"\"decision\":\"accept\""#));
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn scenario_c_requires_exact_marker_execution_evidence() {
    for mode in [
        "accepted-no-marker",
        "wrong-marker",
        "preexisting-marker",
        "approval-preseed-marker",
    ] {
        let output = temp_output(mode);
        let result = run_fake(mode, &output, 10_000);
        assert!(!result.status.success(), "{mode} unexpectedly passed");
        let evidence: Value =
            serde_json::from_slice(&std::fs::read(output.join("evidence.json")).unwrap()).unwrap();
        assert_eq!(
            evidence["protocol_errors"], 1,
            "{mode} did not fail as a protocol error"
        );
        assert_eq!(evidence["scenarios"][2]["turn_completed"], false);
        if mode == "approval-preseed-marker" {
            let capture = std::fs::read_to_string(output.join("capture.jsonl")).unwrap();
            assert!(capture.contains(r#"\"decision\":\"cancel\""#));
            assert!(!capture.contains(r#"\"decision\":\"accept\""#));
        }
        std::fs::remove_dir_all(output).unwrap();
    }
}

#[test]
fn run_s2_rejects_approval_from_in_workspace_child_cwd() {
    let output = temp_output("child-approval");
    let result = run_fake("child-approval", &output, 10_000);
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
        trusted_approval_wrapper: None,
        model: None,
        scenario_timeout: Duration::from_secs(10),
        global_timeout: Duration::from_secs(60),
        test_child_env: Vec::new(),
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
