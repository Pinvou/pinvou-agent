mod common;

use pinvou_cli::{CliCommand, ExitCode, OutputMode, parse_args};
use std::path::PathBuf;

use common::{EnvGuard, env_lock, usage_error};

#[test]
fn a_flag_in_the_family_position_names_itself_instead_of_bare_usage() {
    // `pinvou --output json sessions list` misplaces a GLOBAL flag; the
    // refusal must say so (round-27 review: the bare usage line gives no
    // hint which token is wrong).
    let error = parse_args(["pinvou", "--instal", "sessions", "list"])
        .expect_err("an unknown flag in the family position must be refused");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let message = error.to_string();
    assert!(
        message.contains("unknown family --instal"),
        "the offending token must be named: {message}"
    );
    assert!(
        message.contains("before the family or after the subcommand"),
        "the placement remedy must be stated: {message}"
    );
}

#[test]
fn the_family_position_error_is_not_prefixed_twice() {
    // Round-28 review: main.rs already prepends `pinvou: ` to every CliError,
    // so a message carrying its own prefix prints
    // `pinvou: pinvou: unknown family …`. argv must include the program slot
    // or parse_args strips it as argv[0] and the flag-shaped arm is never
    // reached (the previous form of this test did exactly that and could
    // not fail while the bug was live).
    let message = usage_error(&["pinvou", "--instal", "sessions"]);
    assert!(
        message.contains("unknown family --instal"),
        "the flag-shaped token must be named by the family-position arm: {message}"
    );
    assert!(
        !message.starts_with("pinvou:"),
        "CliError text must be prefix-free; main.rs adds the prefix when printing: {message}"
    );
}

#[test]
fn a_bare_trailing_output_names_the_missing_value() {
    // Round-28 review: `pinvou benchmark list --output` fell through to the
    // family parser's "unknown benchmark command", which names nothing the
    // user can fix; the trailing position is unambiguously an incomplete
    // global pair, so it answers with the missing-value error.
    let error = parse_args(["pinvou", "benchmark", "list", "--output"])
        .expect_err("a valueless trailing --output must be refused");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    let message = error.to_string();
    assert!(
        message.contains("--output requires human or json"),
        "the missing mode value must be named: {message}"
    );
}

/// The discriminator the exact-match test below asserts on. Exhaustive over
/// `CliCommand`, so adding a family forces this test to decide its token —
/// the union test above cannot notice a family routed into another
/// family's variant.
fn family_token(command: &CliCommand) -> &'static str {
    match command {
        CliCommand::Version => "version",
        CliCommand::Help => "help",
        CliCommand::Benchmark(_) => "benchmark",
        CliCommand::Agent(_) => "agent",
        CliCommand::Sessions(_) => "sessions",
        CliCommand::Models(_) => "models",
        CliCommand::Memory(_) => "memory",
        CliCommand::Knowledge(_) => "knowledge",
        CliCommand::Scheduled(_) => "scheduled",
        CliCommand::Plugins(_) => "plugins",
        CliCommand::Connectors(_) => "connectors",
        CliCommand::Personas(_) => "personas",
        CliCommand::Code(_) => "code",
        CliCommand::Files(_) => "files",
        CliCommand::Voice(_) => "voice",
        CliCommand::Deps(_) => "deps",
        CliCommand::Feedback(_) => "feedback",
        CliCommand::Monitor(_) => "monitor",
        CliCommand::Artifacts(_) => "artifacts",
        CliCommand::Projects(_) => "projects",
    }
}

/// The dispatch contract, strengthened from the union test above: each
/// family token must parse into ITS OWN command variant — not merely into
/// "some family's variant". The union shape let a mis-routed token pass
/// (e.g. the `settings` alias accidentally answering as `Sessions`, or a
/// duplicated match arm in `parse_args` sending two tokens to one family),
/// and reverting the `outcome.is_err()`-style routing in the shared parser
/// would not have been caught by it either. Mutating the shared dispatch
/// (any single family's match arm) fails exactly that family's row here.
///
/// Rows carry an explicit expected family, because two tokens legitimately
/// land in another family ON PURPOSE: `settings` is an alias for the models
/// family (pinned by `settings_alias_routes_into_the_models_family`), and
/// the agent/benchmark rows join so the exact-match contract covers every
/// remaining top-level surface token.
#[test]
fn family_tokens_dispatch_to_their_exact_module() {
    let tokens = [
        ("sessions", "list", "sessions"),
        ("models", "list", "models"),
        ("settings", "get", "models"),
        ("memory", "overview", "memory"),
        ("knowledge", "stats", "knowledge"),
        ("scheduled", "list", "scheduled"),
        ("plugins", "readiness", "plugins"),
        ("connectors", "status", "connectors"),
        ("personas", "list", "personas"),
        ("code", "agents list", "code"),
        ("files", "ingest /tmp/a.md", "files"),
        ("voice", "asr-status", "voice"),
        ("deps", "check", "deps"),
        (
            "feedback",
            "submit --type issue --title t --body-file /tmp/b.md",
            "feedback",
        ),
        ("monitor", "status", "monitor"),
        ("artifacts", "list", "artifacts"),
        ("projects", "list", "projects"),
        ("agent", "run --prompt-file /tmp/prompt.txt", "agent"),
        ("benchmark", "list", "benchmark"),
    ];
    for (token, subcommand, expected) in tokens {
        let mut argv = vec!["pinvou".to_string(), token.to_string()];
        argv.extend(subcommand.split_whitespace().map(str::to_string));
        let parsed = parse_args(&argv)
            .unwrap_or_else(|error| panic!("family {token} did not dispatch: {error}"));
        assert_eq!(
            family_token(parsed.command()),
            expected,
            "{token} must dispatch to {expected}'s own module, not another family's"
        );
    }
}

#[test]
fn settings_alias_routes_into_the_models_family() {
    let parsed = parse_args(["pinvou", "settings", "get"]).unwrap();
    assert!(matches!(parsed.command(), CliCommand::Models(_)));
}

#[test]
fn family_without_subcommand_is_a_usage_error_naming_the_family() {
    // The family parser's own USAGE must answer — the catch-all
    // TOP_LEVEL_USAGE also contains the family token, so a bare
    // `contains(family)` would pass even if the routing regressed to it.
    let message = usage_error(&["pinvou", "sessions"]);
    assert!(
        message.contains("pinvou sessions <list|"),
        "the family usage must name the family's subcommands: {message}"
    );
    let message = usage_error(&["pinvou", "memory"]);
    assert!(
        message.contains("pinvou memory <overview|"),
        "the family usage must name the family's subcommands: {message}"
    );
}

// The former `stub_execution_reports_not_implemented_as_host_failure` test
// was removed when the files/voice/deps/feedback/monitor families were
// implemented: it asserted the temporary `not_implemented_yet` stub error for
// `monitor snapshot`, which now boots the windowless host instead (its real
// behavior is covered by the `#[ignore]` opt-in tests in misc_contract.rs).

#[test]
fn unknown_top_level_command_lists_the_full_surface() {
    let message = usage_error(&["pinvou", "nope"]);
    assert!(message.contains("benchmark"));
    assert!(message.contains("agent run"));
    assert!(message.contains("sessions"));
    assert!(message.contains("artifacts"));
}

#[test]
fn output_flag_still_applies_to_families() {
    let parsed = parse_args(["pinvou", "--output", "json", "sessions", "list"]).unwrap();
    assert_eq!(parsed.output(), OutputMode::Json);
}

/// A repeated identical mode is legal (scripts append `--output json`
/// unconditionally), but two conflicting global modes are a usage error —
/// the silent last-one-wins would make an appended flag flip a
/// human-formatted script's output without any signal.
#[test]
fn conflicting_global_output_modes_are_a_usage_error() {
    let error = parse_args([
        "pinvou",
        "--output",
        "human",
        "--output",
        "json",
        "benchmark",
        "list",
    ])
    .expect_err("conflicting --output modes must be rejected");
    assert_eq!(error.exit_code(), ExitCode::Usage);
    assert!(error.to_string().contains("conflicting"), "{error}");
    // Identical repeats stay legal.
    let parsed = parse_args([
        "pinvou",
        "--output",
        "json",
        "--output",
        "json",
        "benchmark",
        "list",
    ])
    .expect("identical --output repeats parse");
    assert_eq!(parsed.output(), OutputMode::Json);
}

/// Round-37 review: the bare `version` token is claimed by its own arm
/// outside the family table; nothing parsed it, so deleting the arm
/// regressed `pinvou version` to exit 2 with no failing row.
#[test]
fn bare_version_token_is_claimed_before_family_parsing() {
    let parsed = parse_args(["pinvou", "version"]).expect("bare version parses");
    let outcome = pinvou_cli::execute(parsed).expect("version executes");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("pinvou "));
}

#[test]
fn version_is_a_usable_subcommand_with_json_output() {
    let parsed = parse_args(["pinvou", "--version"]).expect("--version parses");
    let outcome = pinvou_cli::execute(parsed).expect("version executes");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("pinvou "));

    let parsed = parse_args(["pinvou", "--version", "--output", "json"])
        .expect("--version --output json parses");
    let outcome = pinvou_cli::execute(parsed).expect("version executes");
    let value: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("json output is a single line");
    assert!(value["version"].is_string());
}

// ── product data root (support::sandbox_home) ───────────────────────────────
//
// These live in an integration test, not in the lib's unit tests, on purpose:
// the lib test binary already contains tests that overwrite `PINVOU3_HOME`
// without taking any lock, and an integration test runs in its own process
// where the shared harness's ENV_LOCK is the only writer.

/// The CLI and the application must resolve one store root, never two. The
/// CLI used to consult `USERPROFILE` before `HOME`, so a unix host with
/// `USERPROFILE` exported (cross-platform CI images do that) sent the CLI to
/// a different root than the app, which reads `HOME` only on unix.
#[cfg(unix)]
#[test]
fn sandbox_home_ignores_userprofile_on_unix_like_the_application() {
    let _lock = env_lock();
    let home = std::env::temp_dir().join(format!("pinvou-cli-home-{}", std::process::id()));
    let profile = std::env::temp_dir().join(format!("pinvou-cli-profile-{}", std::process::id()));
    let _guard = EnvGuard::set(&[
        ("PINVOU3_HOME", None),
        ("HOME", home.to_str()),
        ("USERPROFILE", profile.to_str()),
    ]);

    let resolved = pinvou_cli::support::sandbox_home().expect("an absolute $HOME resolves");
    assert_eq!(
        resolved,
        home.join(".pinvou3"),
        "the store root must follow $HOME, the only variable the app reads on unix"
    );
}

/// A store root that is not absolute must fail before any family touches
/// the store: a relative root silently follows the working directory, so the
/// same command run from two directories would half-apply state to two
/// stores. `var_os`/`var` both report a set-but-empty variable as *present*,
/// so neither the `PINVOU3_HOME` nor the `$HOME` branch can rely on the
/// unset fallback to catch it.
#[test]
fn sandbox_home_refuses_every_non_absolute_store_root() {
    let _lock = env_lock();

    {
        let _guard = EnvGuard::set(&[("PINVOU3_HOME", Some(""))]);
        let error = pinvou_cli::support::sandbox_home()
            .expect_err("an empty PINVOU3_HOME must not resolve to the relative .pinvou3");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(error.to_string().contains("set but empty"), "{error}");
    }

    {
        let _guard = EnvGuard::set(&[("PINVOU3_HOME", Some("pinvou-cli-relative-root"))]);
        let error = pinvou_cli::support::sandbox_home()
            .expect_err("a relative PINVOU3_HOME must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(error.to_string().contains("absolute"), "{error}");
    }

    // The $HOME branch: unix-only, because the app resolves the home
    // directory from USERPROFILE/HOMEDRIVE on Windows.
    #[cfg(unix)]
    {
        let _guard = EnvGuard::set(&[("PINVOU3_HOME", None), ("HOME", Some(""))]);
        let error = pinvou_cli::support::sandbox_home()
            .expect_err("an empty $HOME must not resolve to the relative .pinvou3");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("cannot resolve home directory"),
            "{error}"
        );
    }
}

/// An absolute `PINVOU3_HOME` is returned exactly as the application's own
/// resolver produced it — the CLI validates, it does not re-derive.
#[test]
fn sandbox_home_returns_an_absolute_override_unchanged() {
    let _lock = env_lock();
    let root: PathBuf =
        std::env::temp_dir().join(format!("pinvou-cli-store-{}", std::process::id()));
    let _guard = EnvGuard::set(&[("PINVOU3_HOME", root.to_str())]);

    assert_eq!(
        pinvou_cli::support::sandbox_home().expect("an absolute override resolves"),
        root
    );
}
