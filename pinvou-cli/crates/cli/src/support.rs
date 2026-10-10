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
projects|code|files|voice|deps|feedback|monitor|artifacts <command> | pinvou --help|--version|version";

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

/// Failure shape behind [`open_family_lock_file`], split into its two arms
/// so each family maps them onto its own stable error code and message at
/// the call site (the code.rs locks carry the round-37 `{action}_lock`
/// prefix with the path named; the connectors lock keeps its own wording) —
/// the helper deliberately does no error-code mapping of its own.
#[derive(Debug)]
pub(crate) enum FamilyLockError {
    /// The shared `locks/` directory could not be created (replaced by a
    /// file, EACCES on the parent, ...).
    CreateDir { dir: PathBuf, error: std::io::Error },
    /// The named lock file could not be opened or created.
    Open {
        path: PathBuf,
        error: std::io::Error,
    },
}

/// Opens (creating if needed) one of the `~/.pinvou3/locks/` family lock
/// files — one copy of the directory-create and open the lock sites used to
/// hand-copy, plus the permission posture the connectors lock had alone
/// (round-49 review): a lock file at the umask default (0644) is readable by
/// every local account, and `flock(LOCK_EX)` needs only READ permission, so
/// on a shared/group home any low-privilege account could hold LOCK_EX and
/// wedge the whole lane behind the family's documented blocking wait. The
/// file is therefore created at 0600 AND re-tightened to 0600 on EVERY open
/// (best-effort `set_permissions`, the same chmod-on-every-append doctrine
/// as `~/.pinvou3/cli-install.log` and the scan-QR dir), so a pre-existing
/// 0644 file from an older build heals the next time the lane opens it.
///
/// Deliberately free of the `fd_lock::RwLock` wrap: each caller keeps its
/// own blocking/try acquire and error code.
///
/// Round-50 review: the absolute-path contract is enforced HERE, not by
/// caller discipline — a relative/empty `$HOME` would have materialized a
/// cwd-relative `.pinvou3/locks/`, and two invocations from different cwds
/// would then have taken DIFFERENT lock files, silently voiding the mutual
/// exclusion the lock exists for. Every caller already ran
/// `sandbox_home()?` first, so the helper-level call is a second
/// idempotent gate (and `FamilyLockError::CreateDir` now carries the
/// sandbox refusal instead of a silently wrong lock path).
pub(crate) fn open_family_lock_file(name: &str) -> Result<std::fs::File, FamilyLockError> {
    if let Err(error) = crate::support::sandbox_home() {
        // The locks dir cannot be soundly established in a cwd-relative
        // root; the io::Error text carries the real reason so every
        // family's `{action}_lock` message prints it verbatim.
        return Err(FamilyLockError::CreateDir {
            dir: pinvou3_lib::platform::paths::pinvou3_home(),
            error: std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string()),
        });
    }
    let dir = pinvou3_lib::platform::paths::pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| FamilyLockError::CreateDir {
        dir: dir.clone(),
        error,
    })?;
    let path = dir.join(name);
    // `.mode(0o600)` makes a FRESH create private from the first instant
    // (no world-readable window between create and tighten; umask can only
    // strip bits the owner-only mode does not have). It does nothing for a
    // pre-existing file, which is what the best-effort tighten below is for.
    #[cfg(unix)]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
    };
    #[cfg(not(unix))]
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path);
    let file = opened.map_err(|error| FamilyLockError::Open {
        path: path.clone(),
        error,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Best-effort: the open above already created the file private, and
        // the lock stays usable either way (flock needs no write
        // permission), so a failing tighten is not worth failing the command
        // over — same posture as the cli-install.log append.
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(file)
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
/// therefore always exit 1, from a file and from stdin alike. `action`
/// prefixes the error; the actual convention is two-shaped by caller:
/// helper-routed resource failures carry a stable snake_case code
/// (`memory_content_file_unreadable`, `artifact_read_failed`), while
/// family-verb refusals render as human phrases (`"feedback submit"`);
/// docs/pinvou-cli.md tells scripts to match the documented per-command
/// exit codes, not message text (round-37 named the mix; round-45
/// reworded this comment to describe it).
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
    // Round-45 review: saturate so a usize::MAX cap ("unbounded") neither panics in debug nor wraps to take(0) in release.
    file.take((max_bytes as u64).saturating_add(1))
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

/// Round-43 review: read cap for the fixed-path vendor config files the CLI
/// only parses for readiness probes (codex config.toml, kimi config and
/// credentials, mcp.json). A file this size is pathological; over-cap
/// degrades exactly like a parse failure on the readiness lanes and refuses
/// on the mcp.json lane, instead of being slurped whole.
pub const VENDOR_CONFIG_READ_CAP_BYTES: usize = 16 * 1024 * 1024;

/// The byte twin of [`read_text_file_capped`] — same regular-file gate and
/// `take`-bounded read for payloads the CLI only verifies or re-emits
/// (round-38 review: the benchmark artifact lanes were the last unbounded
/// store reads, against the family contract that every file read is capped).
/// The cap is self-defense against a store file some other build or process
/// has grown, not a format limit, so callers pass their existing failure
/// code as `action` and keep their contract stable.
pub fn read_bytes_capped(path: &Path, max_bytes: usize, action: &str) -> Result<Vec<u8>, CliError> {
    // Round-43 review: same error classification as the text twin —
    // collapsing every metadata failure into "cannot inspect" sends the
    // user hunting for a typo when the real cause is EACCES on a parent
    // directory or a symlink loop; the exit class stays 1 for all of them.
    let meta = std::fs::metadata(path).map_err(|error| match error.kind() {
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
    // Round-45 review: saturate so a usize::MAX cap ("unbounded") neither panics in debug nor wraps to take(0) in release.
    file.take((max_bytes as u64).saturating_add(1))
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
    Ok(bytes)
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

/// Human-mode sanitizer for agent-authored text rendered as a BLOCK (a
/// readable file body, a transcript dump, a workspace diff) rather than as
/// one cell of a tab-separated row — the artifacts/code counterparts of the
/// same rule `sessions` applies to transcripts at the terminal boundary.
///
/// Why not [`collapse_control_characters`]: that one flattens every control
/// character including `\n` and `\t`, which is right for a single-line
/// column but would destroy the layout of the very content the caller asked
/// to read — a document legitimately spans many lines and indents code
/// blocks. So newline and tab survive, and everything else in the C0/C1
/// control range collapses to a space. What this keeps out is the
/// attacker-controlled part: ESC (terminal escape sequences — cursor
/// moves, colour, window-title rewrites, and on some terminals clipboard or
/// response injection), CR (redraws the current line), BEL, and the
/// remaining C0/DEL noise, from content the model or a tool wrote.
///
/// What the block sanitizer neutralises beyond `char::is_control`: the same
/// bidi-override/isolate and zero-width set [`is_row_unsafe_char`] carries.
/// The round-27 review found blocks kept those intact — a hostile
/// `.md`/diff rendered by `artifacts read`, `code workspace preview/diff`,
/// `agent run` or `personas show` could still visually reorder its own
/// multi-line block (Trojan-Source-style) even though the ROW sanitizer
/// already neutralised the identical set, because a row is just as
/// reorderable as a block. Newline and tab still survive: they are layout,
/// not spoofing.
fn is_block_unsafe_char(ch: char) -> bool {
    ch.is_control() || is_row_unsafe_char(ch)
}

/// JSON mode needs no equivalent — `serde_json` escapes everything below
/// 0x20 — so this stays strictly a human-rendering choice and the stored
/// file and the JSON payload keep the verbatim bytes.
pub fn collapse_block_control_characters(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\n' | '\t' => ch,
            _ if is_block_unsafe_char(ch) => ' ',
            _ => ch,
        })
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
            // The remaining Bidi_Control members: invisible, forgeable, and
            // reorder-capable in bidi-aware terminals. The GUI's set predates
            // the CLI's terminal-rows threat model; the CLI closes it here
            // (rows are fed by vendor/user titles).
            | '\u{200E}' | '\u{200F}' // LRM / RLM
            | '\u{061C}' // ARABIC LETTER MARK
            | '\u{2060}' // WORD JOINER: invisible, renders two different
                         // strings identically (the hiding hazard this
                         // module's threat model names)
            | '\u{2061}'..='\u{2064}' // INVISIBLE OPERATOR/TIMES/SEPARATOR/
                                      // PLUS: same identical-rendering hazard
            | '\u{FE00}'..='\u{FE0F}' // VARIATION SELECTORS: invisible; they
                                      // swap glyph identity between renders
            | '\u{E0001}' // LANGUAGE TAG (invisible)
            | '\u{E0020}'..='\u{E007F}' // TAG CHARACTERS: invisible payload
                                        // channel (the known Unicode smuggling
                                        // vector)
            // Round-46 review: the remaining invisible General_Category=Cf
            // members outside the ranges above — same identical-rendering /
            // digit-shaping hazard the entries above name. (U+180E is Cf
            // since Unicode 6.3.)
            | '\u{0600}'..='\u{0605}' // ARABIC NUMBER SIGN..NUMBER MARK ABOVE
            | '\u{06DD}' // ARABIC END OF AYAH
            | '\u{070F}' // SYRIAC ABBREVIATION MARK
            | '\u{08E2}' // ARABIC POUND MARK ABOVE
            | '\u{180E}' // MONGOLIAN VOWEL SEPARATOR
            | '\u{110BD}' | '\u{110CD}' // KAITHI NUMBER SIGN(S)
            | '\u{FFF9}'..='\u{FFFB}' // INTERLINEAR ANNOTATION characters
            | '\u{1BCA0}'..='\u{1BCA3}' // SHORTHAND FORMAT CONTROLS
        )
}

/// Only `[A-Za-z0-9_-]`, so an id can never traverse out of the sessions
/// root when it is joined onto a path. Shared by every family that accepts a
/// session id so the usage-error contract is uniform. Delegates to the app's
/// own validator (widened `pub` by this PR) instead of keeping a
/// second alphabet copy that could drift from it.
pub fn valid_session_id(id: &str) -> bool {
    pinvou3_lib::features::sessions::validate_session_id(id).is_ok()
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
/// Round-44 review: every flag whose value is an environment-variable NAME
/// (`--api-key-env`, `--code-env`, `--client-id-env`, `--token-env`) shares
/// this parse-time shape gate — the plugins `--secret` gate's rationale
/// verbatim. Without it a pasted literal secret rides argv (shell history,
/// process list) and only fails later as a missing variable, with the
/// pasted value echoed back in the failure diagnostics. The conventional
/// export NAME is the same shape the plugins gate enforces: letters,
/// digits, underscore, not starting with a digit; bytes outside that set
/// cannot be exported by any POSIX shell without printf tricks, so the
/// refusal is fail-fast, not a capability loss. The value is never echoed.
pub fn ensure_env_var_name(flag: &str, value: &str) -> Result<(), CliError> {
    let name_is_sound = value.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !name_is_sound {
        return Err(CliError::usage(format!(
            "{flag} must be an environment variable NAME (letters, digits, underscore, not \
             starting with a digit) — the secret itself never belongs on argv"
        )));
    }
    Ok(())
}

pub fn resolve_secret(
    api_key_env: &Option<String>,
    api_key_stdin: bool,
) -> Result<Option<String>, CliError> {
    if let Some(var) = api_key_env.as_deref() {
        // Belt for direct callers that bypass the parse-time gate: the
        // not-set/empty errors below interpolate the value, so a pasted
        // literal must never reach them.
        ensure_env_var_name("--api-key-env", var)?;
    }
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

/// Builds a `Command` for a resolved vendor CLI path. Windows batch-file
/// targets (`.cmd`/`.bat` shims, e.g. npm-installed CLIs) are passed to
/// `std::process::Command` as-is: std detects batch files at spawn, runs
/// them through `cmd.exe` itself, and applies the hardened batch-specific
/// argument quoting from the BatBadBut fix (rust-version here is >= 1.89).
/// Wrapping in `cmd /D /S /C` manually would spawn `cmd` as a plain
/// executable — std's batch escaping would not apply, cmd metacharacters in
/// any argument become live (`a&calc`, `%PATH%`), and `/S` quote-stripping
/// mangles a spaced shim path combined with a spaced argument. The
/// app-side `platform::process` helper still carries the manual wrap shape;
/// converging both on the std-native path upstream is recorded as
/// follow-up work.
pub fn build_command(executable: &std::path::Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(executable);
    command.args(args);
    command
}

#[cfg(all(test, target_os = "windows"))]
mod windows_batch_tests {
    /// The batch shim must reach `std::process::Command` as the program
    /// itself — never pre-wrapped through `cmd` — so std's hardened batch
    /// quoting applies (a regression re-introducing the manual
    /// `cmd /D /S /C` wrapper fails this).
    #[test]
    fn build_command_spawns_batch_shims_directly_for_std_hardening() {
        let shim = std::path::Path::new(r"C:\tools\vendor\cli.cmd");
        let command = super::build_command(shim, &["--flag", "value"]);
        assert_eq!(command.get_program(), shim);
    }
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
        // Resolved through the app's own kill path, consumed via the
        // targeted `pinvou3_lib::platform::external_command` re-export (the
        // `process` module itself stays crate-private). The point is
        // resolution parity: the same bare name resolves exactly as it does
        // for the GUI's own kill_process_tree — this lane deliberately does
        // not invent a stricter PATH policy the GUI does not have — and the
        // hidden-window wrapping matches the app's detached spawn. The 2s
        // budget is the part that matters here and is reproduced directly. A
        // wedged WMI/RPC must not stall the caller's own timeout path, so
        // taskkill itself is killed when its budget expires. Null stdio
        // keeps the helper's streams off ours (a bare spawn would inherit
        // them), like the app's detached spawn.
        const TASKKILL_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
        let spawned = pinvou3_lib::platform::external_command(std::path::Path::new("taskkill"))
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut taskkill) = spawned {
            let deadline = std::time::Instant::now() + TASKKILL_BUDGET;
            loop {
                match taskkill.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        let _ = taskkill.kill();
                        let _ = taskkill.wait();
                        break;
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
        } else {
            // Round-43 review: a taskkill that never ran (not on PATH, AV
            // interference) must not be silent — the direct child kill below
            // still lands, but the descendants survive with no trace of why.
            crate::note!(
                "pinvou: warning: could not spawn taskkill for process-group cleanup of pid {}; \
                 descendant processes may outlive this run",
                child.id()
            );
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The shared bounded pipe drain, stated once: KEEP the first `cap + 1`
/// bytes of one stream, but keep reading — and DISCARDING — everything past
/// the cap until true EOF, and report the total bytes seen. Draining past
/// the cap is the load-bearing half of the invariant: a `take(cap)` that
/// simply stops reading leaves the write end full, so a child that
/// outproduces the cap blocks in `write()` forever instead of finishing.
/// The keep window is `cap + 1` (not `cap`) so a caller can hold one byte
/// past its reporting cap, and `total` is the truncation signal
/// (`total > cap` means the stream was cut). A missing pipe (`None` — the
/// child died before the fd could be taken) drains to an empty buffer.
///
/// Moved verbatim from code.rs's `read_capped_to_eof` (round-50 review):
/// the loop math, the `cap + 1` accounting and the `(kept, total)` return
/// are byte-for-byte the code.rs original; the `Option`-tolerant pipe is
/// the defensive piece reconciled in from voice.rs's local copy. voice.rs
/// and code.rs call this; connectors.rs keeps its own exact-cap variant.
pub fn drain_capped_to_eof<R: std::io::Read>(pipe: Option<R>, cap: u64) -> (Vec<u8>, u64) {
    let mut pipe = match pipe {
        Some(pipe) => pipe,
        None => return (Vec::new(), 0),
    };
    let mut kept = Vec::new();
    let mut total: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if (kept.len() as u64) <= cap {
                    let remaining = (cap + 1 - kept.len() as u64) as usize;
                    kept.extend_from_slice(&chunk[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    (kept, total)
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

    /// Pins the moved drain primitive at its new home: the `cap + 1` keep
    /// window, the total-bytes signal, and the `None` arm — the contract
    /// code.rs's `read_capped_to_eof` carried before the hoist. The
    /// drain-past-cap behavior itself is pinned end-to-end by voice.rs's
    /// `drain_capped_keeps_the_cap_but_still_reads_to_eof` and the
    /// code_contract diff tests.
    #[test]
    fn drain_capped_to_eof_keeps_cap_plus_one_and_reports_total() {
        struct Fixed(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for Fixed {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                // A small chunk size exercises the keep-window arithmetic
                // across several reads instead of one.
                let take = buf.len().min(7);
                self.0.read(&mut buf[..take])
            }
        }
        let payload = vec![b'x'; 100];
        let (kept, total) = drain_capped_to_eof(Some(Fixed(std::io::Cursor::new(payload))), 40);
        assert_eq!(kept.len(), 41, "the keep window is cap + 1");
        assert_eq!(total, 100, "total counts everything drained to EOF");
        let (kept, total) = drain_capped_to_eof(None::<std::io::Empty>, 40);
        assert_eq!((kept.len(), total), (0, 0), "a missing pipe drains empty");
    }

    #[test]
    fn block_sanitizer_neutralizes_bidi_and_zero_width_like_rows() {
        // The row set (bidi overrides/isolates, zero-width, soft hyphen,
        // BOM) applies to blocks too: a hostile document can visually
        // reorder its own multi-line block exactly like it reorders a row.
        let hostile = "line\u{202E}spoof\u{202C}mid\u{2066}iso\u{2069}\u{200B}\u{FEFF}";
        let cleaned = collapse_block_control_characters(hostile);
        assert_eq!(cleaned, "line spoof mid iso   ");
        // The invisible-operator/variation-selector/tag ranges join the set
        // with the same identical-rendering rationale as U+2060.
        let hostile2 = "x\u{2061}y\u{FE0F}z\u{E0001}\u{E0020}w\u{E007F}";
        assert_eq!(
            collapse_control_characters(hostile2),
            "x y z  w ",
            "invisible operators, variation selectors, and tag characters must not survive"
        );
        // Layout survives: newline and tab are structure, not spoofing.
        assert_eq!(collapse_block_control_characters("a\n\tb"), "a\n\tb");
        // The row sanitizer keeps flattening layout, as before.
        assert_eq!(collapse_control_characters("a\nb"), "a b");
    }
    use super::{
        ENV_LOCK, FamilyLockError, collapse_block_control_characters, collapse_control_characters,
        decode_arguments, drain_capped_to_eof, emit_report, json_failure_payload,
        open_family_lock_file, read_bytes_capped, read_text_file_capped, resolve_secret,
        validate_sandbox_home,
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
            '\u{202C}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{FEFF}', '\u{200E}', '\u{200F}',
            '\u{061C}', '\u{2060}',
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

    /// Round-46 review: the remaining invisible General_Category=Cf members
    /// (Arabic/Khmer number signs, the Mongolian vowel separator, Kaithi
    /// number signs, interlinear annotation, shorthand format controls) are
    /// in the same identical-rendering class the ranges above neutralize;
    /// each must collapse in both the row and the block sanitizer.
    #[test]
    fn collapse_control_characters_neutralizes_the_remaining_cf_members() {
        for unsafe_char in [
            '\u{0600}',
            '\u{0605}',
            '\u{06DD}',
            '\u{070F}',
            '\u{08E2}',
            '\u{180E}',
            '\u{110BD}',
            '\u{110CD}',
            '\u{FFF9}',
            '\u{FFFB}',
            '\u{1BCA0}',
            '\u{1BCA3}',
        ] {
            let title = format!("ok{unsafe_char}row");
            assert_eq!(
                collapse_control_characters(&title),
                "ok row",
                "U+{:04X} must not survive into a human row",
                unsafe_char as u32
            );
            assert_eq!(
                collapse_block_control_characters(&title),
                "ok row",
                "U+{:04X} must not survive into a rendered block",
                unsafe_char as u32
            );
        }
    }

    /// The ENV lane must distinguish "not set" from "set but not valid
    /// UTF-8", the same distinction `validate_sandbox_home` draws for
    /// `PINVOU3_HOME`: a set-but-non-UTF-8 variable is a different problem
    /// with a different fix, and the old `_ =>` collapsed both into "is not
    /// set". Round-42 review: held under `ENV_LOCK` like every other
    /// env-mutating test in this binary — a unique variable name defeats
    /// logical interference, not the `setenv`×`getenv` data race the
    /// `unsafe` calls exist for, and sibling tests read the process env
    /// concurrently under the same lock.
    #[cfg(unix)]
    #[test]
    fn resolve_secret_env_lane_distinguishes_not_unicode_from_not_set() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
    /// read paths still trimmed — the failure was self-masking. Held under
    /// `ENV_LOCK` (round-42 review): same data-race reasoning as the
    /// sibling test above.
    #[test]
    fn resolve_secret_env_lane_trims_the_stored_value() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        const VAR: &str = "PINVOU_CLI_TEST_RESOLVE_SECRET_ENV";
        // Unique name + save-and-restore guard keeps the variable itself
        // isolated; the lock serializes the env writes against every other
        // test in this binary.
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

    /// Round-45 review: a usize::MAX cap means "read the whole file" — the
    /// old `max_bytes as u64 + 1` overflowed there (debug panic, release
    /// wrap to take(0), i.e. a silent empty read). Reverting the
    /// `saturating_add` fails this test (or panics under debug assertions).
    #[test]
    fn capped_readers_treat_usize_max_as_unbounded_instead_of_overflowing() {
        let dir = std::env::temp_dir().join(format!(
            "pinvou-support-cap-max-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.txt");
        std::fs::write(&path, b"small payload").unwrap();

        let text = read_text_file_capped(&path, usize::MAX, "probe_failed")
            .expect("a usize::MAX cap must read the file, not overflow");
        assert_eq!(
            text, "small payload",
            "the full content must come back, not an empty read"
        );
        let bytes = read_bytes_capped(&path, usize::MAX, "probe_failed")
            .expect("a usize::MAX cap must read the file, not overflow");
        assert_eq!(bytes, b"small payload");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Round-49: the family lock helper creates a FRESH lock file at 0600.
    /// The mode read back is the full 0600 regardless of the process umask:
    /// umask can only strip bits, and the owner-only mode has no group/other
    /// bits to strip. Held under `ENV_LOCK` like every other env-mutating
    /// test in this binary — the helper resolves the root through
    /// `pinvou3_home()`, which reads `PINVOU3_HOME`.
    #[cfg(unix)]
    #[test]
    fn family_lock_helper_creates_a_fresh_lock_at_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou-support-lock-fresh-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        struct RestoreHome(Option<std::ffi::OsString>, PathBuf);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _home = RestoreHome(std::env::var_os("PINVOU3_HOME"), root.clone());
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        let file = open_family_lock_file("contract-fresh.lock")
            .expect("a fresh lock file must open in a writable home");
        let path = root.join("locks").join("contract-fresh.lock");
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o777,
            0o600,
            "a freshly created lock file must be owner-only from the start"
        );
        assert!(path.is_file(), "the lock lands under the shared locks dir");
    }

    /// Round-49: a PRE-EXISTING lock file left at the umask default (0644,
    /// e.g. by an older build) is tightened to 0600 by the next open — the
    /// heal-on-every-open half of the contract, mirroring the
    /// cli-install.log chmod-on-every-append doctrine.
    #[cfg(unix)]
    #[test]
    fn family_lock_helper_tightens_a_preexisting_0644_lock_on_open() {
        use std::os::unix::fs::PermissionsExt as _;
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou-support-lock-heal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("locks")).unwrap();
        struct RestoreHome(Option<std::ffi::OsString>, PathBuf);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _home = RestoreHome(std::env::var_os("PINVOU3_HOME"), root.clone());
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        let path = root.join("locks").join("contract-heal.lock");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let file = open_family_lock_file("contract-heal.lock")
            .expect("an existing 0644 lock must still open");
        assert_eq!(
            file.metadata().unwrap().permissions().mode() & 0o777,
            0o600,
            "a pre-existing 0644 lock file must be tightened to 0600 on open"
        );
    }

    /// The error seam keeps its two arms distinct so each family can render
    /// its own stable message: a file standing in for the `locks/` directory
    /// fails the dir-create arm (with the directory path attached), not the
    /// open arm.
    #[test]
    fn family_lock_helper_reports_the_dir_create_arm_with_its_path() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pinvou-support-lock-dirarm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        struct RestoreHome(Option<std::ffi::OsString>, PathBuf);
        impl Drop for RestoreHome {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
                    None => unsafe { std::env::remove_var("PINVOU3_HOME") },
                }
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let _home = RestoreHome(std::env::var_os("PINVOU3_HOME"), root.clone());
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };

        // A FILE at the locks path: create_dir_all fails with something
        // other than success, and the error must name the DIRECTORY.
        std::fs::write(root.join("locks"), b"not a directory").unwrap();
        let error = open_family_lock_file("contract-dir-arm.lock")
            .expect_err("a file at the locks path must fail the create arm");
        match error {
            FamilyLockError::CreateDir { dir, .. } => {
                assert_eq!(
                    dir,
                    root.join("locks"),
                    "the dir arm must carry the directory path"
                );
            }
            FamilyLockError::Open { .. } => {
                panic!("a file standing in for locks/ must be the CreateDir arm, not Open")
            }
        }
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
