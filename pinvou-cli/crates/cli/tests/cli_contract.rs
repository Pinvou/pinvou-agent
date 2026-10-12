use pinvou_cli::{
    BenchmarkAvailability, BenchmarkCommand, CliCommand, ExitCode, OutputMode, benchmark_registry,
    execute, parse_args, render_list,
};
use std::path::PathBuf;

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
                "--destination",
                "submission.jsonl",
            ],
            BenchmarkCommand::SubmissionGaia {
                run_id: "run-1".into(),
                output: PathBuf::from("submission.jsonl"),
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
fn unrecognized_output_value_falls_through_to_usage_error() {
    // Outside `benchmark submission gaia` (which consumes `--output <file>` as
    // a legacy alias of `--destination`), an unrecognized `--output` value
    // stays in argv and surfaces as the standard usage error.
    let error = parse_args(["pinvou", "--output", "yaml", "benchmark", "list"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert_eq!(
        error.to_string(),
        "usage: pinvou benchmark <command> | pinvou agent run"
    );
}

#[test]
fn output_without_value_points_at_destination() {
    // A valueless `--output` fails fast with a usage error that names the
    // flag for file submissions (the gaia `--output <file>` legacy alias
    // only fires when a value is present).
    let error = parse_args(["pinvou", "--output"]).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert_eq!(
        error.to_string(),
        "--output requires human or json (submission files use --destination)"
    );
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
