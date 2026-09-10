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
        ("code", "agents"),
        ("files", "ingest"),
        ("voice", "asr-status"),
        ("deps", "check"),
        ("feedback", "submit"),
        ("monitor", "status"),
        ("artifacts", "list"),
    ];
    for (token, subcommand) in tokens {
        let parsed = parse_args(["pinvou", token, subcommand])
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

#[test]
fn stub_execution_reports_not_implemented_as_host_failure() {
    let parsed = parse_args(["pinvou", "monitor", "snapshot"]).unwrap();
    let error = pinvou_cli::execute(parsed).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failed);
    assert!(error.to_string().contains("not_implemented_yet"));
}

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
