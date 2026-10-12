//! CLI contract tests for the benchmark command surface.
//!
//! Every test in this file compiles and runs under the default feature set
//! (`product-backend`), because the local-only GAIA operations they exercise
//! (fetch/verify/score) share one unconditional implementation in `lib.rs`
//! regardless of the feature. The single exception is
//! `gaia_run_requires_product_backend_without_exposing_error_chains`, which
//! asserts the no-default-features refusal (`product_backend_not_enabled`)
//! and therefore stays behind `#[cfg(not(feature = "product-backend"))]`;
//! that configuration is compile-checked by the `cli-lint` featureless step
//! in `.github/workflows/pr-check.yml` (which builds `--all-targets`), but
//! no CI leg RUNS it — the refusal itself can only be exercised by a local
//! `cargo test --no-default-features` (disclosed in docs/pinvou-cli.md,
//! Known limitations).

mod common;

use adapter_gaia::{GAIA_LEVEL, GAIA_SPLIT, GaiaAdapter};
use benchmark_core::{BenchmarkAdapter, ModelIdentity, RunManifest, RunStore, Split, ToolPolicyId};
use pinvou_cli::{
    BenchmarkAvailability, BenchmarkCommand, CliCommand, ExitCode, OutputMode, benchmark_registry,
    parse_args, render_list,
};
use std::path::PathBuf;

use common::{HomeGuard, Isolation, RestoreCwd, env_lock, execute_error, usage_error};

/// The canonical manifest every store-backed score test writes through
/// `RunStore`: the officially pinned level 1, split, and tool policy, with an
/// arbitrary model identity (no assertion reads the identity back).
fn gaia_manifest(run_id: &str) -> RunManifest {
    RunManifest::new(
        run_id,
        GaiaAdapter::new().descriptor(),
        Split::new(GAIA_SPLIT),
        ModelIdentity::new("arbitrary-provider", "arbitrary-model").unwrap(),
        ToolPolicyId::new("pinvou-gaia-public-web/v1"),
        1,
    )
    .unwrap()
}

#[test]
fn gaia_score_rejects_every_mutated_manifest_contract_dimension() {
    let _env_guard = env_lock();
    // The throwaway PINVOU3_HOME is a HomeGuard: its Drop restores the
    // variable and removes the tree on every exit path, so a failing
    // assertion can no longer leak the fixture (the manual remove_dir_all
    // epilogue this replaces was exactly the cleanup a panic skipped).
    let home = HomeGuard::new("gaia", "manifest", Isolation::ProductHome);
    let root = home.path();
    assert_eq!(GAIA_LEVEL, 1);
    // Round-47 review: the constant alone is only half the pin — the level
    // is an external contract documented in the gaia doc's dataset table. A
    // `GAIA_LEVEL` bump without the matching dataset/scorer/doc change must
    // fail here instead of shipping a scorer that scores a level the doc
    // does not support.
    assert!(
        include_str!("../../../../docs/gaia-benchmark.md").contains("| 数据集 level | `1` |"),
        "docs/gaia-benchmark.md must still pin dataset level 1; if the level \
         moved, the dataset, scorer and doc must move together"
    );
    let mutations = [
        ("run_id", serde_json::json!("different-run-id")),
        // Schema 1 (legacy) stays scoreable on purpose; only an unknown
        // schema must be rejected by the scoring gate.
        ("schema_version", serde_json::json!(99)),
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
        let manifest = gaia_manifest(&run_id);
        let store = RunStore::create(root, &manifest).unwrap();
        let mut stored = serde_json::to_value(&manifest).unwrap();
        stored[field] = replacement;
        std::fs::write(store.manifest_path(), serde_json::to_vec(&stored).unwrap()).unwrap();

        let error = execute_error(&["pinvou", "benchmark", "score", "gaia", "--run-id", &run_id]);
        assert_eq!(error.to_string(), "gaia_run_manifest_mismatch", "{field}");
    }
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
fn gaia_fetch_from_non_repository_home_does_not_require_git_metadata() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("gaia", "non-repo-home", Isolation::ProductHome);
    let _restore_cwd = RestoreCwd::new();
    std::env::set_current_dir(home.path()).unwrap();

    let error = execute_error(&[
        "pinvou",
        "benchmark",
        "fetch",
        "gaia",
        "--source",
        "missing-snapshot",
    ]);
    // Round-41 review: stronger than the old `!= "gaia_worktree_unavailable"`
    // one-liner — the round-38 regression this test guards made fetch demand
    // git metadata on a non-repository home before ever looking at the
    // source. The contract: the missing SNAPSHOT is the cause (a gaia-family
    // error), and no git/worktree wording surfaces at all.
    let message = error.to_string();
    assert!(
        !message.contains("worktree") && !message.contains("git"),
        "non-repository home must not surface a git/worktree error: {message}"
    );
    assert!(
        message.starts_with("gaia_"),
        "expected a gaia-family error for a missing snapshot, got: {message}"
    );
}

#[test]
fn gaia_verify_keeps_raw_snapshot_validation_separate_from_the_ready_gate() {
    let _env_guard = env_lock();
    let home = HomeGuard::new("gaia", "ready-gate", Isolation::ProductHome);
    let source = home.path().join("arbitrary-source");
    std::fs::create_dir_all(&source).unwrap();

    let error = execute_error(&[
        "pinvou",
        "benchmark",
        "verify",
        "gaia",
        "--source",
        source.to_str().unwrap(),
    ]);
    assert_eq!(error.to_string(), "gaia_verify_failed");
    assert!(!error.to_string().contains(source.to_str().unwrap()));
}

/// Extracts one `lib.rs` helper's body between its `fn` signature prefix and
/// the landmark that opens the next function, so the boundary pins below
/// panic (instead of silently pinning nothing) when a rename or reformat
/// moves either landmark.
fn lib_rs_body(owner: &str, start: &str, end: &str) -> String {
    include_str!("../src/lib.rs")
        .split_once(start)
        .and_then(|(_, tail)| tail.split_once(end))
        .map(|(body, _)| body.to_owned())
        .unwrap_or_else(|| panic!("{owner} must remain a distinct narrow boundary"))
}

#[test]
fn gaia_official_consumers_take_the_digest_bound_dataset_from_the_ready_gate() {
    let helper = lib_rs_body(
        "official GAIA loader",
        "fn open_official_gaia_dataset",
        "\nfn verify_gaia",
    );

    assert!(helper.contains(".verify_offline(snapshot_root)"));
    assert!(helper.contains("acquisition.into_dataset()"));
    assert!(!helper.contains("GaiaDataset::verify"));
}

#[test]
fn gaia_raw_verify_uses_the_trusted_source_boundary() {
    let helper = lib_rs_body(
        "raw GAIA verifier",
        "fn verify_gaia(source",
        "\nfn verify_gaia_with",
    );

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
        usage_error(&arguments);
    }
}

// Kept feature-gated on purpose: this test asserts the no-default-features
// refusal (`product_backend_not_enabled`) of `benchmark run gaia`, which can
// never fire when the default `product-backend` feature is compiled in. CI
// compile-checks this configuration (`cli-lint` builds `--no-default-features
// --all-targets`, see docs/pinvou-cli.md, Known limitations) but never RUNS
// it, so the test stays dead in CI rather than failing everywhere;
// do NOT remove this `cfg` to "revive" it.
#[cfg(not(feature = "product-backend"))]
#[test]
fn gaia_run_requires_product_backend_without_exposing_error_chains() {
    let error = execute_error(&[
        "pinvou",
        "benchmark",
        "run",
        "gaia",
        "--split",
        "validation",
        "--level",
        "1",
    ]);
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
        let error = execute_error(&["pinvou", "benchmark", "run", id]);
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

/// Round-41 review: exit code 2 is the contract scripts branch on, and it is
/// produced by `main`'s process-exit path (`error.exit_code().as_i32()` and
/// the decode-arguments early exit) — a layer the library-level
/// `invalid_usage_maps_to_exit_code_two` never executes. This pins the real
/// binary's process exit code for both refusal classes. Neither path touches
/// the store, so no `PINVOU3_HOME` fixture is needed.
#[test]
fn usage_failures_exit_two_through_the_real_binary() {
    // Both refusal classes exit 2: a plain unknown family token, and (unix
    // only) a non-UTF-8 argv token the decode step must refuse the same way.
    let non_utf8_argument: Option<std::ffi::OsString> = {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Some(std::ffi::OsStr::from_bytes(b"\xff\xfe").to_os_string())
        }
        #[cfg(not(unix))]
        {
            None
        }
    };
    let mut first_arguments: Vec<std::ffi::OsString> = vec!["__no_such_family__".into()];
    first_arguments.extend(non_utf8_argument);
    for argument in first_arguments {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
            .arg(argument)
            .output()
            .expect("spawn the pinvou binary");
        assert_eq!(
            output.status.code(),
            Some(2),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Round-50 review: a panic escaping `execute` must not bypass the
/// supervisor on the way out — `main` catches the unwind, still lets the
/// interrupt-cleanup park run, and keeps the exit contract (101, outside
/// 0/1/2, with the hook's one-line internal-error report). The
/// `PINVOU_CLI_TEST_FORCE_PANIC` seam in main.rs makes the panic path
/// hermetically reachable; a regression to plain unwinding or to an early
/// `std::process::exit` ahead of the park changes this contract's shape.
/// The panic path touches no store, so no `PINVOU3_HOME` fixture is needed.
#[test]
fn a_panicking_run_still_parks_and_exits_101_through_the_real_binary() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_pinvou"))
        .env("PINVOU_CLI_TEST_FORCE_PANIC", "1")
        .output()
        .expect("spawn the pinvou binary");
    assert_eq!(
        output.status.code(),
        Some(101),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("internal error (panic"),
        "the hook's one-line report must stay the only panic output: {stderr}"
    );
    assert!(
        stderr.contains("this is a bug"),
        "the report must name the failure as an internal bug: {stderr}"
    );
}

#[test]
fn unrecognized_output_value_falls_through_to_usage_error() {
    // Outside `benchmark submission gaia` (which consumes `--output <file>` as
    // a legacy alias of `--destination`), an unrecognized `--output` value
    // stays in argv — so the token lands in the FAMILY position and the
    // round-27 flag-position hint names it and states the placement rule,
    // instead of the bare usage line.
    let message = usage_error(&["pinvou", "--output", "yaml", "benchmark", "list"]);
    assert!(message.contains("unknown family --output"), "{message}");
    assert!(
        message.contains("before the family or after the subcommand"),
        "{message}"
    );
    assert!(
        message.contains("usage: pinvou benchmark <command>"),
        "the usage line still follows the hint: {message}"
    );

    // A NON-flag unknown token keeps the exact bare usage line (the drift
    // pin lives on the constant; this pins the behavior).
    let generic = parse_args(["pinvou", "bogus", "list"]).unwrap_err();
    assert_eq!(generic.exit_code(), ExitCode::Usage);
    assert_eq!(
        generic.to_string(),
        "usage: pinvou benchmark <command> | pinvou agent run | pinvou \
         sessions|models|settings|memory|knowledge|scheduled|plugins|connectors|personas|\
         projects|code|files|voice|deps|feedback|monitor|artifacts <command> | pinvou \
         --help|--version|version"
    );
}

#[test]
fn output_without_value_points_at_destination() {
    // A valueless `--output` fails fast with a usage error that names the
    // flag for file submissions (the gaia `--output <file>` legacy alias
    // only fires when a value is present).
    let message = usage_error(&["pinvou", "--output"]);
    assert_eq!(
        message,
        "--output requires human or json (subcommands that export to a file take \
         `--output PATH` instead — see their usage line)"
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
