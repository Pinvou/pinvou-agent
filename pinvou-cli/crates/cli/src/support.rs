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

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use crate::{CliError, CliOutcome, ExitCode};

/// The one lock for every test in this crate's LIBRARY test binary that
/// mutates process-global environment variables (`PINVOU3_HOME`, secret
/// variables). The lib's `#[cfg(test)]` tests run in parallel threads in one
/// process, and the env has no in-process synchronization of its own, so two
/// modules steering it at once race each other's fixtures: the gaia tests in
/// `lib.rs` point `PINVOU3_HOME` at per-fixture temp roots while the models
/// credential tests in `models.rs` do the same for their stores, and the two
/// groups were reproduced failing 4/5 in combined runs while both were green
/// in isolation. The fix is one lock shared by both groups — a per-module
/// static would serialize inside its own module and still race the other's.
///
/// Test-only: integration tests link the library WITHOUT `cfg(test)`, so this
/// static does not exist there and cannot be dead code in a normal build.
/// Each integration binary keeps its own local `ENV_LOCK` instead — separate
/// processes have no shared address space, so there is nothing to serialize
/// between them.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Decodes raw `argv` into the UTF-8 strings the parse layer consumes.
///
/// The program slot (`argv[0]`) is loss-converted when it is not UTF-8:
/// `parse_args` discards it, and odd launchers/execve paths can legitimately
/// carry a byte path — that must not lock the user out of every command.
/// Every later argument must instead be valid UTF-8 or the CLI refuses
/// loudly: the previous behavior silently loss-converted them all, so
/// `pinvou sessions show $'/tmp/\xff-session'` turned an undecodable id into
/// a look-alike name and reported "not found" for an id the user never
/// typed. Per the family exit-code convention this is argv-decidable, so the
/// caller reports it as a usage error (exit 2; see [`read_text_file_capped`]
/// for the other half of the rule, where content-dependent failures exit 1).
///
/// Split out of `main` (a bin crate the integration tests never execute)
/// so the refusal can be unit-pinned; the caller decides how an offending
/// index is rendered.
pub fn decode_arguments(raw: Vec<std::ffi::OsString>) -> Result<Vec<String>, usize> {
    let mut arguments = Vec::with_capacity(raw.len());
    for (index, argument) in raw.into_iter().enumerate() {
        if index == 0 {
            // The program slot is exempt (see the doc comment above); keep
            // the loop shape so every later argument goes through one path.
            arguments.push(argument.to_string_lossy().into_owned());
            continue;
        }
        let Some(text) = argument.to_str() else {
            return Err(index);
        };
        arguments.push(text.to_owned());
    }
    Ok(arguments)
}

/// Writes the run's report to `out` and returns the process exit code.
///
/// Rust ignores SIGPIPE, so a closed pipe (`pinvou ... | head`) turns the write
/// into an error instead of a signal. BrokenPipe therefore only suppresses the
/// 101 panic — it does NOT rewrite the run's verdict: the outcome's own exit
/// code still stands, or `pinvou benchmark run … | head` on a run with failing
/// tasks would report success and silently break the documented 0/1/2 contract
/// scripts branch on. Any other write failure is a real reporting failure and
/// exits 1. Extracted from `main` (a bin crate the integration tests never
/// execute) so the contract is unit-pinned.
pub fn emit_report<W: std::io::Write>(mut out: W, outcome: &CliOutcome) -> i32 {
    match writeln!(out, "{}", outcome.stdout) {
        Ok(()) => outcome.exit_code.as_i32(),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => outcome.exit_code.as_i32(),
        Err(error) => {
            let _ = writeln!(std::io::stderr(), "pinvou: cannot write output: {error}");
            1
        }
    }
}

pub const TOP_LEVEL_USAGE: &str = "usage: pinvou benchmark <command> | pinvou agent run | \
     pinvou sessions|models|settings|memory|knowledge|scheduled|plugins|connectors|personas|\
projects|code|files|voice|deps|feedback|monitor|artifacts <command> | pinvou --version|version";

/// `$PINVOU3_HOME` when set (absolute), else `~/.pinvou3` — the same product
/// data root the app uses, resolved by the app's own resolver.
///
/// The path is NOT recomputed here. `pinvou3_lib::platform::paths::
/// pinvou3_home` is the single source of truth and this helper only layers
/// the CLI-specific contract on top of it, because both resolvers live in
/// the same binary and several commands mix them inside one invocation
/// (`connectors` imports `pinvou3_home` and calls `sandbox_home`;
/// `scheduled` does the same). A second private resolver diverged on three
/// real inputs: a non-UTF-8 `PINVOU3_HOME` (kept by `var_os`, dropped by
/// the app's `var`), `USERPROFILE` set on a unix host (honored here, never
/// read by the app's unix `user_home_dir`), and Windows path compatibility
/// (the app runs the override through `platform_compat_path`, which remaps
/// `/tmp/...` onto the real temp dir). Each divergence splits the store
/// between two roots for the same command.
///
/// What the CLI adds is the failure the app cannot express: its resolver
/// returns a bare `PathBuf` and silently accepts whatever the environment
/// hands it, while every family here calls `sandbox_home()?` precisely so
/// that one binary cannot half-apply state against a cwd-relative store.
/// So the absolute-path invariant is asserted on the *resolved* path on
/// every branch — override or `$HOME` — and set-but-empty variables (a
/// systemd unit, cron, a minimal container) are rejected explicitly
/// instead of collapsing into the relative `.pinvou3`.
pub fn sandbox_home() -> Result<PathBuf, CliError> {
    // The raw override is read separately from the resolved path: the
    // diagnostics below must distinguish "the app ignored an override that
    // the operator did set" from "no override was set", which the resolved
    // path alone cannot tell.
    let raw = std::env::var_os("PINVOU3_HOME");
    validate_sandbox_home(raw.as_deref(), pinvou3_lib::platform::paths::pinvou3_home())
}

/// CLI-side contract check over whatever the app resolver returned. Split
/// out of [`sandbox_home`] so the rules can be unit-tested without mutating
/// process-global environment variables.
fn validate_sandbox_home(
    raw_override: Option<&std::ffi::OsStr>,
    resolved: PathBuf,
) -> Result<PathBuf, CliError> {
    if let Some(raw) = raw_override {
        // The app reads the override with `std::env::var`, so a non-UTF-8
        // value is dropped on the floor there and the store silently stays
        // at `~/.pinvou3` while the operator believes it was relocated.
        // Refusing is the only outcome that cannot split the store: the CLI
        // must never write to a root the app will not read.
        let Some(value) = raw.to_str() else {
            return Err(CliError::failed(
                "PINVOU3_HOME is not valid UTF-8; the application ignores such a value and \
                 would use a different data root",
            ));
        };
        // A set-but-empty variable is reported as *present* by both `var`
        // and `var_os`, so it never reaches the `~/.pinvou3` fallback; it
        // resolves to the empty path instead. Naming the cause beats the
        // generic absolute-path error the check below would otherwise give.
        if value.trim().is_empty() {
            return Err(CliError::failed(
                "PINVOU3_HOME is set but empty; unset it to use ~/.pinvou3, or set an \
                 absolute path",
            ));
        }
    }
    if !resolved.is_absolute() {
        // One check for every branch. An empty or relative `$HOME` (cron,
        // systemd, a scratch container) makes the app's `user_home_dir`
        // return an empty path, whose `.pinvou3` join is the *relative*
        // `.pinvou3` — a store rooted at the current working directory,
        // exactly what the families' `sandbox_home()?` guard exists to
        // prevent. The old `ok_or_else` could not catch it, because a
        // set-but-empty variable is `Some("")`, not `None`.
        return Err(CliError::failed(match raw_override {
            Some(_) => format!(
                "PINVOU3_HOME must be an absolute path (it resolved to {})",
                resolved.display()
            ),
            None => format!(
                "cannot resolve home directory: the product data root resolved to the \
                 relative path {}",
                resolved.display()
            ),
        }));
    }
    Ok(resolved)
}

/// Reads a UTF-8 text file with a byte cap so `--*-file` arguments cannot
/// load an unbounded source (a multi-GB log, a character device like
/// /dev/zero) into memory before the family's own truncation runs, and
/// non-regular files (a FIFO's open/read would block before any cap could
/// act) are rejected by a pre-flight probe. The read itself is bounded
/// (`Read::take`), so the failure is a clean CLI error rather than an OOM
/// or a hang.
///
/// **Exit class** (the rule this helper and [`resolve_secret`] share, so
/// scripts can branch on the documented 0/1/2 contract): exit 2 (usage) is
/// reserved for errors decidable from argv alone — an unknown flag, a
/// missing value, two mutually exclusive flags. Everything that depends on
/// the *content* of a resource the command was pointed at — a file, stdin,
/// an environment variable — is a host failure, exit 1, whether that
/// content is missing, unreadable, too large, or not UTF-8. A byte cap is
/// therefore always exit 1, from a file and from stdin alike.
pub fn read_text_file_capped(
    path: &Path,
    max_bytes: usize,
    action: &str,
) -> Result<String, CliError> {
    // Regular files only, checked through metadata BEFORE the open: a FIFO's
    // open blocks until a writer appears and its read blocks until bytes
    // arrive, so neither the cap nor any downstream deadline could act. The
    // metadata probe follows symlinks, so a link to a regular file still
    // reads. The probe is NOT atomic with the open below — a path swapped
    // to a FIFO in between still reaches `File::open` and can block — so it
    // rejects the ordinary case, it does not close the race.
    let meta = std::fs::metadata(path).map_err(|error| {
        // Classify the probe failure. Reporting every `metadata` error as
        // "does not exist" sends the user hunting for a typo when the real
        // cause is EACCES on a parent directory, a symlink loop (ELOOP) or
        // an over-long path (ENAMETOOLONG) — different problems with
        // different fixes. The exit class stays 1 for all of them.
        match error.kind() {
            std::io::ErrorKind::NotFound => {
                CliError::failed(format!("{action}: {} does not exist", path.display()))
            }
            std::io::ErrorKind::PermissionDenied => CliError::failed(format!(
                "{action}: {} cannot be read: permission denied",
                path.display()
            )),
            _ => CliError::failed(format!(
                "{action}: cannot inspect {}: {error}",
                path.display()
            )),
        }
    })?;
    if !meta.is_file() {
        return Err(CliError::failed(format!(
            "{action}: {} is not a regular file",
            path.display()
        )));
    }
    let file = std::fs::File::open(path).map_err(|error| {
        CliError::failed(format!("{action}: cannot read {}: {error}", path.display()))
    })?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::failed(format!("{action}: cannot read {}: {error}", path.display()))
        })?;
    if bytes.len() > max_bytes {
        return Err(CliError::failed(format!(
            "{action}: {} exceeds the {max_bytes}-byte read limit",
            path.display()
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| CliError::failed(format!("{action}: {} is not valid UTF-8", path.display())))
}

/// Replaces every character that can corrupt a terminal row with a space,
/// so a vendor- or user-controlled string cannot break the column structure
/// of a human tab-separated row: control characters (newline, tab, ESC, …)
/// **and** the invisible formatting characters that reorder or hide text
/// without occupying a column. The latter matter as much as the former
/// here: a session title carrying a bidi override renders the whole row
/// reordered in the terminal, and the columns the row promises no longer
/// mean what they show. JSON output carries the original untouched.
///
/// Shared by the families' human rows (`sessions`, `projects`, `code`, …)
/// so the hygiene is uniform; see `is_row_unsafe_char` for the exact set.
pub fn collapse_control_characters(value: &str) -> String {
    value
        .chars()
        .map(|ch| if is_row_unsafe_char(ch) { ' ' } else { ch })
        .collect()
}

/// The display-hygiene set behind [`collapse_control_characters`], copied —
/// not invented — from the GUI's `features::marketplace::store::
/// is_display_unsafe_char` (crate-private there), which this crate already
/// mirrors in `plugins::is_display_unsafe_char`. Same set, different verb:
/// `plugins` *rejects* a stored display name containing any of these, while
/// this helper *sanitizes* strings that are only rendered. Keeping one set
/// means a title the GUI would refuse cannot slip through the CLI's rows.
fn is_row_unsafe_char(ch: char) -> bool {
    ch.is_control()
        || matches!(ch,
            '\u{00AD}' // SOFT HYPHEN
            | '\u{200B}'..='\u{200D}' // ZERO WIDTH SPACE..JOINER
            | '\u{2028}'..='\u{2029}' // LINE/PARAGRAPH SEPARATOR
            | '\u{202A}'..='\u{202E}' // bidi embedding/override controls
            | '\u{2066}'..='\u{2069}' // bidi isolate controls
            | '\u{FEFF}' // BOM / ZERO WIDTH NO-BREAK SPACE
        )
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

/// Family flag parser shared by the sessions/scheduled/knowledge/plugins/
/// personas/connectors families (mirrors the pair-based `named_options`
/// helper in lib.rs, extended with valueless boolean flags).
///
/// The input is flags-only: **every** token must be a known boolean flag
/// from `boolean_flags`, or a known value flag from `value_flags` followed
/// by its value. There is no positional channel — a token that is neither
/// is an "unsupported option" usage error, so callers that also take
/// positionals must strip them before calling. A value is rejected (usage
/// error, "requires a value") when it is missing, empty, or looks like
/// another flag (`--` prefix); repeating any flag is a duplicate usage
/// error. Returns `(value-flag pairs in argv order, boolean flags seen)` —
/// the second element is the boolean-flag list, not positionals. `family`
/// names the family in error messages.
pub fn parse_family_flags<'a>(
    values: &'a [String],
    value_flags: &[&str],
    boolean_flags: &[&str],
    family: &str,
) -> Result<(Vec<(&'a str, &'a str)>, Vec<&'a str>), CliError> {
    let mut options = Vec::new();
    let mut flags = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if boolean_flags.contains(&token) {
            if flags.contains(&token) {
                return Err(CliError::usage(format!(
                    "duplicate {family} option {token}"
                )));
            }
            flags.push(token);
            index += 1;
            continue;
        }
        if !value_flags.contains(&token) {
            return Err(CliError::usage(format!(
                "unsupported {family} option: {token}"
            )));
        }
        if options.iter().any(|(name, _)| *name == token) {
            return Err(CliError::usage(format!(
                "duplicate {family} option {token}"
            )));
        }
        let value = values
            .get(index + 1)
            .ok_or_else(|| CliError::usage(format!("{family} option {token} requires a value")))?;
        if value.is_empty() || value.starts_with("--") {
            return Err(CliError::usage(format!(
                "{family} option {token} requires a value"
            )));
        }
        options.push((token, value.as_str()));
        index += 2;
    }
    Ok((options, flags))
}

/// First value filed under `name` in a [`parse_family_flags`] option list.
pub fn family_option<'a>(options: &'a [(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    options
        .iter()
        .find_map(|(candidate, value)| (*candidate == name).then_some(*value))
}

/// Positive-integer option shared by the families (`--limit`, `--max-*`):
/// absent flag = `None`; present but zero/non-numeric = usage error.
///
/// `T` is assumed to be an integer type — every caller instantiates it with
/// `usize` or `u64`. The bounds cannot express that (`FromStr + PartialOrd +
/// Default` is the weakest set that lets "parses, and is greater than zero" be
/// written generically), so the assumption is recorded rather than enforced:
/// instantiating `T` with a float would accept `0.5` for an option this error
/// message calls a positive *integer*. The bound is deliberately not
/// tightened, because it is part of a signature six families depend on.
pub fn parse_family_positive<T>(
    options: &[(&str, &str)],
    name: &str,
    family: &str,
) -> Result<Option<T>, CliError>
where
    T: std::str::FromStr + std::cmp::PartialOrd + Default,
{
    match family_option(options, name) {
        None => Ok(None),
        Some(value) => value
            .parse::<T>()
            .ok()
            .filter(|count| *count > T::default())
            .map(Some)
            .ok_or_else(|| CliError::usage(format!("{family} {name} must be a positive integer"))),
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
///
/// **Exit class**, the same rule [`read_text_file_capped`] documents: only
/// the argv-decidable error here — passing both flags at once — is a usage
/// error (exit 2). Every verdict about the *content* behind a flag is a
/// host failure (exit 1): the variable is unset, the variable is empty,
/// stdin is empty, stdin exceeded the cap. Before this rule was applied the
/// over-cap stdin read alone returned 2 while the over-cap file read
/// returned 1, so "input exceeded a byte cap" had two different exit codes
/// depending on where the input came from.
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
        let raw = std::env::var(var).map_err(|error| match error {
            // Same distinction `validate_sandbox_home` draws for
            // PINVOU3_HOME: a set-but-non-UTF-8 value is a different problem
            // from an unset one, and reporting it as "not set" would send the
            // user hunting for a missing export when the variable exists but
            // carries undecodable bytes.
            std::env::VarError::NotUnicode(_) => CliError::failed(format!(
                "secret environment variable {var} is set but not valid UTF-8"
            )),
            _ => CliError::failed(format!("secret environment variable {var} is not set")),
        })?;
        // A set-but-empty variable is as useless as an empty stdin read;
        // storing it would report "key-set" while every signed request fails.
        if raw.trim().is_empty() {
            return Err(CliError::failed(format!(
                "secret environment variable {var} is empty"
            )));
        }
        // Trim like the stdin lane below: environment secrets routinely
        // carry a trailing newline (`read KEY < key.txt`, a file mounted by
        // a CI secret store, most `.env` loaders), and storing it verbatim
        // put that "\n" into the credential while every read path trimmed —
        // the provider kept reporting `configured` and every signed request
        // 401'd. The empty check above stays on the *raw* value so a
        // whitespace-only variable is still named as empty, not trimmed
        // into Ok(Some("")).
        return Ok(Some(raw.trim().to_owned()));
    }
    if api_key_stdin {
        // Bounded read: unbounded stdin (`yes | pinvou ... --api-key-stdin`)
        // would exhaust memory before the empty check ran. Reading one byte
        // past the cap distinguishes "at the cap" from "over it".
        const MAX_SECRET_BYTES: u64 = 64 * 1024;
        let mut value = String::new();
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        if let Err(error) = std::io::Read::read_to_string(
            &mut std::io::Read::take(&mut handle, MAX_SECRET_BYTES + 1),
            &mut value,
        ) {
            return Err(CliError::failed(format!(
                "cannot read secret from stdin: {error}"
            )));
        }
        if value.len() as u64 > MAX_SECRET_BYTES {
            // `failed`, not `usage`: the cap is about what came down the
            // pipe, not about the shape of the command line — the same
            // class `read_text_file_capped` returns for an over-cap file.
            return Err(CliError::failed(
                "the stdin secret exceeds the 64 KiB limit",
            ));
        }
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
        // The two guards below are NOT defensive padding; they are the
        // primitive's contract, copied from the source of truth this helper
        // mirrors (`platform::os::posix::kill_pid_tree`, documented in
        // `platform/os/unsupported.rs`). kill(2) gives pid 0 and -1 special
        // meanings — 0 is the caller's own group, -1 is *every* process the
        // user may signal — so a pgid floor of 1 is what keeps a stray
        // `kill(-1, SIGKILL)` from taking the developer's whole desktop
        // session down (it has happened; see the posix doc comment). The
        // `try_from` is the second half: a pid above `i32::MAX` wraps
        // negative under `as i32`, and negating a negative yields a
        // *positive* pid, i.e. SIGKILL to an unrelated process.
        if let Some(group) = i32::try_from(child.id()).ok().filter(|group| *group > 1) {
            // SAFETY: libc::kill is a direct kill(2) wrapper; no memory is
            // touched. The child was put in its own group at spawn time by
            // `set_process_group`. ESRCH (already gone) is fine to ignore.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        // The single-pid leg of the app's helper is the `child.kill()`
        // below, which runs on every branch — including the one where the
        // guards refused the group kill.
    }
    #[cfg(target_os = "windows")]
    {
        // Difference from the app on purpose, recorded rather than fixed:
        // `platform::process::kill_process_tree` resolves `taskkill`
        // through its hardened `external_command` PATH helper and gives it
        // a 2s budget, but `platform::process` is `pub(crate)` in the app
        // crate and unreachable from here. `.output()` keeps the helper's
        // streams off our stdio (a bare spawn would inherit them) but is
        // unbounded, so a wedged WMI/RPC can stall this call.
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
        crate::OutputMode::Json => serde_json::to_string(value).unwrap_or_else(json_render_failure),
    }
}

/// `--output json` fallback when serialization fails.
///
/// The honest fix would be to propagate the error so the caller exits
/// non-zero, but this signature is the shared renderer for all 16 families
/// and changing it is a cross-module break; so the failure is made as loud
/// as a `-> String` can be. Three things happen instead of a silent
/// success-shaped payload: the object is marked `"ok": false` (a consumer
/// branching on the payload sees a failure rather than a record whose
/// `error` field could plausibly be a normal field), a diagnostic goes to
/// **stderr** so an operator watching the terminal sees it even when stdout
/// is piped into a parser, and a debug assertion fires in test/debug builds
/// — the path is unreachable today (a `serde_json::Value` cannot hold a
/// non-finite float or a non-string map key, the only ways `to_string` can
/// fail), so anything reaching it is a bug worth failing on.
fn json_render_failure(error: serde_json::Error) -> String {
    debug_assert!(false, "a serde_json::Value failed to serialize: {error}");
    let _ = writeln!(
        std::io::stderr(),
        "pinvou: cannot serialize the JSON payload: {error}"
    );
    json_failure_payload(&format!("json serialization failed: {error}"))
}

/// Builds the failure payload through serde_json itself rather than by
/// interpolating into a format string: an error message containing a quote
/// (or a backslash, or a newline) used to emit *invalid* JSON, which a
/// consumer cannot even parse to discover that something went wrong.
fn json_failure_payload(message: &str) -> String {
    serde_json::to_string(&serde_json::json!({ "ok": false, "error": message }))
        // A two-field object of plain strings cannot fail to serialize; the
        // literal exists so this helper has no panic path at all.
        .unwrap_or_else(|_| r#"{"ok":false,"error":"json serialization failed"}"#.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        collapse_control_characters, decode_arguments, emit_report, json_failure_payload,
        resolve_secret, validate_sandbox_home,
    };
    use crate::{CliOutcome, ExitCode};
    use std::io::{self, Write};
    use std::path::PathBuf;

    /// A writer that always fails with the given error kind, standing in for
    /// a closed pipe or an otherwise failed stdout.
    struct FailingWriter(io::ErrorKind);
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "injected"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn outcome(exit_code: ExitCode) -> CliOutcome {
        CliOutcome {
            exit_code,
            stdout: "report".to_owned(),
        }
    }

    #[test]
    fn emit_report_writes_the_report_and_honors_the_outcome_code() {
        let mut sink = Vec::new();
        let code = emit_report(&mut sink, &outcome(ExitCode::Success));
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(sink).unwrap(), "report\n");
        let mut sink = Vec::new();
        let code = emit_report(&mut sink, &outcome(ExitCode::Failed));
        assert_eq!(code, 1);
        assert_eq!(String::from_utf8(sink).unwrap(), "report\n");
    }

    #[test]
    fn emit_report_keeps_the_runs_verdict_when_the_pipe_closes() {
        let code = emit_report(
            FailingWriter(io::ErrorKind::BrokenPipe),
            &outcome(ExitCode::Failed),
        );
        assert_eq!(
            code, 1,
            "a closed pipe suppresses the panic, it does not turn a failed run into a success"
        );
        let code = emit_report(
            FailingWriter(io::ErrorKind::BrokenPipe),
            &outcome(ExitCode::Success),
        );
        assert_eq!(code, 0, "a closed pipe on a successful run stays quiet");
    }

    #[test]
    fn emit_report_surfaces_other_write_failures_as_exit_one() {
        let code = emit_report(
            FailingWriter(io::ErrorKind::PermissionDenied),
            &outcome(ExitCode::Success),
        );
        assert_eq!(code, 1);
    }

    /// `validate_sandbox_home` is the whole CLI-side contract over the app's
    /// resolver, so it is pinned without touching process-global env: the
    /// resolved path is passed in exactly as `pinvou3_home()` would have
    /// produced it for the given raw override.
    #[test]
    fn sandbox_home_validation_rejects_every_non_absolute_root() {
        let os = std::ffi::OsStr::new;
        // `temp_dir` rather than a literal `/...`: a POSIX-looking literal
        // is *not* absolute on Windows (no prefix), which would make this
        // test assert the opposite of its intent there.
        let absolute = std::env::temp_dir().join("pinvou-cli-sandbox-home");

        // No override, healthy $HOME: the resolver's path passes through
        // untouched — the CLI must not "fix up" what the app computed.
        let resolved = absolute.join(".pinvou3");
        assert_eq!(
            validate_sandbox_home(None, resolved.clone()).unwrap(),
            resolved
        );

        // Empty $HOME: `user_home_dir` yields the empty path, whose
        // `.pinvou3` join is the cwd-relative `.pinvou3`. The old
        // `ok_or_else` never fired here, because the variable is `Some("")`.
        let error = validate_sandbox_home(None, PathBuf::from(".pinvou3"))
            .expect_err("a relative product data root must never be used");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(
            error.to_string().contains("cannot resolve home directory"),
            "{error}"
        );

        // Set-but-empty PINVOU3_HOME (systemd units, cron, minimal
        // containers export variables this way) is named as such rather
        // than reported as a generic relative-path error.
        let error = validate_sandbox_home(Some(os("")), PathBuf::from(""))
            .expect_err("an empty PINVOU3_HOME must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(error.to_string().contains("set but empty"), "{error}");
        let error = validate_sandbox_home(Some(os("   ")), PathBuf::from("   "))
            .expect_err("a whitespace-only PINVOU3_HOME must be refused");
        assert!(error.to_string().contains("set but empty"), "{error}");

        // Relative PINVOU3_HOME keeps its historical wording ("absolute"),
        // which sessions_contract.rs asserts on.
        let relative = "pinvou-cli-relative-root";
        let error = validate_sandbox_home(Some(os(relative)), PathBuf::from(relative))
            .expect_err("a relative PINVOU3_HOME must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(error.to_string().contains("absolute"), "{error}");

        // An absolute override passes through as the app resolved it.
        assert_eq!(
            validate_sandbox_home(Some(absolute.as_os_str()), absolute.clone()).unwrap(),
            absolute
        );
    }

    /// A non-UTF-8 override is *not* what the app uses: its `var`-based read
    /// drops the value and falls back to `~/.pinvou3`, so accepting it here
    /// would point the CLI at a root the app never reads.
    #[cfg(unix)]
    #[test]
    fn sandbox_home_validation_refuses_a_non_utf8_override() {
        use std::os::unix::ffi::OsStrExt as _;
        let raw = std::ffi::OsStr::from_bytes(b"/tmp/pinvou-\xff-home");
        let error = validate_sandbox_home(Some(raw), PathBuf::from("/home/pinvou/.pinvou3"))
            .expect_err("a non-UTF-8 PINVOU3_HOME must be refused, not silently ignored");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        assert!(error.to_string().contains("not valid UTF-8"), "{error}");
    }

    /// The shared row sanitizer must cover the invisible formatting
    /// characters too, not only category Cc: a bidi override in a session
    /// title reorders the entire rendered row.
    #[test]
    fn collapse_control_characters_neutralizes_bidi_and_zero_width_characters() {
        assert_eq!(collapse_control_characters("a\tb\nc"), "a b c");
        for unsafe_char in [
            '\u{00AD}', '\u{200B}', '\u{200C}', '\u{200D}', '\u{2028}', '\u{2029}', '\u{202A}',
            '\u{202C}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{FEFF}',
        ] {
            let title = format!("ok{unsafe_char}row");
            assert_eq!(
                collapse_control_characters(&title),
                "ok row",
                "U+{:04X} must not survive into a human row",
                unsafe_char as u32
            );
        }
        // Ordinary text, including non-ASCII and combining marks, is not a
        // display hazard and must be left alone.
        assert_eq!(collapse_control_characters("会话 café ✓"), "会话 café ✓");
    }

    /// The ENV lane must distinguish "not set" from "set but not valid
    /// UTF-8", the same distinction `validate_sandbox_home` draws for
    /// `PINVOU3_HOME`: a set-but-non-UTF-8 variable is a different problem
    /// with a different fix, and the old `_ =>` collapsed both into "is not
    /// set". Pinned without the env lock: unique variable name plus a
    /// save-and-restore guard, like the sibling test below it.
    #[cfg(unix)]
    #[test]
    fn resolve_secret_env_lane_distinguishes_not_unicode_from_not_set() {
        use std::os::unix::ffi::OsStrExt as _;
        const VAR: &str = "PINVOU_CLI_TEST_RESOLVE_SECRET_NOT_UTF8";
        struct RestoreVar(Option<std::ffi::OsString>);
        impl Drop for RestoreVar {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => unsafe { std::env::set_var(VAR, value) },
                    None => unsafe { std::env::remove_var(VAR) },
                }
            }
        }
        let previous = RestoreVar(std::env::var_os(VAR));

        unsafe { std::env::set_var(VAR, std::ffi::OsStr::from_bytes(b"sk-\xff-not-utf8")) };
        let error = resolve_secret(&Some(VAR.to_owned()), false)
            .expect_err("a non-UTF-8 secret variable must be refused");
        assert_eq!(error.exit_code(), ExitCode::Failed);
        let message = error.to_string();
        assert!(
            message.contains("set but not valid UTF-8"),
            "a set-but-non-UTF-8 variable must be named as such: {message}"
        );
        assert!(
            !message.contains("is not set"),
            "misreporting a set variable as unset sends the user hunting for a missing export: \
             {message}"
        );

        unsafe { std::env::remove_var(VAR) };
        let error = resolve_secret(&Some(VAR.to_owned()), false)
            .expect_err("an unset secret variable must be refused");
        assert!(
            error.to_string().contains("is not set"),
            "an unset variable keeps its message: {error}"
        );
        drop(previous);
    }

    /// The ENV lane of [`resolve_secret`] must trim the way the stdin lane
    /// already does. Environment secrets routinely carry a trailing newline
    /// (`export KEY=$(printf '%s\n' …)` without a chomp, a file mounted by a
    /// CI secret store, most `.env` loaders); storing it verbatim put that
    /// "\n" into the credential, and every signed request 401'd while the
    /// read paths still trimmed — the failure was self-masking.
    #[test]
    fn resolve_secret_env_lane_trims_the_stored_value() {
        const VAR: &str = "PINVOU_CLI_TEST_RESOLVE_SECRET_ENV";
        // Unique name + save-and-restore guard: this is the only test in the
        // binary reading the variable, so it needs no cross-test env lock.
        struct RestoreVar(Option<std::ffi::OsString>);
        impl Drop for RestoreVar {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => unsafe { std::env::set_var(VAR, value) },
                    None => unsafe { std::env::remove_var(VAR) },
                }
            }
        }
        let previous = RestoreVar(std::env::var_os(VAR));
        unsafe { std::env::set_var(VAR, "sk-env-lane\n") };

        let stored = resolve_secret(&Some(VAR.to_owned()), false)
            .expect("an env lane with content must resolve");

        assert_eq!(
            stored,
            Some("sk-env-lane".to_owned()),
            "a trailing newline from the environment must not reach the stored credential"
        );
        drop(previous);
    }

    /// The JSON failure payload is built through serde_json, so a message
    /// carrying quotes/backslashes/newlines still parses; and it is marked
    /// as a failure instead of looking like an ordinary record.
    #[test]
    fn json_failure_payload_escapes_the_message_and_marks_the_failure() {
        let payload = json_failure_payload("bad \"quote\" \\ and \n newline");
        let parsed: serde_json::Value =
            serde_json::from_str(&payload).expect("the failure payload must be valid JSON");
        assert_eq!(parsed["ok"], serde_json::Value::Bool(false));
        assert_eq!(
            parsed["error"],
            serde_json::json!("bad \"quote\" \\ and \n newline")
        );
        assert!(
            !payload.contains('\n'),
            "the payload stays a single line: {payload}"
        );
    }

    /// A UTF-8 argv decodes unchanged, so nothing else in this contract can
    /// regress ordinary invocations.
    #[test]
    fn decode_arguments_accepts_a_utf8_argv_unchanged() {
        let raw: Vec<std::ffi::OsString> = ["pinvou", "sessions", "show", "s-1"]
            .iter()
            .map(std::ffi::OsString::from)
            .collect();
        let decoded = decode_arguments(raw).expect("a UTF-8 argv decodes");
        assert_eq!(decoded, ["pinvou", "sessions", "show", "s-1"]);
    }

    /// A non-UTF-8 program slot (`argv[0]`) is loss-converted instead of
    /// refused — `parse_args` discards that slot, and a launcher carrying a
    /// byte path must not lock the user out of every command — while the
    /// same bytes as a real argument are refused with its index. The
    /// pre-fix `to_string_lossy` produced a look-alike session id and a
    /// "not found" for an id the user never typed; that is the mutation this
    /// test pins against (a lossy decode returns `Ok` and never `Err`, so
    /// both asserts below fail without the refusal).
    #[cfg(unix)]
    #[test]
    fn decode_arguments_refuses_non_utf8_arguments_but_keeps_the_program_slot() {
        use std::os::unix::ffi::OsStrExt as _;
        let program = std::ffi::OsStr::from_bytes(b"/opt/\xff/pinvou").to_owned();
        let raw = vec![
            program,
            std::ffi::OsString::from("sessions"),
            std::ffi::OsStr::from_bytes(b"show\xff").to_owned(),
        ];
        let error = decode_arguments(raw).expect_err("a non-UTF-8 argument must be refused");
        assert_eq!(error, 2, "the offending index is the third argv slot");
        // The same non-UTF-8 bytes are fine in the program slot.
        let raw = vec![
            std::ffi::OsStr::from_bytes(b"/opt/\xff/pinvou").to_owned(),
            std::ffi::OsString::from("--version"),
        ];
        let decoded = decode_arguments(raw).expect("argv[0] is loss-converted, not refused");
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[1], "--version");
    }
}

// Child-group signal supervision, split out of this file the same way
// `support.rs` itself is a flat family module: one `pub mod` line here keeps
// the module name the families and integration tests already reach it by —
// `pinvou_cli::support` — and exposes the new API to spawn sites as
// `pinvou_cli::support::supervise` (the path the examples in
// `support/supervise.rs` document). Later waves wire spawn sites through
// that path without editing lib.rs, support.rs or main.rs again.
pub mod supervise;
