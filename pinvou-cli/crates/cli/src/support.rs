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
code|files|voice|deps|feedback|monitor|artifacts <command>";

/// `$PINVOU3_HOME` when set (absolute), else `~/.pinvou3` — the same product
/// data root the benchmark family uses.
#[allow(dead_code)] // consumed by family implementations as they land
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

/// Destructive subcommands must opt in explicitly, mirroring the GUI's
/// confirmation dialogs.
#[allow(dead_code)] // consumed by family implementations as they land
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
#[allow(dead_code)] // consumed by family implementations as they land
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

#[allow(dead_code)] // consumed by family implementations as they land
pub fn success(stdout: String) -> crate::CliOutcome {
    crate::CliOutcome {
        exit_code: ExitCode::Success,
        stdout,
    }
}

/// Serializes `value` for `--output json` (single line) and renders `human`
/// verbatim otherwise.
#[allow(dead_code)] // consumed by family implementations as they land
pub fn render(output: crate::OutputMode, human: String, value: &serde_json::Value) -> String {
    match output {
        crate::OutputMode::Human => human,
        crate::OutputMode::Json => serde_json::to_string(value).unwrap_or_else(|error| {
            format!("{{\"error\":\"json serialization failed: {error}\"}}")
        }),
    }
}

/// Declares a not-yet-implemented family module. Family implementation
/// replaces the whole file; the stub only routes the top-level token so the
/// dispatch surface and usage errors exist from day one.
macro_rules! declare_stub_family {
    ($command:ident, $family:literal, $subcommands:literal) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum $command {
            Stub {
                subcommand: String,
                args: Vec<String>,
            },
        }

        pub fn parse(values: &[String]) -> Result<$command, crate::CliError> {
            let subcommand = values.get(1).cloned().ok_or_else(|| {
                crate::CliError::usage(concat!("usage: pinvou ", $family, " <", $subcommands, ">"))
            })?;
            Ok($command::Stub {
                subcommand,
                args: values.get(2..).unwrap_or_default().to_vec(),
            })
        }

        pub fn execute(
            command: $command,
            _output: crate::OutputMode,
        ) -> Result<crate::CliOutcome, crate::CliError> {
            let $command::Stub { subcommand, .. } = command;
            Err(crate::CliError::failed(format!(
                concat!($family, " {} not_implemented_yet"),
                subcommand
            )))
        }
    };
}

// The macro above is intentionally only referenced by the stub modules; each
// family implementation replaces its module wholesale.
pub(crate) use declare_stub_family;
