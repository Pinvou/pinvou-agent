use pinvou_cli::{CliCommand, ExitCode, OutputMode, parse_args};

fn usage_error(args: [&str; 2]) -> String {
    let error = parse_args(args).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    error.to_string()
}

#[test]
fn every_family_token_dispatches_to_its_module() {
    // One representative valid subcommand per family: the parse layer must
    // route the family token even when the subcommand-specific validation
    // differs per family.
    let tokens = [
        ("sessions", "list"),
        ("models", "list"),
        ("settings", "get"),
        ("memory", "overview"),
        ("knowledge", "stats"),
        ("scheduled", "list"),
        ("plugins", "readiness"),
        ("connectors", "status"),
        ("personas", "list"),
        ("code", "agents list"),
        // files ingest requires its PATH argument since the family was
        // implemented (it used to be a bare-token stub).
        ("files", "ingest /tmp/a.md"),
        ("voice", "asr-status"),
        ("deps", "check"),
        // feedback submit requires its options since the family was
        // implemented (it used to be a bare-token stub).
        (
            "feedback",
            "submit --type issue --title t --body-file /tmp/b.md",
        ),
        ("monitor", "status"),
        ("artifacts", "list"),
    ];
    for (token, subcommand) in tokens {
        let mut argv = vec!["pinvou".to_string(), token.to_string()];
        argv.extend(subcommand.split_whitespace().map(str::to_string));
        let parsed = parse_args(&argv)
            .unwrap_or_else(|error| panic!("family {token} did not dispatch: {error}"));
        let command = parsed.command();
        assert!(
            matches!(
                command,
                CliCommand::Sessions(_)
                    | CliCommand::Models(_)
                    | CliCommand::Memory(_)
                    | CliCommand::Knowledge(_)
                    | CliCommand::Scheduled(_)
                    | CliCommand::Plugins(_)
                    | CliCommand::Connectors(_)
                    | CliCommand::Personas(_)
                    | CliCommand::Code(_)
                    | CliCommand::Files(_)
                    | CliCommand::Voice(_)
                    | CliCommand::Deps(_)
                    | CliCommand::Feedback(_)
                    | CliCommand::Monitor(_)
                    | CliCommand::Artifacts(_)
            ),
            "unexpected command variant for {token}: {command:?}"
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
    let message = usage_error(["pinvou", "sessions"]);
    assert!(
        message.contains("sessions"),
        "unexpected message: {message}"
    );
    let message = usage_error(["pinvou", "memory"]);
    assert!(message.contains("memory"), "unexpected message: {message}");
}

// The former `stub_execution_reports_not_implemented_as_host_failure` test
// was removed when the files/voice/deps/feedback/monitor families were
// implemented: it asserted the temporary `not_implemented_yet` stub error for
// `monitor snapshot`, which now boots the windowless host instead (its real
// behavior is covered by the `#[ignore]` opt-in tests in misc_contract.rs).

#[test]
fn unknown_top_level_command_lists_the_full_surface() {
    let message = usage_error(["pinvou", "nope"]);
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

#[test]
fn version_is_a_usable_subcommand_with_json_output() {
    let parsed = parse_args(["pinvou", "--version"]).expect("--version parses");
    let outcome = pinvou_cli::execute(parsed).expect("version executes");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    assert!(outcome.stdout.contains("pinvou "), "{}", outcome.stdout);

    let parsed = parse_args(["pinvou", "--version", "--output", "json"])
        .expect("--version --output json parses");
    let outcome = pinvou_cli::execute(parsed).expect("version executes");
    let value: serde_json::Value =
        serde_json::from_str(&outcome.stdout).expect("json output is a single line");
    assert!(value["version"].is_string(), "{}", outcome.stdout);
}
