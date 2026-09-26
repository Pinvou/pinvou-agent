//! `memory` family: GUI-parity surface over `pinvou3_lib::features::memory`,
//! mapped one-to-one onto the GUI commands in
//! `pinvou3-app/src-tauri/src/app/commands/memory.rs`.
//!
//! Every subcommand except `memory organize` is pure storage and calls the
//! feature io layer directly (no host boot). `memory organize` boots the
//! windowless product host via `pinvou3_lib::headless_bridge::
//! run_windowless_host` (the same bootstrap `run_with_product_backend` wraps)
//! and calls `organize_memory_with_llm` exactly like the GUI command and the
//! scheduled memory-organize executor: it needs a display (xvfb on headless
//! Linux) and a configured, active model.
//!
//! Known deviation from the GUI bridge resolution: the GUI prefers
//! `pool.fresh_bridge_for(active_session)` (session-bound model plus runtime
//! credential preparation), but that method is `pub(crate)` to `pinvou3_lib`,
//! so the CLI always organizes with the shared pool bridge refreshed from the
//! current global prefs — the GUI's own no-active-session fallback.
//!
//! Cross-process caveat: `memory add`/`update`/`delete`/`pending`/`organize`
//! rewrite stores a live GUI may be writing at the same time. The feature
//! layer serializes its writes behind a process-local mutex
//! (`features/memory/io.rs` `write_lock`) that a separate CLI process cannot
//! see, so the last writer wins and the desktop app's newest changes can be
//! lost — avoid memory mutations while the desktop app is actively writing
//! memory (same caveat as the `sessions` family header).
//!
//! Snapshot rewrite caveat: `memory overview` and `memory organize` refresh
//! `snapshot.md` through the feature layer with `runtime: None` (a one-shot
//! CLI never owns the desktop app's active session — see the overview
//! command), so the rewritten document carries no `runtime` section. Running
//! either command concurrently while the desktop app has an active session
//! temporarily drops that section from `snapshot.md` until the app next
//! refreshes it; avoid `overview`/`organize` while the app is displaying
//! memory for a live session. Because `overview` otherwise reads like a
//! read-only command, it discloses the rewrite at the point of action — on
//! stderr and on its own output (`snapshot_rewritten_without_runtime` in
//! JSON) — so a caller polling it in a loop cannot degrade the app's snapshot
//! silently.
//!
//! Replace-per-topic note: `memory add preference` and `memory add
//! work-context` do not append. Both stores are organized into topic buckets
//! and the write deletes the bucket's previous item, and the CLI adds without
//! a topic, so every add replaces the previous CLI add's item.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pinvou3_lib::features::memory as feature;
use pinvou3_lib::features::memory::{
    MemoryOrganizeReport, MemorySuggestion, MemoryTextPatch, PendingIgnoreOutcome, PreferenceFile,
    RecentWorkItem, TimedMemoryItem, WorkContextFile,
};

use crate::support::{self, render, require_yes, success};
use crate::{CliError, CliOutcome, OutputMode};

const MEMORY_USAGE: &str = "usage: pinvou memory <overview|profile|list|add|update|delete|\
archive|pending|organize|organize-history>\n\
note: adding to preferences/work-context replaces the previous item in that topic bucket";

/// Disclosure for the one mutation `memory overview` performs. The command
/// reads like a read-only summary but rewrites the shared `snapshot.md`, and
/// with `runtime: None` it drops the runtime section the desktop app wrote for
/// its active session (see the module header). A caller polling `overview` in
/// a loop must not degrade the app's snapshot silently, so the rewrite is
/// reported on the command's own output as well as on stderr.
const OVERVIEW_SNAPSHOT_REWRITE_NOTE: &str = "Note: this overview rewrote snapshot.md without \
its runtime section (a one-shot CLI owns no active session); the desktop app restores that \
section on its next refresh";

/// Stores addressable by `memory list|update|delete`. Hyphenated values are
/// canonical, the GUI's underscore spellings are accepted as aliases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryStore {
    Preferences,
    WorkContext,
    CurrentFocus,
    RecentActivity,
    RecentWork,
    Pending,
}

impl MemoryStore {
    fn parse_value(value: &str) -> Result<Self, CliError> {
        match value {
            "preferences" => Ok(Self::Preferences),
            "work-context" | "work_context" => Ok(Self::WorkContext),
            "current-focus" | "current_focus" => Ok(Self::CurrentFocus),
            "recent-activity" | "recent_activity" => Ok(Self::RecentActivity),
            "recent-work" | "recent_work" => Ok(Self::RecentWork),
            "pending" => Ok(Self::Pending),
            other => Err(CliError::usage(format!(
                "unknown memory store '{other}' (valid: preferences, work-context, \
current-focus, recent-activity, recent-work, pending)"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Preferences => "preferences",
            Self::WorkContext => "work_context",
            Self::CurrentFocus => "current_focus",
            Self::RecentActivity => "recent_activity",
            Self::RecentWork => "recent_work",
            Self::Pending => "pending",
        }
    }

    /// Stores whose items support text update/delete; recent-work only
    /// archives and pending items are resolved through `memory pending`.
    fn supports_content_edit(self) -> bool {
        matches!(
            self,
            Self::Preferences | Self::WorkContext | Self::CurrentFocus | Self::RecentActivity
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddKind {
    Preference,
    WorkContext,
}

impl AddKind {
    fn parse_value(value: &str) -> Result<Self, CliError> {
        match value {
            "preference" => Ok(Self::Preference),
            "work-context" | "work_context" => Ok(Self::WorkContext),
            other => Err(CliError::usage(format!(
                "unknown memory add kind '{other}' (valid: preference, work-context)"
            ))),
        }
    }

    /// The `features::memory` suggestion kind the GUI pipeline uses.
    fn feature_kind(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::WorkContext => "work_context",
        }
    }
}

/// Content comes inline (`--content` or trailing arguments joined) or from a
/// file (`--file`); file reading happens at execute time so `parse` stays pure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddSource {
    Inline(String),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAction {
    Confirm,
    Ignore,
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryCommand {
    Overview,
    ProfileGet,
    ProfileSet {
        call_name: Option<String>,
        assistant_alias: Option<String>,
    },
    List {
        store: Option<MemoryStore>,
    },
    Add {
        kind: AddKind,
        source: AddSource,
    },
    Update {
        store: MemoryStore,
        id: String,
        content: String,
    },
    Delete {
        store: MemoryStore,
        id: String,
        confirmed: bool,
    },
    Archive {
        id: String,
    },
    Pending {
        action: PendingAction,
        id: String,
        reason: Option<String>,
    },
    Organize {
        confirmed: bool,
    },
    OrganizeHistory,
}

pub fn parse(values: &[String]) -> Result<MemoryCommand, CliError> {
    let subcommand = values.get(1).ok_or_else(|| CliError::usage(MEMORY_USAGE))?;
    match subcommand.as_str() {
        "overview" => {
            expect_no_arguments(&values[2..], "memory overview")?;
            Ok(MemoryCommand::Overview)
        }
        "profile" => parse_profile(&values[2..]),
        "list" => parse_list(&values[2..]),
        "add" => parse_add(&values[2..]),
        "update" => parse_update(&values[2..]),
        "delete" => parse_delete(&values[2..]),
        "archive" => {
            let id = values
                .get(2)
                .cloned()
                .ok_or_else(|| CliError::usage("usage: pinvou memory archive <id>"))?;
            expect_no_arguments(&values[3..], "memory archive")?;
            Ok(MemoryCommand::Archive { id })
        }
        "pending" => parse_pending(&values[2..]),
        "organize" => {
            // organize rewrites the stores under LLM decisions — a
            // destructive action like `memory delete`, so it opts in the
            // same way.
            let options = parse_options(&values[2..], &[], &["--yes"])?;
            options.ensure_no_positionals("memory organize")?;
            Ok(MemoryCommand::Organize {
                confirmed: options.has_flag("--yes"),
            })
        }
        "organize-history" => {
            expect_no_arguments(&values[2..], "memory organize-history")?;
            Ok(MemoryCommand::OrganizeHistory)
        }
        other => Err(CliError::usage(format!(
            "unknown memory command '{other}'; {MEMORY_USAGE}"
        ))),
    }
}

fn parse_profile(values: &[String]) -> Result<MemoryCommand, CliError> {
    let action = values.first().ok_or_else(|| {
        CliError::usage(
            "usage: pinvou memory profile <get|set> [--call-name N] [--assistant-alias A]",
        )
    })?;
    match action.as_str() {
        "get" => {
            expect_no_arguments(&values[1..], "memory profile get")?;
            Ok(MemoryCommand::ProfileGet)
        }
        "set" => {
            let options = parse_options(&values[1..], &["--call-name", "--assistant-alias"], &[])?;
            // `profile set --call-name X junk` must not write X and drop
            // `junk`: the stray token is almost always a name the caller
            // forgot to quote, so writing the truncated half silently is the
            // worst outcome available.
            options.ensure_no_positionals("memory profile set")?;
            let call_name = options.value("--call-name");
            let assistant_alias = options.value("--assistant-alias");
            if call_name.is_none() && assistant_alias.is_none() {
                return Err(CliError::usage(
                    "memory profile set requires --call-name and/or --assistant-alias",
                ));
            }
            Ok(MemoryCommand::ProfileSet {
                call_name: call_name.map(str::to_owned),
                assistant_alias: assistant_alias.map(str::to_owned),
            })
        }
        other => Err(CliError::usage(format!(
            "unknown memory profile action '{other}' (valid: get, set)"
        ))),
    }
}

fn parse_list(values: &[String]) -> Result<MemoryCommand, CliError> {
    let options = parse_options(values, &["--store"], &[])?;
    // `memory list preferences` is the spelling a user reaches for before
    // discovering `--store`; dropping the token made it print all six stores
    // and exit 0, so the command answered a question nobody asked instead of
    // naming the option.
    options.ensure_no_positionals("memory list")?;
    let store = match options.value("--store") {
        Some(value) => Some(MemoryStore::parse_value(value)?),
        None => None,
    };
    Ok(MemoryCommand::List { store })
}

fn parse_add(values: &[String]) -> Result<MemoryCommand, CliError> {
    let kind = values.first().ok_or_else(|| {
        CliError::usage("usage: pinvou memory add <preference|work-context> --content S")
    })?;
    let kind = AddKind::parse_value(kind)?;
    let options = parse_options(&values[1..], &["--content", "--file"], &[])?;
    let inline = options.value("--content");
    let file = options.value("--file");
    let source = match (inline, file) {
        (Some(_), Some(_)) => {
            return Err(CliError::usage(
                "memory add accepts one of --content or --file",
            ));
        }
        (Some(content), None) => {
            if !options.positional.is_empty() {
                return Err(CliError::usage(
                    "memory add --content does not take positional arguments",
                ));
            }
            AddSource::Inline(content.to_owned())
        }
        (None, Some(path)) => {
            // Same guard as the `--content` branch above: with `--file` the
            // trailing words are not the content, so storing the file and
            // discarding `extra words` would silently answer a different
            // command than the one that was typed.
            options.ensure_no_positionals("memory add --file")?;
            AddSource::File(PathBuf::from(path))
        }
        (None, None) => {
            if options.positional.is_empty() {
                return Err(CliError::usage(
                    "memory add requires --content S, --file PATH, or content arguments",
                ));
            }
            AddSource::Inline(options.positional.join(" "))
        }
    };
    Ok(MemoryCommand::Add { kind, source })
}

fn parse_update(values: &[String]) -> Result<MemoryCommand, CliError> {
    let store = values
        .first()
        .ok_or_else(|| CliError::usage("usage: pinvou memory update <store> <id> --content S"))?;
    let store = parse_editable_store(store, "update")?;
    let id = values
        .get(1)
        .cloned()
        .ok_or_else(|| CliError::usage("memory update requires <store> <id>"))?;
    let options = parse_options(&values[2..], &["--content"], &[])?;
    options.ensure_no_positionals("memory update")?;
    let content = options
        .value("--content")
        .ok_or_else(|| CliError::usage("memory update requires --content S"))?
        .to_owned();
    Ok(MemoryCommand::Update { store, id, content })
}

fn parse_delete(values: &[String]) -> Result<MemoryCommand, CliError> {
    let store = values
        .first()
        .ok_or_else(|| CliError::usage("usage: pinvou memory delete <store> <id> --yes"))?;
    let store = parse_editable_store(store, "delete")?;
    let id = values
        .get(1)
        .cloned()
        .ok_or_else(|| CliError::usage("memory delete requires <store> <id>"))?;
    let options = parse_options(&values[2..], &[], &["--yes"])?;
    options.ensure_no_positionals("memory delete")?;
    Ok(MemoryCommand::Delete {
        store,
        id,
        confirmed: options.has_flag("--yes"),
    })
}

fn parse_pending(values: &[String]) -> Result<MemoryCommand, CliError> {
    let action = values.first().ok_or_else(|| {
        CliError::usage("usage: pinvou memory pending <confirm|ignore|never> <id> [--reason R]")
    })?;
    let action = match action.as_str() {
        "confirm" => PendingAction::Confirm,
        "ignore" => PendingAction::Ignore,
        "never" => PendingAction::Never,
        other => {
            return Err(CliError::usage(format!(
                "unknown memory pending action '{other}' (valid: confirm, ignore, never)"
            )));
        }
    };
    let id = values
        .get(1)
        .cloned()
        .ok_or_else(|| CliError::usage("memory pending requires <id>"))?;
    let options = parse_options(&values[2..], &["--reason"], &[])?;
    // `pending never ID my reason` (the `--reason` flag forgotten) used to
    // resolve the candidate with an EMPTY reason and drop the words — a
    // destructive decision recorded without the justification the caller
    // typed, so the stray tokens are a usage error here too.
    options.ensure_no_positionals("memory pending")?;
    Ok(MemoryCommand::Pending {
        action,
        id,
        reason: options.value("--reason").map(str::to_owned),
    })
}

/// `update`/`delete` only apply to stores with per-item content editing;
/// recent-work is archive-only and pending items are resolved, both rejected
/// at parse time with the supported alternative.
fn parse_editable_store(value: &str, action: &str) -> Result<MemoryStore, CliError> {
    let store = MemoryStore::parse_value(value)?;
    if store.supports_content_edit() {
        return Ok(store);
    }
    match store {
        MemoryStore::RecentWork => Err(CliError::usage(format!(
            "memory {action} does not support store 'recent-work'; recent work is \
archive-only: pinvou memory archive <id>"
        ))),
        MemoryStore::Pending => Err(CliError::usage(format!(
            "memory {action} does not support store 'pending'; resolve pending items: \
pinvou memory pending confirm|ignore|never <id>"
        ))),
        _ => unreachable!("editable stores are filtered above"),
    }
}

fn expect_no_arguments(values: &[String], command: &str) -> Result<(), CliError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(CliError::usage(format!(
            "{command} accepts no options or arguments"
        )))
    }
}

/// Trailing-option parser in the style of the benchmark family: valued
/// `--name value` pairs (each at most once), bare flags, and positional
/// arguments; unknown options are rejected.
struct ParsedOptions {
    valued: Vec<(String, String)>,
    flags: Vec<String>,
    positional: Vec<String>,
}

impl ParsedOptions {
    /// Option-only commands must not silently drop stray tokens.
    fn ensure_no_positionals(&self, usage: &str) -> Result<(), CliError> {
        if self.positional.is_empty() {
            Ok(())
        } else {
            Err(CliError::usage(format!(
                "{usage} takes no positional arguments (got {})",
                self.positional.join(" ")
            )))
        }
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.valued
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn parse_options(
    values: &[String],
    allowed_valued: &[&str],
    allowed_flags: &[&str],
) -> Result<ParsedOptions, CliError> {
    let mut options = ParsedOptions {
        valued: Vec::new(),
        flags: Vec::new(),
        positional: Vec::new(),
    };
    let mut index = 0;
    while index < values.len() {
        let token = values[index].as_str();
        if token.starts_with("--") {
            if allowed_flags.contains(&token) {
                // Duplicate boolean flags are a usage error here for the same
                // reason they are in `support::parse_family_flags`: repeating
                // a flag almost always means the command line was assembled
                // wrongly (a script appending `--yes` to an argv that already
                // had one), and for a destructive family that silently
                // accepting it would be the difference between refusing and
                // deleting. Without this check `memory delete X --yes --yes`
                // succeeded while `models remove X --yes --yes` exited 2,
                // which made the "duplicate flags are rejected" contract a
                // per-family accident instead of a rule.
                if options.has_flag(token) {
                    return Err(CliError::usage(format!("duplicate option {token}")));
                }
                options.flags.push(token.to_owned());
                index += 1;
                continue;
            }
            if allowed_valued.contains(&token) {
                let value = values
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage(format!("{token} requires a value")))?
                    .clone();
                // An empty value is rejected alongside a flag-looking one:
                // `--store ""` or `--topic ""` is a missing value that the
                // shell expanded from an unset variable, not a request to
                // address the empty-named item. Letting it through handed the
                // store a blank id/topic to resolve, which is either a
                // confusing host failure or — worse — a match on whatever the
                // store normalizes the empty string to. Mirrors the
                // `value.is_empty()` guard in `support::parse_family_flags`.
                if value.is_empty() || value.starts_with("--") {
                    return Err(CliError::usage(format!("{token} requires a value")));
                }
                if options.value(token).is_some() {
                    return Err(CliError::usage(format!("duplicate option {token}")));
                }
                options.valued.push((token.to_owned(), value));
                index += 2;
                continue;
            }
            return Err(CliError::usage(format!(
                "unknown option '{token}' for pinvou memory"
            )));
        }
        options.positional.push(token.to_owned());
        index += 1;
    }
    Ok(options)
}

pub fn execute(command: MemoryCommand, output: OutputMode) -> Result<CliOutcome, CliError> {
    match command {
        MemoryCommand::Overview => overview(output),
        MemoryCommand::ProfileGet => profile_get(output),
        MemoryCommand::ProfileSet {
            call_name,
            assistant_alias,
        } => profile_set(call_name, assistant_alias, output),
        MemoryCommand::List { store } => list(store, output),
        MemoryCommand::Add { kind, source } => add(kind, source, output),
        MemoryCommand::Update { store, id, content } => update(store, &id, &content, output),
        MemoryCommand::Delete {
            store,
            id,
            confirmed,
        } => delete(store, &id, confirmed, output),
        MemoryCommand::Archive { id } => archive(&id, output),
        MemoryCommand::Pending { action, id, reason } => pending(action, &id, reason, output),
        MemoryCommand::Organize { confirmed } => {
            require_yes(confirmed)?;
            organize(output)
        }
        MemoryCommand::OrganizeHistory => organize_history(output),
    }
}

fn feature_error(action: &str, error: std::io::Error) -> CliError {
    CliError::failed(format!("memory_{action}_failed: {error}"))
}

fn not_found(store: MemoryStore, id: &str) -> CliError {
    CliError::failed(format!("{}_not_found: {id}", store.as_str()))
}

/// The eight authoritative memory sources loaded through the shared
/// per-source diagnostics, plus the warning list and the per-source
/// availability map: the single loading path behind `overview` and the
/// post-organize snapshot refresh, so the two cannot drift apart (the GUI
/// keeps the same helper: `load_memory_sources`).
struct LoadedMemorySources {
    profile: feature::MemoryProfile,
    preferences: Vec<feature::PreferenceFile>,
    work_context: Vec<feature::WorkContextFile>,
    current_focus: Vec<feature::TimedMemoryItem>,
    recent_activity: Vec<feature::TimedMemoryItem>,
    recent_work: Vec<feature::RecentWorkItem>,
    pending: Vec<feature::PendingMemoryItem>,
    never: Vec<feature::NeverMemoryItem>,
    warnings: Vec<serde_json::Value>,
    sources: BTreeMap<String, serde_json::Value>,
}

fn load_memory_sources() -> LoadedMemorySources {
    let mut warnings = Vec::new();
    let mut sources = BTreeMap::new();
    let profile = loaded_source(
        "profile",
        feature::load_profile(),
        &mut warnings,
        &mut sources,
    );
    let preferences = loaded_topic_source(
        "preferences",
        feature::list_preferences_with_cleanup(),
        &mut warnings,
        &mut sources,
    );
    let work_context = loaded_topic_source(
        "work_context",
        feature::load_work_context_with_cleanup(),
        &mut warnings,
        &mut sources,
    );
    let current_focus = loaded_source(
        "current_focus",
        feature::load_current_focus(),
        &mut warnings,
        &mut sources,
    );
    let recent_activity = loaded_source(
        "recent_activity",
        feature::load_recent_activity(),
        &mut warnings,
        &mut sources,
    );
    let recent_work = loaded_source(
        "recent_work",
        feature::load_recent_work(),
        &mut warnings,
        &mut sources,
    );
    let pending = loaded_source(
        "pending",
        feature::load_pending_memory(),
        &mut warnings,
        &mut sources,
    );
    let never = loaded_source(
        "never",
        feature::load_never_memory(),
        &mut warnings,
        &mut sources,
    );
    LoadedMemorySources {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        warnings,
        sources,
    }
}

fn overview(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let LoadedMemorySources {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        mut warnings,
        mut sources,
    } = load_memory_sources();
    // Runtime prompt: the GUI overview renders the ACTIVE session's cached
    // runtime memory, but `SessionStore::active_id()` is process-local state —
    // a one-shot CLI process never owns the desktop app's active session — so
    // the previous `boot() + active_id()` resolution here was a dead branch
    // and "Runtime" was always "none". Keep the GUI's own no-active-session
    // arm: the runtime source reports available, nothing is rendered, and the
    // snapshot document below is written without a runtime section. The key
    // stays in the JSON as an always-null value so the shape is stable.
    mark_source(&mut sources, "runtime", true, None);
    // Same gate as the GUI overview: refresh snapshot.md only when every
    // authoritative source is available, otherwise defer (a partial read must
    // not wipe that category from the snapshot document).
    let mut snapshot_failed = false;
    let snapshot_path = if sources.values().all(available) {
        match feature::write_memory_snapshot_document(
            &profile,
            &preferences,
            &work_context,
            &current_focus,
            &recent_activity,
            &recent_work,
            &pending,
            &never,
            // No runtime section: see the runtime source note above — a
            // one-shot CLI process has no active session to render a prompt
            // for, which is exactly what the GUI passes when none is open.
            None,
        ) {
            Ok(path) => {
                mark_source(&mut sources, "snapshot", true, None);
                // Disclosure at the point of action: `overview` reads like a
                // read-only command but rewrites the shared snapshot.md, and
                // with `runtime: None` it drops the runtime section the
                // desktop app wrote for its active session. The behaviour is
                // deliberate (see the module header), yet a caller polling
                // `overview` in a loop would otherwise keep degrading the
                // app's snapshot with nothing anywhere saying so. Also
                // reported on stdout/JSON below, because stderr is exactly
                // what such a loop discards.
                note!(
                    "[memory] snapshot_rewritten_without_runtime snapshot: {} was rewritten \
without the runtime section (a one-shot CLI owns no active session); the desktop app restores \
it on its next refresh",
                    path.display()
                );
                path.display().to_string()
            }
            Err(error) => {
                push_warning(
                    &mut warnings,
                    "snapshot_refresh_failed",
                    "snapshot",
                    format!("write memory snapshot: {error}"),
                );
                mark_source(
                    &mut sources,
                    "snapshot",
                    false,
                    Some("snapshot_refresh_failed"),
                );
                snapshot_failed = true;
                String::new()
            }
        }
    } else {
        push_warning(
            &mut warnings,
            "snapshot_refresh_deferred",
            "snapshot",
            "memory sources unavailable; snapshot refresh deferred".to_owned(),
        );
        mark_source(
            &mut sources,
            "snapshot",
            false,
            Some("snapshot_refresh_deferred"),
        );
        String::new()
    };
    let mut lines = vec![
        format!(
            "Profile: call_name={} assistant_alias={}",
            profile.identity.call_name, profile.identity.assistant_alias
        ),
        format!("Preferences: {}", preferences.len()),
        format!("Work context: {}", work_context.len()),
        format!("Current focus: {}", current_focus.len()),
        format!("Recent activity: {}", recent_activity.len()),
        format!("Recent work: {}", recent_work.len()),
        format!("Pending: {}", pending.len()),
        format!("Never: {}", never.len()),
    ];
    // Always "none": the runtime prompt belongs to the desktop app's active
    // session, which a one-shot CLI process cannot render (see the runtime
    // source note above).
    lines.push("Runtime: none".to_owned());
    lines.push(format!(
        "Snapshot: {}",
        if !snapshot_path.is_empty() {
            &snapshot_path
        } else if snapshot_failed {
            "(failed)"
        } else {
            "(deferred)"
        }
    ));
    // Only when the document was really rewritten (a deferred or failed
    // refresh changed nothing). Deliberately not a `warnings` entry: nothing
    // failed and nothing was deferred, and that array feeds the
    // source-availability diagnostics a consumer branches on.
    let snapshot_rewritten = !snapshot_path.is_empty();
    if snapshot_rewritten {
        lines.push(OVERVIEW_SNAPSHOT_REWRITE_NOTE.to_owned());
    }
    append_warning_lines(&mut lines, &warnings);
    let value = serde_json::json!({
        "profile": serde_json::to_value(&profile).unwrap_or_default(),
        "preferences": serde_json::to_value(&preferences).unwrap_or_default(),
        "work_context": serde_json::to_value(&work_context).unwrap_or_default(),
        "current_focus": serde_json::to_value(&current_focus).unwrap_or_default(),
        "recent_activity": serde_json::to_value(&recent_activity).unwrap_or_default(),
        "recent_work": serde_json::to_value(&recent_work).unwrap_or_default(),
        "pending": serde_json::to_value(&pending).unwrap_or_default(),
        "never": serde_json::to_value(&never).unwrap_or_default(),
        // Kept as an always-null key so the overview JSON shape stays stable;
        // a one-shot CLI process never renders a runtime prompt (see the
        // runtime source note above).
        "runtime": serde_json::Value::Null,
        "snapshot_path": snapshot_path,
        // True whenever this run really rewrote the document: the same
        // disclosure as the human note above, in the shape a script can test.
        "snapshot_rewritten_without_runtime": snapshot_rewritten,
        "warnings": warnings,
        "sources": sources,
    });
    Ok(success(render(output, lines.join("\n"), &value)))
}

fn profile_get(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let profile = feature::load_profile().map_err(|error| feature_error("profile_load", error))?;
    let human = format!(
        "call_name: {}\nassistant_alias: {}\nlanguage: {}\nupdated_at: {}",
        profile.identity.call_name,
        profile.identity.assistant_alias,
        profile.conventions.language,
        profile.updated_at,
    );
    let value = serde_json::to_value(&profile).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

fn profile_set(
    call_name: Option<String>,
    assistant_alias: Option<String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let patch = feature::ProfilePatch {
        call_name,
        assistant_alias,
        language: None,
        doc_standard: None,
        number_usage: None,
        style_notes: None,
    };
    let profile =
        feature::update_profile(patch).map_err(|error| feature_error("profile_update", error))?;
    let human = format!(
        "call_name: {}\nassistant_alias: {}",
        profile.identity.call_name, profile.identity.assistant_alias
    );
    let value = serde_json::to_value(&profile).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

fn list(store: Option<MemoryStore>, output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    match store {
        Some(store) => {
            let (item_lines, value) = load_store_items(store)?;
            let mut lines = vec![format!("{} ({})", store.as_str(), item_lines.len())];
            lines.extend(item_lines);
            Ok(success(render(output, lines.join("\n"), &value)))
        }
        None => {
            // The cleanup warnings the GUI surfaces as
            // `memory_topic_cleanup_required` are part of the read result;
            // dropping them would hide a store the GUI keeps complaining
            // about.
            let preferences = feature::list_preferences_with_cleanup()
                .map_err(|error| feature_error("list", error))?;
            let work_context = feature::load_work_context_with_cleanup()
                .map_err(|error| feature_error("list", error))?;
            let mut cleanup_warnings: Vec<(String, String)> = Vec::new();
            for (topic, warning) in [
                ("preferences", preferences.cleanup_warning.as_ref()),
                ("work_context", work_context.cleanup_warning.as_ref()),
            ] {
                if let Some(detail) = warning {
                    cleanup_warnings.push((topic.to_owned(), detail.clone()));
                }
            }
            let preferences = &preferences.value;
            let work_context = &work_context.value;
            let current_focus =
                feature::load_current_focus().map_err(|error| feature_error("list", error))?;
            let recent_activity =
                feature::load_recent_activity().map_err(|error| feature_error("list", error))?;
            let recent_work =
                feature::load_recent_work().map_err(|error| feature_error("list", error))?;
            let pending =
                feature::load_pending_memory().map_err(|error| feature_error("list", error))?;
            let mut lines = Vec::new();
            let mut section = |label: &str, items: &[String]| {
                lines.push(format!("{label} ({})", items.len()));
                lines.extend(items.iter().cloned());
            };
            section(
                "preferences",
                &preferences
                    .iter()
                    .map(render_preference)
                    .collect::<Vec<_>>(),
            );
            section(
                "work_context",
                &work_context
                    .iter()
                    .map(render_work_context)
                    .collect::<Vec<_>>(),
            );
            section(
                "current_focus",
                &current_focus.iter().map(render_timed).collect::<Vec<_>>(),
            );
            section(
                "recent_activity",
                &recent_activity.iter().map(render_timed).collect::<Vec<_>>(),
            );
            section(
                "recent_work",
                &recent_work
                    .iter()
                    .map(render_recent_work)
                    .collect::<Vec<_>>(),
            );
            section(
                "pending",
                &pending.iter().map(render_pending).collect::<Vec<_>>(),
            );
            for (topic, detail) in &cleanup_warnings {
                lines.push(format!(
                    "warning: memory_topic_cleanup_required ({topic}): {detail}"
                ));
            }
            let value = serde_json::json!({
                "cleanup_warnings": cleanup_warnings
                    .iter()
                    .map(|(topic, detail)| serde_json::json!({
                        "topic": topic,
                        "code": "memory_topic_cleanup_required",
                        "detail": detail,
                    }))
                    .collect::<Vec<_>>(),
                "preferences": serde_json::to_value(&preferences).unwrap_or_default(),
                "work_context": serde_json::to_value(&work_context).unwrap_or_default(),
                "current_focus": serde_json::to_value(&current_focus).unwrap_or_default(),
                "recent_activity": serde_json::to_value(&recent_activity).unwrap_or_default(),
                "recent_work": serde_json::to_value(&recent_work).unwrap_or_default(),
                "pending": serde_json::to_value(&pending).unwrap_or_default(),
            });
            Ok(success(render(output, lines.join("\n"), &value)))
        }
    }
}

/// Loads one store for `memory list --store`, returning the human item lines
/// and the JSON DTO array mirroring the GUI shapes.
///
/// Every store returns the same `{items, cleanup_warnings}` envelope so a
/// `--output json` consumer never has to branch on the shape per store;
/// stores without a cleanup sweep report an empty `cleanup_warnings` array.
fn load_store_items(store: MemoryStore) -> Result<(Vec<String>, serde_json::Value), CliError> {
    let io_error = |error| feature_error("list", error);
    let (items, value, cleanup_warnings) = match store {
        MemoryStore::Preferences => {
            let read = feature::list_preferences_with_cleanup().map_err(io_error)?;
            let lines: Vec<String> = read.value.iter().map(render_preference).collect();
            let warnings = read
                .cleanup_warning
                .map(|detail| {
                    vec![serde_json::json!({
                        "topic": "preferences",
                        "code": "memory_topic_cleanup_required",
                        "detail": detail,
                    })]
                })
                .unwrap_or_default();
            (
                lines,
                serde_json::to_value(&read.value).unwrap_or_default(),
                warnings,
            )
        }
        MemoryStore::WorkContext => {
            let read = feature::load_work_context_with_cleanup().map_err(io_error)?;
            let lines: Vec<String> = read.value.iter().map(render_work_context).collect();
            let warnings = read
                .cleanup_warning
                .map(|detail| {
                    vec![serde_json::json!({
                        "topic": "work_context",
                        "code": "memory_topic_cleanup_required",
                        "detail": detail,
                    })]
                })
                .unwrap_or_default();
            (
                lines,
                serde_json::to_value(&read.value).unwrap_or_default(),
                warnings,
            )
        }
        MemoryStore::CurrentFocus => {
            let items = feature::load_current_focus().map_err(io_error)?;
            (
                items.iter().map(render_timed).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
                Vec::new(),
            )
        }
        MemoryStore::RecentActivity => {
            let items = feature::load_recent_activity().map_err(io_error)?;
            (
                items.iter().map(render_timed).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
                Vec::new(),
            )
        }
        MemoryStore::RecentWork => {
            let items = feature::load_recent_work().map_err(io_error)?;
            (
                items.iter().map(render_recent_work).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
                Vec::new(),
            )
        }
        MemoryStore::Pending => {
            let items = feature::load_pending_memory().map_err(io_error)?;
            (
                items.iter().map(render_pending).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
                Vec::new(),
            )
        }
    };
    Ok((
        items,
        serde_json::json!({ "items": value, "cleanup_warnings": cleanup_warnings }),
    ))
}

/// Character cap the `memory add` pipeline actually applies, for BOTH kinds.
///
/// `memory add` never writes a store directly: it enqueues a candidate and
/// immediately confirms it, and the FIRST normalization on that route is
/// `pending_item_from_suggestion`'s `clean_text(&suggestion.content, 120)`
/// (`features/memory/io.rs`), whose tail is a hard `.chars().take(120)`. The
/// per-store caps downstream — `PREFERENCE_TEXT_MAX_CHARS` (120) and
/// `WORK_CONTEXT_TEXT_MAX_CHARS` (160) — are applied to a string that is
/// already at most 120 characters, so the work-context store's nominal 160
/// can never bind on this path: input of 121..=160 characters loses its tail
/// in the pending queue before the work-context writer ever sees it.
///
/// Warning against the store cap (160) instead of the cap that is really
/// applied is what made the loss silent, so this constant is deliberately the
/// pending-queue cap rather than either store constant. It is a plain literal
/// because the feature layer does not export the pending-queue cap: the 120
/// in `pending_item_from_suggestion` is an unnamed literal, and
/// `PREFERENCE_TEXT_MAX_CHARS` — which happens to share the value — is
/// `pub(super)` and in any case describes a different, later stage.
/// `memory_add_work_context_over_the_cap_reports_the_truncation` pins the
/// value against the feature layer's observable behavior — it asserts the
/// stored length against the store itself — so this cannot drift unnoticed.
const ADD_PIPELINE_TEXT_MAX_CHARS: usize = 120;

/// Mirror of `features::memory::util::clean_text` (`pub(super)`, so the CLI
/// cannot call it): collapse every whitespace run to one space, trim, then
/// hard-truncate to `max_chars` characters.
///
/// Reproducing it is what lets `add` predict — from the ORIGINAL user input —
/// exactly what the store will hold, instead of trusting the value the
/// pending queue echoes back. The mirror is pinned by the add tests, which
/// compare this prediction against what the feature layer really wrote.
fn clean_text_like_feature(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

/// The stderr half of the truncation disclosure shared by `memory add` and
/// `memory update`, emitted at measurement time — before the store write — so
/// even a command that fails later has already named the loss. One formatter
/// for both lanes so the wording cannot drift.
fn note_truncation(lane: &str, submitted_chars: usize, cap_chars: usize, cap_clause: &str) {
    crate::note!(
        "memory {lane}: content is {submitted_chars} characters and exceeds the \
         {cap_chars}-character cap {cap_clause}; the tail was truncated"
    );
}

/// The output half of the same disclosure, appended after the command
/// succeeded: stderr notes vanish into `2>/dev/null` and are invisible to a
/// JSON consumer, so a truncating add or update also reports the loss on the
/// command's own output channel — the item IS stored, just shortened, but
/// neither a human nor a script can miss it. Fields: `truncated`,
/// `submitted_characters`, `stored_characters`, top-level like every other
/// item field.
fn disclose_truncation(
    value: &mut serde_json::Value,
    human: &mut String,
    submitted_chars: usize,
    stored_chars: usize,
    cap_chars: usize,
    cap_clause: &str,
) {
    if let Some(object) = value.as_object_mut() {
        object.insert("truncated".to_owned(), serde_json::json!(true));
        object.insert(
            "submitted_characters".to_owned(),
            serde_json::json!(submitted_chars),
        );
        object.insert(
            "stored_characters".to_owned(),
            serde_json::json!(stored_chars),
        );
    }
    human.push_str(&format!(
        "\nNote: the submitted {submitted_chars} characters exceed the \
         {cap_chars}-character cap {cap_clause}; only the first {stored_chars} \
         characters were stored"
    ));
}

/// The exact text `memory add` will store for `content`, derived from the
/// ORIGINAL user input by replaying the feature pipeline stage by stage.
///
/// Stage 1 is the pending queue's `clean_text(content, 120)`; stage 2 is the
/// target store's own normalization — verbatim for preferences
/// (`write_preference_unlocked` stores `item.content` as-is) and
/// `clean_candidate_sentence` for work context (`upsert_work_context_unlocked`
/// strips 请记住-style prefixes and outer punctuation).
///
/// Anchoring the post-write verification on this instead of on the enqueue
/// result is the point: `enqueue_memory_candidate` does not always return the
/// caller's own text. Its dedupe branch matches an existing *pending* row on
/// a LOWERCASED content key and returns that row unchanged, so a re-add that
/// differs only in case gets the earlier row's casing written to the store.
/// Verifying against that returned value compares the store with itself and
/// passes no matter what was lost; verifying against the user's input
/// detects it.
fn expected_stored_text(kind: AddKind, content: &str) -> String {
    let enqueued = clean_text_like_feature(content, ADD_PIPELINE_TEXT_MAX_CHARS);
    match kind {
        AddKind::Preference => enqueued,
        AddKind::WorkContext => {
            feature::clean_candidate_sentence(&enqueued, feature::WORK_CONTEXT_TEXT_MAX_CHARS)
        }
    }
}

/// The kind of pending row this add would identify as its own.
///
/// The pending stage's `pending_item_from_suggestion` runs
/// `normalize_pending_kind(clean_text(&suggestion.kind, 20))` on the SUGGESTION
/// kind; both CLI kinds ("preference", "work_context") are recognized there, so
/// the row kind is the kind string itself, unchanged. The AddKind enum already
/// carries it through `feature_kind`, which is the same string — no mirror, no
/// drift; this accessor exists so the divergence check reads symmetrically
/// next to `expected_pending_topic` and `expected_stored_text`.
fn expected_pending_kind(kind: AddKind) -> &'static str {
    kind.feature_kind()
}

/// The pending-row topic this add would identify as its own, per kind.
///
/// `pending_item_from_suggestion` normalizes topics ONLY for preference-kind
/// suggestions (`features/memory/io.rs`: `if kind == "preference" { topic =
/// normalize_preference_topic(&topic) }`), and the CLI adds without a topic,
/// so the two kinds land on different row topics. Both normalizers are
/// `pub(super)` to the app crate, so the values are documented literals
/// rather than calls; each is pinned by `memory add`'s own contract — a
/// drifting literal fails every ordinary add in the divergence check, which
/// is what `memory_add_still_confirms_when_every_field_matches_this_adds_own_
/// candidate` turns into a red test.
fn expected_pending_topic(kind: AddKind) -> &'static str {
    match kind {
        // `normalize_preference_topic` (features/memory/types.rs) maps the
        // empty suggestion topic onto the default preference bucket.
        AddKind::Preference => "answer_style",
        // No topic normalization for work context at the pending stage: the
        // row an add of this kind calls its own carries the topic verbatim,
        // which for a topic-less CLI add is the empty string. (The store-side
        // `upsert_work_context_memory` maps the empty topic onto
        // "task_pattern" only later, at the confirm.)
        AddKind::WorkContext => "",
    }
}

/// Error for an enqueue that handed back a pending row which is not this
/// add's own candidate, or `None` when the row matches this invocation on
/// every identifying field.
///
/// Round-18 fix: the check used to compare only the text body, so a pending
/// GUI candidate whose text matched but whose topic (or kind) differed was
/// treated as "the same candidate" and confirmed — approving someone else's
/// data and writing through their entry. The comparison now covers every
/// identifying field of a candidate, each in the form that field carries in
/// the pending row.
///
/// Text (as before): `enqueue_memory_candidate` does not always queue what it
/// was handed. Its dedupe branch matches an existing *pending* row on a
/// LOWERCASED content key and returns that row with its own `content` intact
/// (only topic/source/updated_at are touched); confirming the id it returns
/// would approve a candidate the user has not reviewed yet AND write that
/// row's wording into the topic bucket. Topic and kind (round-18): the same
/// dedupe branch matches on the kind plus a case-insensitive content key and
/// IGNORES the topic, so a foreign-topic candidate whose text matches
/// case-insensitively was handed back with its own topic/kind untouched —
/// fields the prior check never looked at.
///
/// On any mismatch the add fails with the pending queue and every store
/// untouched — before `confirm_pending_memory` runs — which is what the
/// remediation below promises.
fn diverged_candidate(
    kind: AddKind,
    pending_id: &str,
    pending: &feature::PendingMemoryItem,
    expected: &str,
) -> Option<CliError> {
    let diverge = |field: &str, own: &str, foreign: &str| {
        Some(CliError::failed(format!(
            "memory_add_not_materialized: the candidate pipeline reused the existing \
             pending entry {pending_id}, whose {field} {foreign:?} differs from this add's \
             own {own:?}; nothing was confirmed and no store was written — resolve that \
             entry first (`pinvou memory pending confirm|ignore|never {pending_id}`), \
             then retry this add"
        )))
    };
    if pending.topic != expected_pending_topic(kind) {
        return diverge("topic", expected_pending_topic(kind), &pending.topic);
    }
    if pending.kind != expected_pending_kind(kind) {
        return diverge("kind", expected_pending_kind(kind), &pending.kind);
    }
    let actually_stored = expected_stored_text(kind, &pending.content);
    if actually_stored == *expected {
        return None;
    }
    Some(CliError::failed(format!(
        "memory_add_not_materialized: the candidate pipeline reused the existing pending entry \
         {pending_id}, whose content {actually_stored:?} would be stored instead of the \
         submitted {expected:?} (the two match case-insensitively); nothing was confirmed and \
         no store was written — resolve that entry first (`pinvou memory pending \
         confirm|ignore|never {pending_id}`), then retry this add"
    )))
}

/// Adds a memory item through the same pipeline the GUI uses: enqueue the
/// candidate into `_pending.jsonl` and immediately confirm it so the item is
/// materialized into its authoritative store.
fn add(kind: AddKind, source: AddSource, output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let content = match source {
        AddSource::Inline(content) => content,
        AddSource::File(path) => {
            support::read_text_file_capped(&path, 64 * 1024, "memory_content_file_unreadable")?
        }
    };
    if content.trim().is_empty() {
        return Err(CliError::usage("memory add requires non-empty content"));
    }
    // Fail before any state change: preference-shaped profile text (the
    // feature heuristic `looks_like_profile_preference_text`, Chinese-only
    // needles) is intentionally NOT materialized by the confirm path —
    // `write_preference_unlocked` silently skips it because it belongs to the
    // memory profile, not the preference store — yet confirm still marks the
    // candidate confirmed. Enqueueing anyway would strand a
    // confirmed-but-never-stored pending entry and only fail afterwards, so
    // reject up front with the same message the post-write verification
    // produces, leaving the pending store untouched. The heuristic cleans the
    // text the same way the pending write does, so this probe is exact.
    if kind == AddKind::Preference && feature::looks_like_profile_preference_text(&content) {
        return Err(CliError::failed(
            "memory_add_not_materialized: preference content belongs to the \
memory profile instead",
        ));
    }
    // The text the feature pipeline will really store, predicted from the
    // original input. Computed once and reused for the pre-flight probe, the
    // truncation report and the post-write verification, so those three can
    // never disagree about what "stored" means.
    let expected = expected_stored_text(kind, &content);
    // Same fail-before-state-change discipline for work-context: content the
    // confirm path's sentence cleanup empties ("记住。") would pass the
    // enqueue gates, then fail inside the work-context write with the
    // pending entry already stranded. Reject up front, mirroring the
    // preference probe above. The probe replays the full pipeline (pending
    // truncation first, then the sentence cleanup) rather than cleaning the
    // raw input, so it matches the writer exactly.
    if kind == AddKind::WorkContext && expected.trim().is_empty() {
        return Err(CliError::failed(
            "memory_add_not_materialized: work-context content is empty after \
normalization (task-like or punctuation-only text is not stored)",
        ));
    }
    // Truncation is decided by whether the whitespace-collapsed input still
    // fits the cap the pipeline applies — NOT by `content.chars().count()` on
    // the raw string, which over-reports for input padded with newlines or
    // runs of spaces that normalization removes before the cap is reached.
    //
    // The cap is `ADD_PIPELINE_TEXT_MAX_CHARS` for both kinds: warning
    // work-context input against the work-context store's 160 was the bug —
    // 121..=160 characters are truncated by the pending queue at 120 and the
    // warning never fired, so the tail vanished silently and the post-write
    // check (which compared against the already-truncated echo) still passed.
    let collapsed = clean_text_like_feature(&content, usize::MAX);
    let truncated = collapsed.chars().count() > ADD_PIPELINE_TEXT_MAX_CHARS;
    if truncated {
        note_truncation(
            "add",
            collapsed.chars().count(),
            ADD_PIPELINE_TEXT_MAX_CHARS,
            "applied when the candidate is queued",
        );
    }
    // The preference and work-context stores are replace-per-topic: a new
    // item lands in a fixed topic bucket (the CLI adds without a topic, so
    // every add targets the same bucket) and the write deletes that bucket's
    // previous item. Bucket ids are topic-derived, so the replacement keeps
    // the old id for the new text — detection must compare (id, text)
    // entries. The replace semantics are correct feature behavior, but they
    // must not present as append-only: what this add removed is reported in
    // the output below.
    let existing = store_entries(kind).map_err(|error| feature_error("add", error))?;
    let suggestion = MemorySuggestion {
        kind: kind.feature_kind().to_owned(),
        topic: String::new(),
        content,
        source: "cli".to_owned(),
    };
    let pending = feature::enqueue_memory_candidate(suggestion)
        .map_err(|error| feature_error("add", error))?;
    // The enqueue can hand back a row this invocation did not create: its
    // dedupe branch matches an EXISTING pending row on a kind plus
    // lowercased content key and returns that row with its own fields
    // intact. Confirming that id has two real side effects behind what ends
    // up being a FAILED command — it approves a candidate the user never
    // reviewed and writes the other row's wording into ITS topic bucket
    // (round-18: a foreign-topic candidate with the same text was adopted
    // and confirmed through, deleting that bucket's previous item) — so the
    // divergence is caught here, before `confirm_pending_memory` runs,
    // leaving the pending queue and both stores exactly as they were.
    //
    // The comparison replays the pipeline over the returned row's fields, so
    // a row that IS derivable from this caller's input still passes: an add
    // whose text was merely truncated at ADD_PIPELINE_TEXT_MAX_CHARS predicts
    // the same `expected` and proceeds to report its truncation below.
    if let Some(error) = diverged_candidate(kind, &pending.id, &pending, &expected) {
        return Err(error);
    }
    feature::confirm_pending_memory(&pending.id)
        .map_err(|error| feature_error("add", error))?
        .ok_or_else(|| {
            CliError::failed("memory_add_failed: pending candidate disappeared before confirm")
        })?;
    // Every lookup below matches on `expected` — derived from the ORIGINAL
    // user input, never from `pending.content`. `pending.content` is the
    // pipeline's own echo of what it decided to keep, so comparing the store
    // against it asks "did the store keep what the store kept?", which is
    // true even when the submitted text was truncated or replaced by a
    // lowercased dedupe match. Matching on the prediction turns both of
    // those into a visible failure instead of a green no-op.
    let (mut human, mut value, replaced) = match kind {
        AddKind::Preference => {
            let items = feature::list_preferences().map_err(|error| feature_error("add", error))?;
            let item = items
                .iter()
                .rev()
                .find(|item| item.text == expected)
                // A pipeline that queued someone else's text is already
                // refused above, so the only remaining way to miss here is a
                // confirm that wrote nothing: the profile-shaped preference
                // skip (whose Chinese-only heuristic the pre-flight probe
                // shares) or a concurrent writer clearing the bucket.
                .ok_or_else(|| {
                    CliError::failed(
                        "memory_add_not_materialized: preference content belongs to the \
memory profile instead",
                    )
                })?;
            let after = items
                .iter()
                .map(|item| (item.id.clone(), item.text.clone()))
                .collect::<Vec<_>>();
            (
                format!("Remembered preference: {}", item.id),
                serde_json::to_value(item).unwrap_or_default(),
                replaced_entries(&existing, &after),
            )
        }
        AddKind::WorkContext => {
            // `expected` already carries the confirm path's
            // `clean_candidate_sentence` stage (leading 请记住-style prefixes
            // and outer punctuation stripped), so ordinary punctuated input
            // matches instead of false-failing after storing fine.
            let items =
                feature::load_work_context().map_err(|error| feature_error("add", error))?;
            let item = items
                .iter()
                .rev()
                .find(|item| item.text == expected)
                // Divergent pipeline text is refused before the confirm, so
                // reaching here means the confirm ran and the work-context
                // bucket still does not hold the predicted text.
                .ok_or_else(|| {
                    CliError::failed("memory_add_not_materialized: work context was not stored")
                })?;
            let after = items
                .iter()
                .map(|item| (item.id.clone(), item.text.clone()))
                .collect::<Vec<_>>();
            (
                format!("Remembered work context: {}", item.id),
                serde_json::to_value(item).unwrap_or_default(),
                replaced_entries(&existing, &after),
            )
        }
    };
    // stderr notes vanish into `2>/dev/null` and are invisible to a JSON
    // consumer, so a truncating add also reports the loss on the command's
    // own output channel: the add still succeeds (the item IS stored, just
    // shortened), but neither a human nor a script can miss it.
    if truncated {
        disclose_truncation(
            &mut value,
            &mut human,
            collapsed.chars().count(),
            expected.chars().count(),
            ADD_PIPELINE_TEXT_MAX_CHARS,
            "applied when the candidate is queued",
        );
    }
    if !replaced.is_empty() {
        if let Some(object) = value.as_object_mut() {
            object.insert("replaced".to_owned(), serde_json::json!(replaced));
        }
        human.push_str(&format!(
            "\nNote: this replaced {} earlier item(s) in the same topic bucket: {}",
            replaced.len(),
            replaced.join(", ")
        ));
    }
    Ok(success(render(output, human, &value)))
}

/// (id, text) snapshot of the store an add targets, for replacement
/// detection (ids are topic-derived, so (id, text) identifies a concrete
/// stored entry).
fn store_entries(kind: AddKind) -> Result<Vec<(String, String)>, std::io::Error> {
    let entries = match kind {
        AddKind::Preference => feature::list_preferences()?
            .into_iter()
            .map(|item| (item.id, item.text))
            .collect::<Vec<_>>(),
        AddKind::WorkContext => feature::load_work_context()?
            .into_iter()
            .map(|item| (item.id, item.text))
            .collect::<Vec<_>>(),
    };
    Ok(entries)
}

/// Entries that were present before an add but are gone after it — the
/// replace-per-topic write's collateral, reported by id.
fn replaced_entries(before: &[(String, String)], after: &[(String, String)]) -> Vec<String> {
    before
        .iter()
        .filter(|entry| !after.contains(entry))
        .map(|(id, _)| id.clone())
        .collect()
}

/// The text cap the `memory update` writer applies to the patch text, per
/// store — the `clean_candidate_sentence` cap at each write site
/// (`features/memory/io.rs`: `update_preference_unlocked`,
/// `update_work_context_unlocked`, `update_timed_memory_unlocked`). The
/// preference and timed constants are `pub(super)` to the app crate, so they
/// are documented literals here; the update truncation contract test reads
/// the stored length back from the store itself, so a drifting literal fails
/// loudly. `None` for the two stores `parse_update` refuses: they have no
/// writer, so there is nothing to predict.
fn update_writer_cap(store: MemoryStore) -> Option<usize> {
    match store {
        // PREFERENCE_TEXT_MAX_CHARS.
        MemoryStore::Preferences => Some(120),
        MemoryStore::WorkContext => Some(feature::WORK_CONTEXT_TEXT_MAX_CHARS),
        // TIMED_TEXT_MAX_CHARS, shared by both timed stores.
        MemoryStore::CurrentFocus | MemoryStore::RecentActivity => Some(180),
        MemoryStore::RecentWork | MemoryStore::Pending => None,
    }
}

/// The `memory update` truncation measurement, captured before the write: the
/// target store's writer cap plus the submitted-versus-stored character
/// counts, everything the two disclosure channels need.
struct UpdateTruncation {
    cap_chars: usize,
    cap_clause: String,
    submitted_chars: usize,
    stored_chars: usize,
}

impl UpdateTruncation {
    /// Predicts, from the ORIGINAL `--content`, what the store's writer will
    /// keep. The submitted side is measured whitespace-collapsed — the exact
    /// measurement the add lane warns against, so input padded with newlines
    /// or runs of spaces cannot manufacture a disclosure — and the stored
    /// side is the writer's own `clean_candidate_sentence(text, cap)`, the
    /// same normalization the empty check above replays (there against the
    /// exported work-context constant, which cannot change the emptiness
    /// outcome; here the per-store cap is the whole point).
    fn measure(store: MemoryStore, content: &str) -> Option<Self> {
        let cap_chars = update_writer_cap(store)?;
        Some(Self {
            submitted_chars: clean_text_like_feature(content, usize::MAX).chars().count(),
            cap_clause: format!("applied when the {} item is written", store.as_str()),
            cap_chars,
            stored_chars: feature::clean_candidate_sentence(content, cap_chars)
                .chars()
                .count(),
        })
    }

    fn is_truncated(&self) -> bool {
        self.stored_chars < self.submitted_chars
    }
}

fn update(
    store: MemoryStore,
    id: &str,
    content: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    // Same parse-time gate as `add`: an empty/whitespace body is a usage
    // error, not a host failure (the store rejection would exit 1 with a
    // store-flavored message for a knowable-at-parse-time invalid argument).
    if content.trim().is_empty() {
        return Err(CliError::usage("memory update requires non-empty content"));
    }
    // Same fail-before-the-write classification as `add`: every editable
    // store's writer normalizes the patch text with `clean_candidate_sentence`
    // (preferences, work context and both timed stores all do) and rejects the
    // result when it is empty, so text like "记住。" — a 请记住-style prefix
    // plus punctuation — reached the store and came back as a
    // `memory_update_failed: ...` io error, while the identical input is
    // classified up front by `add`. Predict it here instead so the two
    // commands answer the same input the same way.
    //
    // Which cap is passed cannot change the outcome: `clean_candidate_sentence`
    // truncates AFTER stripping, so a non-empty stripped text stays non-empty
    // for every cap, and only the exported work-context constant is reachable
    // from the CLI (`PREFERENCE_TEXT_MAX_CHARS` and the timed cap are
    // `pub(super)`).
    let normalized =
        feature::clean_candidate_sentence(content, feature::WORK_CONTEXT_TEXT_MAX_CHARS);
    if normalized.is_empty() {
        return Err(CliError::failed(format!(
            "memory_update_not_applied: {} content is empty after normalization \
(prefix-only or punctuation-only text is not stored)",
            store.as_str()
        )));
    }
    // Truncation honesty, the same disclosure `add` performs for the same
    // loss class: every editable store's writer truncates the patch text to
    // its own `clean_candidate_sentence` cap with a hard `chars().take`, so
    // an over-cap `--content` stores fewer characters than submitted and
    // still exits 0 — with nothing anywhere saying so. Predicted before the
    // write, mirroring `add`: the stderr note fires even when the write later
    // fails (an unknown id), and the JSON fields and human note ride the
    // successful output at the bottom of this function.
    let truncation = UpdateTruncation::measure(store, content);
    if let Some(truncation) = truncation.as_ref().filter(|t| t.is_truncated()) {
        note_truncation(
            "update",
            truncation.submitted_chars,
            truncation.cap_chars,
            &truncation.cap_clause,
        );
    }
    support::sandbox_home()?;
    let patch = MemoryTextPatch {
        topic: None,
        text: Some(content.to_owned()),
    };
    // The feature layer reports topic-directory cleanup warnings
    // (TopicMutation.cleanup_warning) alongside the write; the GUI surfaces
    // them, so the CLI must not swallow them.
    let (human, value, warning) = match store {
        MemoryStore::Preferences => {
            let event = feature::update_preference(id, patch)
                .map_err(|error| feature_error("update", error))?
                .ok_or_else(|| not_found(store, id))?;
            (
                format!("Updated preference: {}", event.value.id),
                serde_json::to_value(&event.value).unwrap_or_default(),
                event.cleanup_warning,
            )
        }
        MemoryStore::WorkContext => {
            let event = feature::update_work_context(id, patch)
                .map_err(|error| feature_error("update", error))?
                .ok_or_else(|| not_found(store, id))?;
            (
                format!("Updated work context: {}", event.value.id),
                serde_json::to_value(&event.value).unwrap_or_default(),
                event.cleanup_warning,
            )
        }
        MemoryStore::CurrentFocus | MemoryStore::RecentActivity => {
            let item = feature::update_timed_memory(store.as_str(), id, patch)
                .map_err(|error| feature_error("update", error))?
                .ok_or_else(|| not_found(store, id))?;
            (
                format!("Updated {}: {}", store.as_str(), item.id),
                serde_json::to_value(&item).unwrap_or_default(),
                None,
            )
        }
        // Kept as defence in depth, not as a reachable path: `parse_update`
        // already refuses both stores through `parse_editable_store`, so the
        // CLI argv route never lands here. The match still has to be
        // exhaustive, and the choice is between this usage error and an
        // `unreachable!` panic — an in-process caller that builds
        // `MemoryCommand::Update` directly (a test, a future embedding of
        // `execute`) deserves a refusal it can handle rather than a crash
        // that also loses the report.
        MemoryStore::RecentWork | MemoryStore::Pending => {
            return Err(CliError::usage(format!(
                "memory update does not support store '{}' (valid: preferences, \
work-context, current-focus, recent-activity)",
                store.as_str()
            )));
        }
    };
    let mut human = match &warning {
        Some(warning) => format!("{human}\nwarning: {warning}"),
        None => human,
    };
    // Item fields stay top-level with `warning` appended (mirroring `add`):
    // nesting the item under a key only when a warning exists would change
    // the JSON shape exactly when a consumer is least likely to re-check it.
    let mut value = match warning {
        Some(warning) => {
            let mut object = match value {
                serde_json::Value::Object(map) => map,
                other => serde_json::Map::from_iter([("item".to_owned(), other)]),
            };
            object.insert("warning".to_owned(), serde_json::Value::String(warning));
            serde_json::Value::Object(object)
        }
        None => value,
    };
    // The command succeeded, but "succeeded" can still mean "stored fewer
    // characters than submitted" — the same disclosure `add` appends for its
    // own truncations, on both output channels (the stderr note already fired
    // at measurement time above).
    if let Some(truncation) = truncation.as_ref().filter(|t| t.is_truncated()) {
        disclose_truncation(
            &mut value,
            &mut human,
            truncation.submitted_chars,
            truncation.stored_chars,
            truncation.cap_chars,
            &truncation.cap_clause,
        );
    }
    Ok(success(render(output, human, &value)))
}

fn delete(
    store: MemoryStore,
    id: &str,
    confirmed: bool,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    require_yes(confirmed)?;
    let changed = match store {
        MemoryStore::Preferences => {
            feature::delete_preference(id).map_err(|error| feature_error("delete", error))?
        }
        MemoryStore::WorkContext => {
            feature::delete_work_context(id).map_err(|error| feature_error("delete", error))?
        }
        MemoryStore::CurrentFocus | MemoryStore::RecentActivity => {
            feature::delete_timed_memory(store.as_str(), id)
                .map_err(|error| feature_error("delete", error))?
        }
        // Defence in depth for the same reason as the `update` arm above:
        // `parse_delete` rejects both stores at parse time, but the match must
        // be exhaustive and a handled refusal beats a panic for any in-process
        // caller that constructs the command directly.
        MemoryStore::RecentWork | MemoryStore::Pending => {
            return Err(CliError::usage(format!(
                "memory delete does not support store '{}' (valid: preferences, \
work-context, current-focus, recent-activity)",
                store.as_str()
            )));
        }
    };
    if !changed {
        return Err(not_found(store, id));
    }
    let human = format!("Deleted {}: {}", store.as_str(), id);
    let value = serde_json::json!({ "store": store.as_str(), "id": id, "deleted": true });
    Ok(success(render(output, human, &value)))
}

fn archive(id: &str, output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let changed =
        feature::archive_recent_work(id).map_err(|error| feature_error("archive", error))?;
    if !changed {
        return Err(not_found(MemoryStore::RecentWork, id));
    }
    let human = format!("Archived recent work: {id}");
    let value = serde_json::json!({ "id": id, "archived": true });
    Ok(success(render(output, human, &value)))
}

fn pending(
    action: PendingAction,
    id: &str,
    reason: Option<String>,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let (human, value) = match action {
        PendingAction::Confirm => {
            let event = feature::confirm_pending_memory(id)
                .map_err(|error| feature_error("pending", error))?
                .ok_or_else(|| not_found(MemoryStore::Pending, id))?;
            // The confirm path can mark a candidate confirmed while the
            // target store is never written (profile-shaped preference text
            // is the best-known case). Reporting success would strand the
            // item confirmed-but-never-materialized, so read the row back and
            // ask the feature layer whether the write is visible.
            //
            // The read-back matches the RAW argv id, while
            // `confirm_pending_memory` resolves `clean_id(id)` and
            // `load_pending_memory` normalizes stored ids the same way: an id
            // whose cleaned form differs (quoted, punctuated, padded) confirms
            // a real row and then matches nothing here. Defaulting that to
            // "materialized" skipped the honesty check exactly when the input
            // was off, so a row we cannot read back is its own failure rather
            // than a silent success.
            let confirmed = feature::load_pending_memory()
                .map_err(|error| feature_error("pending", error))?
                .into_iter()
                .find(|item| item.id == id);
            let Some(confirmed) = confirmed else {
                return Err(CliError::failed(format!(
                    "memory pending confirm({id}): the confirm was accepted, but no pending \
                     entry with this id can be read back, so whether the target store was \
                     written cannot be verified; the id given probably differs from its stored \
                     spelling (ids normalize to letters, digits, '-' and '_'), or the entry was \
                     removed concurrently — check `pinvou memory list --store pending`"
                )));
            };
            // The observable fact is only that the confirm produced no store
            // row we can see; `confirmed_pending_memory_is_materialized`
            // returns false for several distinct causes and does not say
            // which, so the message lists them instead of asserting one. In
            // particular its catch-all arm reports false for a `profile`
            // candidate with any topic other than call_name/assistant_alias,
            // where a write may well have happened.
            if !feature::confirmed_pending_memory_is_materialized(&confirmed) {
                return Err(CliError::failed(format!(
                    "memory pending confirm({id}): the candidate is marked confirmed, but no \
                     matching item is visible in its target store. Known causes: the content is \
                     profile-shaped preference text the write deliberately skips (never \
                     materialized); a current-focus/recent-activity item whose store row is no \
                     longer active (TTL archival); an item deleted after an earlier confirm (a \
                     re-confirm short-circuits and rewrites nothing); a concurrent removal of a \
                     just-written item; or a profile candidate whose topic is neither call_name \
                     nor assistant_alias, which this check cannot verify at all. Inspect the \
                     target store with `pinvou memory list` before retrying"
                )));
            }
            (
                format!("Confirmed pending: {id}"),
                serde_json::json!({ "id": id, "result": "confirmed", "event": serde_json::to_value(&event).unwrap_or_default() }),
            )
        }
        PendingAction::Ignore => {
            let outcome = feature::ignore_pending_memory(id)
                .map_err(|error| feature_error("pending", error))?;
            match outcome {
                PendingIgnoreOutcome::Ignored(event) => (
                    format!("Ignored pending: {id}"),
                    serde_json::json!({ "id": id, "result": "ignored", "event": serde_json::to_value(&event).unwrap_or_default() }),
                ),
                PendingIgnoreOutcome::AlreadyDecided => (
                    format!("Pending already decided: {id}"),
                    serde_json::json!({ "id": id, "result": "already_decided", "event": null }),
                ),
                PendingIgnoreOutcome::NotFound => {
                    return Err(not_found(MemoryStore::Pending, id));
                }
            }
        }
        PendingAction::Never => {
            let event = feature::never_pending_memory(id, reason)
                .map_err(|error| feature_error("pending", error))?
                .ok_or_else(|| not_found(MemoryStore::Pending, id))?;
            (
                format!("Marked pending as never: {id}"),
                serde_json::json!({ "id": id, "result": "never", "event": serde_json::to_value(&event).unwrap_or_default() }),
            )
        }
    };
    Ok(success(render(output, human, &value)))
}

/// Cross-process single-flight lock for `memory organize` — see [`organize`]
/// for why the feature layer's in-memory guard is not enough for two CLI
/// processes. Same `$PINVOU3_HOME/locks` directory and same fd-lock primitive
/// as `voice asr-install`'s install lock. The caller must keep the returned
/// lock alive alongside its write guard.
fn organize_lock() -> Result<fd_lock::RwLock<std::fs::File>, CliError> {
    // `sandbox_home` already ran in `execute`, so the lock cannot land in a
    // cwd-relative directory.
    let dir = pinvou3_lib::platform::paths::pinvou3_home().join("locks");
    std::fs::create_dir_all(&dir).map_err(|error| {
        CliError::failed(format!(
            "memory organize: cannot create {}: {error}",
            dir.display()
        ))
    })?;
    let path = dir.join("memory-organize.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| {
            CliError::failed(format!(
                "memory organize: cannot open {}: {error}",
                path.display()
            ))
        })?;
    Ok(fd_lock::RwLock::new(file))
}

/// Runs one full memory organize pass through the windowless product host —
/// the same wiring as the scheduled memory-organize executor. Requires a
/// display (xvfb on headless Linux) and a configured, active model; organize
/// calls the LLM and applies delete/update/merge actions to every store, then
/// refreshes the snapshot.md device document like the GUI command does.
///
/// Cross-process single-flight, now closed where every surface meets: the
/// feature layer's `ORGANIZE_IN_FLIGHT` guard is process-local
/// (`features/memory/organize.rs`: "the two passes would interleave
/// destructive actions based on their own (up to 75-second-old) snapshots"),
/// so it serializes the GUI's own two triggers but says nothing about a
/// second CLI process. That half no longer depends on this command's lock:
/// `organize_memory_with_llm` itself now takes the feature layer's
/// cross-process `.organize.lock` (`io.rs` `try_lock_organize_pass`) around
/// the whole pass, and every surface — the GUI button, the scheduled
/// executor, and this CLI host lane — goes through it. So a GUI-triggered
/// organize now fails busy while a CLI pass runs and vice versa, and the
/// destructive-apply phases can no longer interleave across processes.
/// Residual, disclosed: the post-pass `snapshot.md` refresh runs after
/// `organize_memory_with_llm` returns — outside the `.organize.lock` — so
/// two passes' snapshot refreshes can still interleave; the store mutations
/// the lock exists for are inside it on every surface. This command's own
/// earlier `memory-organize.lock` remains as the CLI-vs-CLI gate covering
/// the host-boot window the feature lock does not see.
fn organize(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    if !feature::memory_enabled() {
        return Err(CliError::failed(
            "memory_organize_disabled: memory is disabled in settings",
        ));
    }
    let mut organize_lock = organize_lock()?;
    let _organize_guard = organize_lock.try_write().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            CliError::failed(
                "memory_organize_busy: another pinvou process is organizing memory; retry after \
                 it finishes",
            )
        } else {
            CliError::failed(format!(
                "memory organize: cannot acquire the organize lock: {error}"
            ))
        }
    })?;
    let report = pinvou3_lib::headless_bridge::run_windowless_host(|pool, _store| async move {
        // Same shared-bridge fallback as the GUI command and the scheduled
        // executor; fresh_bridge_for is crate-private to pinvou3_lib, so the
        // CLI always organizes with the shared bridge plus current global prefs.
        let mut bridge = pool.bridge.clone();
        bridge.prefs = pinvou3_lib::platform::prefs::UserPrefs::load();
        bridge.session_model = None;
        feature::organize_memory_with_llm(&bridge, None).await
        // No runtime prompt refresh here: the previous `store.active_id()`
        // branch was dead (the active session is process-local state a
        // one-shot CLI process never owns), and the live GUI's cached prompt
        // can only be refreshed by the GUI process itself. The snapshot
        // document refresh happens below, outside the host.
    })
    .map_err(|error| {
        CliError::failed(format!(
            "memory_organize_failed: {}",
            pinvou3_lib::platform::credential_store::redact_secret(&format!("{error:#}"))
        ))
    })?;
    // Same post-organize refresh as the GUI command (app/commands/memory.rs
    // `organize_memory` → `refresh_memory_snapshot_document`): reload every
    // authoritative source and rewrite the snapshot document so it reflects
    // the organize pass. A refresh failure is a warning, never a failed
    // organize — the GUI treats it the same way.
    refresh_snapshot_document_after_organize();
    let mut lines = vec![
        organize_summary(&report),
        format!("Started: {}", report.started_at),
        format!("Finished: {}", report.finished_at),
        format!("Model: {}", report.model),
    ];
    if !report.warnings.is_empty() {
        lines.push(format!("Warnings: {}", report.warnings.join("; ")));
    }
    let value = serde_json::to_value(&report).unwrap_or_default();
    Ok(success(render(output, lines.join("\n"), &value)))
}

/// Post-organize snapshot refresh: reload the eight authoritative memory
/// sources and rewrite the snapshot.md device document, mirroring the GUI
/// organize command's `refresh_memory_snapshot_document` (which reuses the
/// same overview loading and snapshot-write helpers). All diagnostics go to
/// stderr as warnings — the organize result is never failed by a refresh
/// problem, exactly like the GUI. The same all-sources-available gate as the
/// overview applies: a partial read must not wipe that category from the
/// document, so the refresh is deferred when any source is unavailable.
fn refresh_snapshot_document_after_organize() {
    let LoadedMemorySources {
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        warnings,
        sources,
    } = load_memory_sources();
    // Surface every load/cleanup diagnostic on stderr like the GUI's
    // load_memory_source does, so a deferred or partial refresh is explained.
    append_warning_lines_to_stderr(&warnings);
    if !sources.values().all(available) {
        note!(
            "[memory] snapshot_refresh_deferred snapshot: memory sources unavailable; \
snapshot refresh deferred after organize"
        );
        return;
    }
    if let Err(error) = feature::write_memory_snapshot_document(
        &profile,
        &preferences,
        &work_context,
        &current_focus,
        &recent_activity,
        &recent_work,
        &pending,
        &never,
        // No runtime section: the desktop app's active session is
        // process-local state, so there is nothing to render here (the GUI
        // passes None the same way when no session is open).
        None,
    ) {
        note!("[memory] snapshot_refresh_failed snapshot: write memory snapshot: {error}");
    }
}

/// stderr variant of `append_warning_lines` for paths without a JSON payload
/// (the post-organize refresh reports through stderr only).
fn append_warning_lines_to_stderr(warnings: &[serde_json::Value]) {
    for warning in warnings {
        note!(
            "[memory] {} {}: {}",
            warning["code"].as_str().unwrap_or(""),
            warning["source"].as_str().unwrap_or(""),
            warning["detail"].as_str().unwrap_or(""),
        );
    }
}

fn organize_history(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let history = feature::load_organize_history();
    let human = if history.is_empty() {
        "No organize history.".to_owned()
    } else {
        history
            .iter()
            .map(|report| {
                format!(
                    "{}\t{}\t{}",
                    report.started_at,
                    report.model,
                    organize_summary(report)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let value = serde_json::to_value(&history).unwrap_or_default();
    Ok(success(render(output, human, &value)))
}

/// Same summary wording as the scheduled executor's memory_organize_summary.
fn organize_summary(report: &MemoryOrganizeReport) -> String {
    let total = |counts: &BTreeMap<String, u32>| counts.values().copied().sum::<u32>();
    if report.no_change {
        return format!(
            "Memory organize completed: scanned {}, no changes needed",
            total(&report.scanned)
        );
    }
    format!(
        "Memory organize completed: scanned {}, deleted {}, updated {}, merged {}, skipped sensitive {}",
        total(&report.scanned),
        total(&report.deleted),
        total(&report.updated),
        total(&report.merged),
        report.skipped_sensitive,
    )
}

// ---- shared overview loading helpers (mirror app/commands/memory.rs) ----

fn available(status: &serde_json::Value) -> bool {
    status["available"] == true
}

fn mark_source(
    sources: &mut BTreeMap<String, serde_json::Value>,
    name: &str,
    ok: bool,
    code: Option<&str>,
) {
    sources.insert(
        name.to_owned(),
        serde_json::json!({ "available": ok, "code": code }),
    );
}

fn push_warning(warnings: &mut Vec<serde_json::Value>, code: &str, source: &str, detail: String) {
    warnings.push(serde_json::json!({
        "code": code,
        "source": source,
        "detail": detail,
    }));
}

fn loaded_source<T: Default>(
    source: &str,
    result: std::io::Result<T>,
    warnings: &mut Vec<serde_json::Value>,
    sources: &mut BTreeMap<String, serde_json::Value>,
) -> T {
    match result {
        Ok(value) => {
            mark_source(sources, source, true, None);
            value
        }
        Err(error) => {
            push_warning(
                warnings,
                "memory_source_unavailable",
                source,
                format!("load {source}: {error}"),
            );
            mark_source(sources, source, false, Some("memory_source_unavailable"));
            T::default()
        }
    }
}

fn loaded_topic_source<T: Default>(
    source: &str,
    result: std::io::Result<feature::TopicRead<T>>,
    warnings: &mut Vec<serde_json::Value>,
    sources: &mut BTreeMap<String, serde_json::Value>,
) -> T {
    match result {
        Ok(read) => {
            let code = read
                .cleanup_warning
                .as_ref()
                .map(|_| "memory_topic_cleanup_required");
            if let Some(detail) = read.cleanup_warning {
                push_warning(warnings, "memory_topic_cleanup_required", source, detail);
            }
            mark_source(sources, source, true, code);
            read.value
        }
        Err(error) => loaded_source(source, Err(error), warnings, sources),
    }
}

fn append_warning_lines(lines: &mut Vec<String>, warnings: &[serde_json::Value]) {
    for warning in warnings {
        lines.push(format!(
            "Warning: {} {}: {}",
            warning["code"].as_str().unwrap_or(""),
            warning["source"].as_str().unwrap_or(""),
            warning["detail"].as_str().unwrap_or(""),
        ));
    }
}

// ---- per-store human line renderers (one item per line, tab separated) ----

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_preference(item: &PreferenceFile) -> String {
    format!("{}\t{}\t{}", item.id, item.topic, one_line(&item.text))
}

fn render_work_context(item: &WorkContextFile) -> String {
    format!("{}\t{}\t{}", item.id, item.topic, one_line(&item.text))
}

fn render_timed(item: &TimedMemoryItem) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        item.id,
        item.topic,
        one_line(&item.text),
        item.status
    )
}

fn render_recent_work(item: &RecentWorkItem) -> String {
    format!("{}\t{}\t{}", item.id, item.status, one_line(&item.title))
}

fn render_pending(item: &feature::PendingMemoryItem) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        item.id,
        item.status,
        item.kind,
        one_line(&item.content)
    )
}
