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

use std::collections::BTreeMap;
use std::path::PathBuf;

use pinvou3_lib::features::memory as feature;
use pinvou3_lib::features::memory::{
    MemoryOrganizeReport, MemorySuggestion, MemoryTextPatch, PendingIgnoreOutcome, PreferenceFile,
    RecentWorkItem, TimedMemoryItem, WorkContextFile,
};
use pinvou3_lib::features::sessions::SessionStore;

use crate::support::{self, render, require_yes, success};
use crate::{CliError, CliOutcome, OutputMode};

const MEMORY_USAGE: &str = "usage: pinvou memory <overview|profile|list|add|update|delete|\
archive|pending|organize|organize-history>";

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
    Organize,
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
            expect_no_arguments(&values[2..], "memory organize")?;
            Ok(MemoryCommand::Organize)
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
        (None, Some(path)) => AddSource::File(PathBuf::from(path)),
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
                options.flags.push(token.to_owned());
                index += 1;
                continue;
            }
            if allowed_valued.contains(&token) {
                let value = values
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage(format!("{token} requires a value")))?
                    .clone();
                if value.starts_with("--") {
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
        MemoryCommand::Organize => organize(output),
        MemoryCommand::OrganizeHistory => organize_history(output),
    }
}

fn feature_error(action: &str, error: std::io::Error) -> CliError {
    CliError::failed(format!("memory_{action}_failed: {error}"))
}

fn not_found(store: MemoryStore, id: &str) -> CliError {
    CliError::failed(format!("{}_not_found: {id}", store.as_str()))
}

fn overview(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
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
    // Runtime prompt cache: like the GUI, resolve the active session through the
    // standalone session store; any failure is a warning, never a failed overview.
    let runtime = match SessionStore::boot() {
        Ok(store) => match store.active_id() {
            Some(session_id) => match feature::runtime_snapshot(&session_id) {
                Ok(snapshot) => {
                    mark_source(&mut sources, "runtime", true, None);
                    Some(snapshot)
                }
                Err(error) => {
                    push_warning(
                        &mut warnings,
                        "runtime_refresh_failed",
                        "runtime",
                        format!("render runtime memory: {error}"),
                    );
                    mark_source(
                        &mut sources,
                        "runtime",
                        false,
                        Some("runtime_refresh_failed"),
                    );
                    None
                }
            },
            None => {
                mark_source(&mut sources, "runtime", true, None);
                None
            }
        },
        Err(error) => {
            push_warning(
                &mut warnings,
                "runtime_refresh_failed",
                "runtime",
                format!("boot session store: {error}"),
            );
            mark_source(
                &mut sources,
                "runtime",
                false,
                Some("runtime_refresh_failed"),
            );
            None
        }
    };
    // Same gate as the GUI overview: refresh snapshot.md only when every
    // authoritative source is available, otherwise defer (a partial read must
    // not wipe that category from the snapshot document).
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
            runtime.as_ref(),
        ) {
            Ok(path) => {
                mark_source(&mut sources, "snapshot", true, None);
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
    match &runtime {
        Some(snapshot) => lines.push(format!(
            "Runtime: {} ({} items)",
            snapshot.session_id,
            snapshot.items.len()
        )),
        None => lines.push("Runtime: none".to_owned()),
    }
    lines.push(format!(
        "Snapshot: {}",
        if snapshot_path.is_empty() {
            "(deferred)"
        } else {
            &snapshot_path
        }
    ));
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
        "runtime": runtime
            .as_ref()
            .map(|snapshot| serde_json::to_value(snapshot).unwrap_or_default()),
        "snapshot_path": snapshot_path,
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
fn load_store_items(store: MemoryStore) -> Result<(Vec<String>, serde_json::Value), CliError> {
    let io_error = |error| feature_error("list", error);
    Ok(match store {
        MemoryStore::Preferences => {
            let read = feature::list_preferences_with_cleanup().map_err(io_error)?;
            let warning = read.cleanup_warning.map(|detail| {
                serde_json::json!([{
                    "topic": "preferences",
                    "code": "memory_topic_cleanup_required",
                    "detail": detail,
                }])
            });
            (
                read.value.iter().map(render_preference).collect(),
                match warning {
                    Some(warnings) => serde_json::json!({
                        "items": serde_json::to_value(&read.value).unwrap_or_default(),
                        "cleanup_warnings": warnings,
                    }),
                    None => serde_json::to_value(&read.value).unwrap_or_default(),
                },
            )
        }
        MemoryStore::WorkContext => {
            let read = feature::load_work_context_with_cleanup().map_err(io_error)?;
            let warning = read.cleanup_warning.map(|detail| {
                serde_json::json!([{
                    "topic": "work_context",
                    "code": "memory_topic_cleanup_required",
                    "detail": detail,
                }])
            });
            (
                read.value.iter().map(render_work_context).collect(),
                match warning {
                    Some(warnings) => serde_json::json!({
                        "items": serde_json::to_value(&read.value).unwrap_or_default(),
                        "cleanup_warnings": warnings,
                    }),
                    None => serde_json::to_value(&read.value).unwrap_or_default(),
                },
            )
        }
        MemoryStore::CurrentFocus => {
            let items = feature::load_current_focus().map_err(io_error)?;
            (
                items.iter().map(render_timed).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
            )
        }
        MemoryStore::RecentActivity => {
            let items = feature::load_recent_activity().map_err(io_error)?;
            (
                items.iter().map(render_timed).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
            )
        }
        MemoryStore::RecentWork => {
            let items = feature::load_recent_work().map_err(io_error)?;
            (
                items.iter().map(render_recent_work).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
            )
        }
        MemoryStore::Pending => {
            let items = feature::load_pending_memory().map_err(io_error)?;
            (
                items.iter().map(render_pending).collect(),
                serde_json::to_value(&items).unwrap_or_default(),
            )
        }
    })
}

/// Adds a memory item through the same pipeline the GUI uses: enqueue the
/// candidate into `_pending.jsonl` and immediately confirm it so the item is
/// materialized into its authoritative store.
fn add(kind: AddKind, source: AddSource, output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let content = match source {
        AddSource::Inline(content) => content,
        AddSource::File(path) => std::fs::read_to_string(&path).map_err(|error| {
            CliError::failed(format!(
                "memory_content_file_unreadable: {}: {error}",
                path.display()
            ))
        })?,
    };
    if content.trim().is_empty() {
        return Err(CliError::usage("memory add requires non-empty content"));
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
    feature::confirm_pending_memory(&pending.id)
        .map_err(|error| feature_error("add", error))?
        .ok_or_else(|| {
            CliError::failed("memory_add_failed: pending candidate disappeared before confirm")
        })?;
    let (mut human, mut value, replaced) = match kind {
        AddKind::Preference => {
            let items = feature::list_preferences().map_err(|error| feature_error("add", error))?;
            let item = items
                .iter()
                .rev()
                .find(|item| item.text == pending.content)
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
            // The confirm path stores `clean_candidate_sentence(content)` —
            // leading 请记住-style prefixes and outer punctuation stripped —
            // so the verification must compare against the same normalized
            // form, or ordinary punctuated input false-fails after storing
            // fine.
            let stored = feature::clean_candidate_sentence(&pending.content, 160);
            let items =
                feature::load_work_context().map_err(|error| feature_error("add", error))?;
            let item = items
                .iter()
                .rev()
                .find(|item| item.text == stored)
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

fn update(
    store: MemoryStore,
    id: &str,
    content: &str,
    output: OutputMode,
) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    let patch = MemoryTextPatch {
        topic: None,
        text: Some(content.to_owned()),
        ttl_days: None,
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
        MemoryStore::RecentWork | MemoryStore::Pending => {
            return Err(CliError::usage(format!(
                "memory update does not support store '{}' (valid: preferences, \
work-context, current-focus, recent-activity)",
                store.as_str()
            )));
        }
    };
    let human = match &warning {
        Some(warning) => format!("{human}\nwarning: {warning}"),
        None => human,
    };
    let value = match warning {
        Some(warning) => serde_json::json!({ "warning": warning, "item": value }),
        None => value,
    };
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

/// Runs one full memory organize pass through the windowless product host —
/// the same wiring as the scheduled memory-organize executor. Requires a
/// display (xvfb on headless Linux) and a configured, active model; organize
/// calls the LLM and applies delete/update/merge actions to every store.
fn organize(output: OutputMode) -> Result<CliOutcome, CliError> {
    support::sandbox_home()?;
    if !feature::memory_enabled() {
        return Err(CliError::failed(
            "memory_organize_disabled: memory is disabled in settings",
        ));
    }
    let report = pinvou3_lib::headless_bridge::run_windowless_host(|pool, store| async move {
        // Same shared-bridge fallback as the GUI command and the scheduled
        // executor; fresh_bridge_for is crate-private to pinvou3_lib, so the
        // CLI always organizes with the shared bridge plus current global prefs.
        let mut bridge = pool.bridge.clone();
        bridge.prefs = pinvou3_lib::platform::prefs::UserPrefs::load();
        bridge.session_model = None;
        let result = feature::organize_memory_with_llm(&bridge, None).await;
        // Best-effort runtime refresh after organize, same as the executor:
        // organize may have removed items the cached runtime prompt still serves.
        if let Some(session_id) = store.active_id() {
            if let Err(error) = feature::runtime_snapshot(&session_id) {
                eprintln!("[memory] refresh runtime memory after organize: {error}");
            }
        }
        result
    })
    .map_err(|error| {
        CliError::failed(format!(
            "memory_organize_failed: {}",
            pinvou3_lib::platform::credential_store::redact_secret(&format!("{error:#}"))
        ))
    })?;
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
