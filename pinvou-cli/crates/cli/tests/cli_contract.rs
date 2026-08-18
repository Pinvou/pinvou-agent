use pinvou_cli::{
    BenchmarkAvailability, BenchmarkCommand, CliCommand, ExitCode, OutputMode, benchmark_registry,
    execute, parse_args, render_list,
};
use std::path::PathBuf;
#[cfg(not(feature = "product-backend"))]
use std::sync::Mutex;

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
#[cfg(not(feature = "product-backend"))]
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(not(feature = "product-backend"))]
#[test]
fn gaia_score_rejects_every_mutated_manifest_contract_dimension() {
    use adapter_gaia::{GAIA_LEVEL, GAIA_SPLIT, GaiaAdapter};
    use benchmark_core::{
        BenchmarkAdapter, ModelIdentity, RunManifest, RunStore, Split, ToolPolicyId,
    };

    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!(
        "pinvou-cli-gaia-manifest-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let previous = std::env::var_os("PINVOU3_HOME");
    unsafe { std::env::set_var("PINVOU3_HOME", &root) };
    let adapter = GaiaAdapter::new();
    let expected = RunManifest::new(
        "gaia-manifest-probe",
        adapter.descriptor(),
        Split::new(GAIA_SPLIT),
        ModelIdentity::new("arbitrary-provider", "arbitrary-model").unwrap(),
        ToolPolicyId::new("pinvou-gaia-public-web/v1"),
        1,
    )
    .unwrap();
    assert_eq!(GAIA_LEVEL, 1);
    let mutations = [
        ("run_id", serde_json::json!("different-run-id")),
        ("schema_version", serde_json::json!(2)),
        ("concurrency", serde_json::json!(2)),
        ("pass", serde_json::json!(2)),
        (
            "adapter_version",
            serde_json::json!("pinvou-gaia-adapter/v2"),
        ),
        ("dataset_revision", serde_json::json!("changed-dataset")),
        ("scorer_revision", serde_json::json!("changed-scorer")),
        ("split", serde_json::json!("test")),
        ("tool_policy", serde_json::json!("changed-policy/v1")),
    ];
    for (index, (field, replacement)) in mutations.into_iter().enumerate() {
        let run_id = format!("gaia-manifest-{index}");
        let manifest = RunManifest::new(
            run_id.as_str(),
            adapter.descriptor(),
            Split::new(GAIA_SPLIT),
            ModelIdentity::new("arbitrary-provider", "arbitrary-model").unwrap(),
            ToolPolicyId::new("pinvou-gaia-public-web/v1"),
            1,
        )
        .unwrap();
        let store = RunStore::create(&root, &manifest).unwrap();
        let mut stored = serde_json::to_value(&expected).unwrap();
        stored["run_id"] = serde_json::json!(run_id);
        stored[field] = replacement;
        std::fs::write(store.manifest_path(), serde_json::to_vec(&stored).unwrap()).unwrap();

        let parsed =
            parse_args(["pinvou", "benchmark", "score", "gaia", "--run-id", &run_id]).unwrap();
        let error = execute(parsed).expect_err("mutated manifest must be rejected before scoring");
        assert_eq!(error.to_string(), "gaia_run_manifest_mismatch", "{field}");
    }
    match previous {
        Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn gaia_parser_exposes_only_the_pinned_official_level_one_workflow() {
    let cases = [
        (
            vec![
                "pinvou",
                "benchmark",
                "fetch",
                "gaia",
                "--token-env",
                "HF_TOKEN",
            ],
            BenchmarkCommand::FetchGaia {
                token_env: Some("HF_TOKEN".into()),
                source: None,
            },
        ),
        (
            vec![
                "pinvou",
                "benchmark",
                "fetch",
                "gaia",
                "--source",
                "snapshot",
            ],
            BenchmarkCommand::FetchGaia {
                token_env: None,
                source: Some(PathBuf::from("snapshot")),
            },
        ),
        (
            vec![
                "pinvou",
                "benchmark",
                "verify",
                "gaia",
                "--source",
                "snapshot",
            ],
            BenchmarkCommand::VerifyGaia {
                source: PathBuf::from("snapshot"),
            },
        ),
        (
            vec![
                "pinvou",
                "benchmark",
                "run",
                "gaia",
                "--split",
                "validation",
                "--level",
                "1",
            ],
            BenchmarkCommand::RunGaia {
                split: "validation".into(),
                level: 1,
            },
        ),
        (
            vec!["pinvou", "benchmark", "score", "gaia", "--run-id", "run-1"],
            BenchmarkCommand::ScoreGaia {
                run_id: "run-1".into(),
            },
        ),
        (
            vec![
                "pinvou",
                "benchmark",
                "submission",
                "gaia",
                "--run-id",
                "run-1",
                "--output",
                "submission.jsonl",
            ],
            BenchmarkCommand::SubmissionGaia {
                run_id: "run-1".into(),
                output: PathBuf::from("submission.jsonl"),
            },
        ),
    ];

    for (arguments, expected) in cases {
        assert_eq!(
            parse_args(arguments).expect("valid GAIA command").command(),
            &CliCommand::Benchmark(expected)
        );
    }
}

#[test]
fn gaia_output_mode_remains_global_without_stealing_submission_destination() {
    for arguments in [
        vec!["pinvou", "benchmark", "list", "--output", "json"],
        vec!["pinvou", "benchmark", "run", "smoke", "--output", "json"],
        vec!["pinvou", "benchmark", "status", "run-1", "--output", "json"],
        vec!["pinvou", "benchmark", "resume", "run-1", "--output", "json"],
        vec!["pinvou", "benchmark", "report", "run-1", "--output", "json"],
    ] {
        assert_eq!(parse_args(arguments).unwrap().output(), OutputMode::Json);
    }

    let submission = parse_args([
        "pinvou",
        "benchmark",
        "submission",
        "gaia",
        "--run-id",
        "run-1",
        "--destination",
        "submission.jsonl",
        "--output",
        "json",
    ])
    .unwrap();
    assert_eq!(submission.output(), OutputMode::Json);
    assert_eq!(
        submission.command(),
        &CliCommand::Benchmark(BenchmarkCommand::SubmissionGaia {
            run_id: "run-1".into(),
            output: PathBuf::from("submission.jsonl"),
        })
    );
}

#[cfg(not(feature = "product-backend"))]
#[test]
fn gaia_fetch_from_non_repository_home_does_not_require_git_metadata() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pinvou-cli-home-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&home).unwrap();
    let previous_dir = std::env::current_dir().unwrap();
    let previous_home = std::env::var_os("PINVOU3_HOME");
    std::env::set_current_dir(&home).unwrap();
    unsafe { std::env::set_var("PINVOU3_HOME", &home) };

    let parsed = parse_args([
        "pinvou",
        "benchmark",
        "fetch",
        "gaia",
        "--source",
        "missing-snapshot",
    ])
    .unwrap();
    let error = execute(parsed).unwrap_err();
    assert_ne!(error.to_string(), "gaia_worktree_unavailable");

    std::env::set_current_dir(previous_dir).unwrap();
    match previous_home {
        Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(not(feature = "product-backend"))]
#[test]
fn gaia_verify_keeps_raw_snapshot_validation_separate_from_the_ready_gate() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!(
        "pinvou-cli-gaia-ready-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let source = home.join("arbitrary-source");
    std::fs::create_dir_all(&source).unwrap();
    let previous = std::env::var_os("PINVOU3_HOME");
    unsafe { std::env::set_var("PINVOU3_HOME", &home) };

    let parsed = parse_args([
        "pinvou",
        "benchmark",
        "verify",
        "gaia",
        "--source",
        source.to_str().unwrap(),
    ])
    .unwrap();
    let error = execute(parsed).expect_err("incomplete raw snapshot must fail dataset validation");
    assert_eq!(error.to_string(), "gaia_verify_failed");
    assert!(!error.to_string().contains(source.to_str().unwrap()));

    match previous {
        Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
        None => unsafe { std::env::remove_var("PINVOU3_HOME") },
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn gaia_official_consumers_take_the_digest_bound_dataset_from_the_ready_gate() {
    let source = include_str!("../src/lib.rs");
    let helper = source
        .split_once("fn open_official_gaia_dataset")
        .and_then(|(_, tail)| tail.split_once("\nfn verify_gaia"))
        .map(|(body, _)| body)
        .expect("official GAIA loader must remain a distinct narrow boundary");

    assert!(helper.contains(".verify_offline(snapshot_root)"));
    assert!(helper.contains("acquisition.into_dataset()"));
    assert!(!helper.contains("GaiaDataset::verify"));
}

#[test]
fn gaia_raw_verify_uses_the_trusted_source_boundary() {
    let source = include_str!("../src/lib.rs");
    let helper = source
        .split_once("fn verify_gaia(source")
        .and_then(|(_, tail)| tail.split_once("\nfn verify_gaia_with"))
        .map(|(body, _)| body)
        .expect("raw GAIA verifier must remain a distinct narrow boundary");

    assert!(helper.contains(".verify_source(source)"));
    assert!(!helper.contains("GaiaDataset::verify"));
}

#[test]
fn gaia_parser_rejects_unpinned_or_noncomparable_modes() {
    let invalid = [
        vec!["pinvou", "benchmark", "fetch", "gaia", "--token", "secret"],
        vec!["pinvou", "benchmark", "fetch", "gaia", "--revision", "main"],
        vec![
            "pinvou",
            "benchmark",
            "run",
            "gaia",
            "--split",
            "test",
            "--level",
            "1",
        ],
        vec![
            "pinvou",
            "benchmark",
            "run",
            "gaia",
            "--split",
            "validation",
            "--level",
            "2",
        ],
        vec![
            "pinvou",
            "benchmark",
            "run",
            "gaia",
            "--split",
            "validation",
            "--level",
            "1",
            "--task-id",
            "one",
        ],
        vec![
            "pinvou",
            "benchmark",
            "submission",
            "gaia",
            "--run-id",
            "run-1",
        ],
    ];
    for arguments in invalid {
        let error = parse_args(arguments).expect_err("unsafe GAIA mode must be rejected");
        assert_eq!(error.exit_code(), ExitCode::Usage);
    }
}

#[test]
fn gaia_registry_is_available_while_other_official_adapters_remain_planned() {
    let registry = benchmark_registry();
    let gaia = registry.iter().find(|spec| spec.id() == "gaia").unwrap();
    assert_eq!(gaia.availability(), BenchmarkAvailability::Available);
    assert_eq!(gaia.score_kind(), "official_compatible_local");
    for id in ["bfcl", "workbuddy"] {
        let spec = registry.iter().find(|spec| spec.id() == id).unwrap();
        assert_eq!(spec.availability(), BenchmarkAvailability::Planned);
        assert_eq!(spec.command_error(), "benchmark_not_available");
    }
}

#[cfg(not(feature = "product-backend"))]
#[test]
fn gaia_run_requires_product_backend_without_exposing_error_chains() {
    let parsed = parse_args([
        "pinvou",
        "benchmark",
        "run",
        "gaia",
        "--split",
        "validation",
        "--level",
        "1",
    ])
    .unwrap();
    let error = execute(parsed).expect_err("feature-off run is unavailable");
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert_eq!(error.to_string(), "product_backend_not_enabled");
    assert!(!error.to_string().contains("Caused by"));
}

#[test]
fn registry_is_the_stable_source_for_all_benchmark_commands() {
    let registry = benchmark_registry();
    assert_eq!(
        registry.iter().map(|spec| spec.id()).collect::<Vec<_>>(),
        ["smoke", "gaia", "bfcl", "workbuddy"]
    );
    assert_eq!(registry[0].score_kind(), "internal_health");
    assert_eq!(registry[1].availability(), BenchmarkAvailability::Available);
    assert_eq!(registry[1].score_kind(), "official_compatible_local");
    for spec in &registry[2..] {
        assert_eq!(spec.availability(), BenchmarkAvailability::Planned);
        assert_eq!(spec.command_error(), "benchmark_not_available");
    }
}

#[test]
fn registry_dispatches_smoke_and_rejects_planned_benchmarks_consistently() {
    assert_eq!(
        parse_args(["pinvou", "benchmark", "run", "smoke"])
            .expect("registered smoke")
            .command(),
        &CliCommand::Benchmark(BenchmarkCommand::RunSmoke)
    );
    for id in ["bfcl", "workbuddy"] {
        let parsed = parse_args(["pinvou", "benchmark", "run", id])
            .expect("planned benchmark has a stable command representation");
        let error = execute(parsed).expect_err("planned benchmark cannot run");
        assert_eq!(error.exit_code(), ExitCode::Usage);
        assert_eq!(error.to_string(), "benchmark_not_available");
    }
}

#[test]
fn parses_required_benchmark_command_tree() {
    let cases = [
        (vec!["pinvou", "benchmark", "list"], BenchmarkCommand::List),
        (
            vec!["pinvou", "benchmark", "run", "smoke"],
            BenchmarkCommand::RunSmoke,
        ),
        (
            vec!["pinvou", "benchmark", "status", "run-1"],
            BenchmarkCommand::Status("run-1".into()),
        ),
        (
            vec!["pinvou", "benchmark", "resume", "run-1"],
            BenchmarkCommand::Resume("run-1".into()),
        ),
        (
            vec!["pinvou", "benchmark", "report", "run-1"],
            BenchmarkCommand::Report("run-1".into()),
        ),
    ];
    for (arguments, expected) in cases {
        let parsed = parse_args(arguments).expect("valid command");
        assert_eq!(parsed.command(), &CliCommand::Benchmark(expected));
        assert_eq!(parsed.output(), OutputMode::Human);
    }
}

#[test]
fn parses_json_output_and_rejects_non_gaia_workflow_commands() {
    let list =
        parse_args(["pinvou", "--output", "json", "benchmark", "list"]).expect("json output");
    assert_eq!(list.output(), OutputMode::Json);

    for name in ["fetch", "verify", "score", "submission"] {
        assert!(parse_args(["pinvou", "benchmark", name, "smoke"]).is_err());
    }
}

#[test]
fn invalid_usage_maps_to_exit_code_two() {
    let error = parse_args(["pinvou", "benchmark", "run", "unknown"])
        .expect_err("unknown benchmark is rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert_eq!(ExitCode::Success.as_i32(), 0);
    assert_eq!(ExitCode::Failed.as_i32(), 1);
    assert_eq!(ExitCode::Usage.as_i32(), 2);
}

#[test]
fn list_output_is_stable_and_labels_smoke_as_internal_health() {
    let human = render_list(OutputMode::Human);
    assert!(human.contains("smoke"));
    assert!(human.contains("内部健康检查"));

    let json = render_list(OutputMode::Json);
    assert!(json.contains(&format!(
        r#""available":{}"#,
        cfg!(feature = "product-backend")
    )));
    assert!(human.contains("gaia\tavailable"));
    assert!(json.contains(r#"{"id":"gaia","availability":"available""#));
    for id in ["bfcl", "workbuddy"] {
        assert!(human.contains(&format!("{id}\tplanned")));
        assert!(json.contains(&format!(r#"{{"id":"{id}","availability":"planned""#)));
    }
}
