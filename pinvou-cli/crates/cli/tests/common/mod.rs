//! Shared harness for the per-family contract-test binaries under
//! `crates/cli/tests/`. Each `*_contract.rs` file is its own test binary and
//! declares `mod common;` plus `use common::…` for the subset it needs; the
//! `#![allow(dead_code)]` below covers the presets and helpers only some
//! families (or only the phase-B files) consume.
//!
//! Two invariants every user of this module must keep:
//!
//! 1. ENV serialization — a test that touches the process-global environment
//!    holds [`env_lock`] for its whole body, and nothing else takes it.
//! 2. One argv convention — the `run*` helpers here take the FULL argv
//!    including the leading `"pinvou"` token, and the JSON helper appends
//!    `--output json` at the END of the line.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_cli::{CliError, CliOutcome, ExitCode, execute, parse_args};
use pinvou3_lib::features::sessions::SessionStore;

/// Serialises tests that mutate the process-global `PINVOU3_HOME` environment
/// variable, preventing data races when the parallel test runner executes them
/// concurrently.
///
/// Each contract file is its own process; callers MUST hold the returned
/// guard for the whole test. The guard constructors and `Drop`s in this
/// module deliberately do NOT take the lock themselves: `std::sync::Mutex`
/// is not reentrant, so a guard taking it here would deadlock a caller that
/// already holds it.
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A unique, NOT-YET-CREATED path under the temp directory:
/// `<prefix>-<pid>-<nanos>`. The absent-home fixtures deliberately need a
/// path that does not exist; callers that want the directory use
/// [`scratch_dir`].
pub fn unique_root(prefix: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nonce}", std::process::id()))
}

/// [`unique_root`], with the directory created.
pub fn scratch_dir(prefix: &str) -> PathBuf {
    let root = unique_root(prefix);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The environment presets a family's sandbox home needs. Every variant sets
/// `PINVOU3_HOME` to the guard's root; the others add the CodeWhale secret
/// backend and the home-directory variables the platform code consults.
pub enum Isolation {
    /// `PINVOU3_HOME` only (artifacts/sessions/personas/projects/memory/
    /// knowledge/scheduled/misc/cli/plugins).
    ProductHome,
    /// `PINVOU3_HOME` plus the file secret backend under
    /// `<root>/codewhale-home` (connectors).
    FileSecrets,
    /// The file secret backend AND `$HOME` both at `<root>/secrets-home`
    /// (models).
    SecretsHome,
    /// The full seven-variable provider-home shape used by the code family:
    /// the file secret backend at `<root>/codewhale`, and `HOME`/`USERPROFILE`
    /// at `<root>/home` with the Windows pair pointing there too.
    ProviderHome,
}

/// Panic-safe restore for the variables a test overwrites: the previous
/// value is captured before the write and `Drop` puts back exactly that
/// state (re-set, or removed when it was absent) on every exit path,
/// including a failing assertion. Must be constructed while ENV_LOCK is
/// held. `var_os`, not `var`: a host may legitimately hold a non-UTF-8
/// value, and `var` would lose it.
pub struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

impl EnvGuard {
    /// Captures the previous values, then sets each entry (`None` removes
    /// the variable).
    pub fn set(entries: &[(&'static str, Option<&str>)]) -> Self {
        Self::apply(
            entries
                .iter()
                .map(|(name, value)| (*name, value.map(OsString::from))),
        )
    }

    /// [`EnvGuard::set`] for already-owned `OsString` values.
    pub fn set_values(entries: &[(&'static str, Option<OsString>)]) -> Self {
        Self::apply(entries.iter().cloned())
    }

    /// Captures the previous values, then removes the named variables.
    pub fn remove(names: &[&'static str]) -> Self {
        Self::apply(names.iter().map(|name| (*name, None)))
    }

    fn apply<I: IntoIterator<Item = (&'static str, Option<OsString>)>>(entries: I) -> Self {
        let entries: Vec<(&'static str, Option<OsString>)> = entries.into_iter().collect();
        let saved = entries
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in entries {
            match value {
                // SAFETY: the caller holds ENV_LOCK for the whole test, so
                // env writes are serialized in-process.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // SAFETY: as above.
                None => unsafe { std::env::remove_var(name) },
            }
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                // SAFETY: ENV_LOCK is held by the owning test.
                Some(value) => unsafe { std::env::set_var(name, value) },
                // SAFETY: ENV_LOCK is held by the owning test.
                None => unsafe { std::env::remove_var(name) },
            }
        }
    }
}

/// Sets the environment to a fresh throwaway sandbox home for the duration
/// of the test and restores the previous values on drop (every saved
/// variable is re-set, or removed when it was absent), then removes the
/// tree — both survive a panicking assertion.
pub struct HomeGuard {
    env: Option<EnvGuard>,
    root: PathBuf,
}

impl HomeGuard {
    /// Names the root `pinvou-cli-<family>-<label>-<pid>-<nanos>` under the
    /// temp directory and applies the [`Isolation`] preset's variables.
    pub fn new(family: &str, label: &str, isolation: Isolation) -> Self {
        let root = scratch_dir(&format!("pinvou-cli-{family}-{label}"));
        let mut vars: Vec<(&'static str, Option<OsString>)> =
            vec![("PINVOU3_HOME", Some(root.clone().into_os_string()))];
        vars.extend(match isolation {
            Isolation::ProductHome => Vec::new(),
            Isolation::FileSecrets => vec![
                ("CODEWHALE_SECRET_BACKEND", Some(OsString::from("file"))),
                (
                    "CODEWHALE_HOME",
                    Some(root.join("codewhale-home").into_os_string()),
                ),
            ],
            Isolation::SecretsHome => vec![
                ("CODEWHALE_SECRET_BACKEND", Some(OsString::from("file"))),
                (
                    "CODEWHALE_HOME",
                    Some(root.join("secrets-home").into_os_string()),
                ),
                ("HOME", Some(root.join("secrets-home").into_os_string())),
            ],
            Isolation::ProviderHome => vec![
                ("CODEWHALE_SECRET_BACKEND", Some(OsString::from("file"))),
                (
                    "CODEWHALE_HOME",
                    Some(root.join("codewhale").into_os_string()),
                ),
                ("HOME", Some(root.join("home").into_os_string())),
                ("USERPROFILE", Some(root.join("home").into_os_string())),
                ("HOMEDRIVE", Some(OsString::from("C:"))),
                ("HOMEPATH", Some(root.join("home").into_os_string())),
            ],
        });
        Self {
            env: Some(EnvGuard::set_values(&vars)),
            root,
        }
    }

    /// The sandbox home root.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// `~/.pinvou3/sessions` under the sandbox home.
    pub fn sessions_root(&self) -> PathBuf {
        self.root.join("sessions")
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // Restore every saved variable first (re-set, or removed when it was
        // absent), then unlink the tree. The restore must survive a panicking
        // assertion, otherwise the polluted values leak into every later test
        // in the process.
        if let Some(env) = self.env.take() {
            drop(env);
        }
        // Best-effort cleanup: a leftover temp directory must never turn an
        // assertion failure into a panic raised from inside Drop.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// RAII restore for tests that change the process working directory: the
/// previous directory is captured at construction and `Drop` puts it back on
/// every exit path, including a failing assertion — a manual restore line
/// after the call under test is skipped by a panicking assert and leaves the
/// fixture directory as the cwd of every later test in the binary.
/// Best-effort: a failing restore is ignored so `Drop` cannot raise on top of
/// an assertion failure. Must be constructed while [`env_lock`] is held (the
/// cwd is process-global, like the environment).
pub struct RestoreCwd(PathBuf);

impl RestoreCwd {
    /// Captures the process's current working directory.
    pub fn new() -> Self {
        Self(std::env::current_dir().expect("read the current directory"))
    }
}

impl Default for RestoreCwd {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RestoreCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// Creates one empty chat session through `SessionStore` (the same standalone
/// boot constructor the CLI uses — `SessionStore::boot()` in most families,
/// `open_store` in the artifacts family) and returns its id.
pub fn create_session_fixture() -> String {
    let store = SessionStore::boot().expect("boot session store");
    let session = store
        .create_new("test-model".to_owned(), None, std::env::temp_dir())
        .expect("create session");
    session.metadata.id
}

/// Runs one command through the in-process dispatch: parse (which must
/// succeed), then execute. Returns the outcome for exit-code and stdout
/// assertions. Takes the FULL argv including the leading `"pinvou"`.
pub fn run(arguments: &[&str]) -> Result<CliOutcome, CliError> {
    let parsed = parse_args(arguments).expect("arguments must parse");
    execute(parsed)
}

/// Runs one command and asserts the success class, returning stdout.
pub fn run_ok(arguments: &[&str]) -> String {
    let outcome = run(arguments).expect("execute must succeed");
    assert_eq!(outcome.exit_code, ExitCode::Success);
    outcome.stdout
}

/// Runs one command with `--output json` appended at the END of the line and
/// parses the single-line JSON envelope.
pub fn run_json(arguments: &[&str]) -> serde_json::Value {
    let mut owned = arguments.to_vec();
    owned.extend(["--output", "json"]);
    let stdout = run_ok(&owned);
    assert!(!stdout.contains('\n'), "json output must stay single-line");
    serde_json::from_str(&stdout).expect("json output must be a single serde_json line")
}

/// Runs the command and asserts the exit-1 (host failure) class, returning
/// the error for message assertions.
pub fn expect_failed(arguments: &[&str]) -> CliError {
    match run(arguments) {
        Ok(outcome) => panic!(
            "expected a failed execution, got exit {:?}: {}",
            outcome.exit_code, outcome.stdout
        ),
        Err(error) => {
            assert_eq!(error.exit_code(), ExitCode::Failed, "{error}");
            error
        }
    }
}

/// Asserts a PARSE-time refusal in the exit-2 usage class and returns the
/// message.
pub fn usage_error(arguments: &[&str]) -> String {
    let error = parse_args(arguments).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Usage);
    error.to_string()
}

/// Runs one command through the in-process dispatch and returns the error
/// from either lane: a parse-time refusal straight from `parse_args`, or an
/// execute-time failure from `execute` (which must not succeed). The exit
/// class is the caller's assertion — usage-refusal pins may deliberately
/// fire at parse, while other call sites expect exit-1 host failures — so
/// the name says what it guarantees and nothing more.
pub fn command_error(arguments: &[&str]) -> CliError {
    match parse_args(arguments.to_vec()) {
        Err(error) => error,
        Ok(parsed) => execute(parsed).expect_err("expected a command error"),
    }
}

/// Asserts the usage exit-code class (2) for a refused argv and returns the
/// message. Usage errors may surface from `parse` or from `execute`; both
/// must carry the exit-code 2 usage marker.
pub fn assert_usage(arguments: &[&str]) -> String {
    let error = command_error(arguments);
    assert_eq!(error.exit_code(), ExitCode::Usage, "{error}");
    error.to_string()
}

/// A command that parses but must fail at execute time; the exit class is
/// the caller's assertion.
pub fn execute_error(arguments: &[&str]) -> CliError {
    let parsed = parse_args(arguments.to_vec()).expect("valid command");
    execute(parsed).expect_err("expected execute failure")
}

/// A command that fails at execute time; returns the rendered message and
/// its exit class (not always `Failed`).
pub fn run_err(args: &[&str]) -> (String, ExitCode) {
    let error = execute_error(args);
    (error.to_string(), error.exit_code())
}

/// The family command types are private to the crate, so parse-level
/// assertions freeze the derived Debug shape of the parsed command.
pub fn parsed_debug(arguments: &[&str]) -> String {
    format!(
        "{:?}",
        parse_args(arguments.to_vec())
            .expect("valid command")
            .command()
    )
}
