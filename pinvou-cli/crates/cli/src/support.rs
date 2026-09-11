//! Shared helpers for the product feature families added for GUI parity.
//!
//! Conventions for every family module (enforced by review, not the type
//! system): `parse` recognizes the family's subcommands and returns a typed
//! command; `execute` maps the command onto `pinvou3_lib::features::*` /
//! `pinvou3_lib::platform::*` building blocks directly for pure-storage
//! operations, or through `pinvou_product_backend::run_with_product_backend`
//! (the windowless product host) for engine/model-dependent operations.
//! Exit codes: 0 success, 1 host failure, 2 usage error. JSON output is a
//! single serde_json line.

use std::path::PathBuf;

use crate::{CliError, ExitCode};

pub const TOP_LEVEL_USAGE: &str = "usage: pinvou benchmark <command> | pinvou agent run | \
     pinvou sessions|models|settings|memory|knowledge|scheduled|plugins|connectors|personas|\
code|files|voice|deps|feedback|monitor|artifacts <command> | pinvou --version";

/// `$PINVOU3_HOME` when set (absolute), else `~/.pinvou3` — the same product
/// data root the benchmark family uses.
pub fn sandbox_home() -> Result<PathBuf, CliError> {
    if let Some(home) = std::env::var_os("PINVOU3_HOME") {
        let home = PathBuf::from(home);
        if !home.is_absolute() {
            return Err(CliError::failed("PINVOU3_HOME must be an absolute path"));
        }
        return Ok(home);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| CliError::failed("cannot resolve home directory"))?;
    Ok(home.join(".pinvou3"))
}

/// Mirrors `features::sessions::validate_session_id` (crate-private in the
/// app): only `[A-Za-z0-9_-]`, so an id can never traverse out of the
/// sessions root when it is joined onto a path. Shared by every family that
/// accepts a session id so the usage-error contract is uniform.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Usage error for an id outside the [`valid_session_id`] alphabet.
pub fn require_valid_session_id(id: &str, action: &str) -> Result<(), CliError> {
    if valid_session_id(id) {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "{action} requires a valid session id ([A-Za-z0-9_-])"
        )))
    }
}

/// Destructive subcommands must opt in explicitly, mirroring the GUI's
/// confirmation dialogs.
pub fn require_yes(confirmed: bool) -> Result<(), CliError> {
    if confirmed {
        Ok(())
    } else {
        Err(CliError::usage(
            "pass --yes to confirm this destructive action",
        ))
    }
}

/// Resolve a secret from `--api-key-env VAR` / `--api-key-stdin`. Plaintext
/// argv flags are deliberately not offered: argv leaks through shell history
/// and process listings.
pub fn resolve_secret(
    api_key_env: &Option<String>,
    api_key_stdin: bool,
) -> Result<Option<String>, CliError> {
    if api_key_env.is_some() && api_key_stdin {
        return Err(CliError::usage(
            "use only one of --api-key-env or --api-key-stdin",
        ));
    }
    if let Some(var) = api_key_env {
        let value = std::env::var(var).map_err(|_| {
            CliError::failed(format!("secret environment variable {var} is not set"))
        })?;
        // A set-but-empty variable is as useless as an empty stdin read;
        // storing it would report "key-set" while every signed request fails.
        if value.trim().is_empty() {
            return Err(CliError::failed(format!(
                "secret environment variable {var} is empty"
            )));
        }
        return Ok(Some(value));
    }
    if api_key_stdin {
        let mut value = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut value)
            .map_err(|error| CliError::failed(format!("cannot read secret from stdin: {error}")))?;
        let trimmed = value.trim().to_owned();
        if trimmed.is_empty() {
            return Err(CliError::failed("no secret provided on stdin"));
        }
        return Ok(Some(trimmed));
    }
    Ok(None)
}

pub fn success(stdout: String) -> crate::CliOutcome {
    crate::CliOutcome {
        exit_code: ExitCode::Success,
        stdout,
    }
}

/// Platform binary-name candidates for a bare CLI name: Windows installs are
/// `.exe` real binaries or npm `.cmd` shims, and PATH scanning must try both
/// because bare names never match there.
pub fn binary_candidates(name: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            name.to_owned(),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![name.to_owned()]
    }
}

/// Builds a `Command` for a resolved vendor CLI path. Windows cannot
/// `CreateProcess` an npm `.cmd` shim directly, so `.cmd` targets run through
/// `cmd /D /S /C` — the same wrapper the app's platform process helper uses.
pub fn build_command(executable: &std::path::Path, args: &[&str]) -> std::process::Command {
    #[cfg(target_os = "windows")]
    if executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"))
    {
        let mut command = std::process::Command::new("cmd");
        command.arg("/D").arg("/S").arg("/C");
        command.arg(executable);
        command.args(args);
        return command;
    }
    let mut command = std::process::Command::new(executable);
    command.args(args);
    command
}

/// Puts a long-running vendor CLI child in its own process group so a
/// timeout kill can take its npm/shell descendants with it instead of
/// orphaning them (the app sets the same group for connector CLI spawns).
pub fn set_process_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// Kills a timed-out vendor CLI child **and its descendants**: unix takes the
/// whole process group (the child was spawned with [`set_process_group`]);
/// Windows uses `taskkill /T`, mirroring the app's `kill_pid_tree`.
pub fn kill_process_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Safety: `kill` with a negative pid signals the process group; the
        // child was put in its own group at spawn time. ESRCH (already gone)
        // is fine to ignore.
        let group = -(child.id() as i32);
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Serializes `value` for `--output json` (single line) and renders `human`
/// verbatim otherwise.
pub fn render(output: crate::OutputMode, human: String, value: &serde_json::Value) -> String {
    match output {
        crate::OutputMode::Human => human,
        crate::OutputMode::Json => serde_json::to_string(value).unwrap_or_else(|error| {
            format!("{{\"error\":\"json serialization failed: {error}\"}}")
        }),
    }
}
