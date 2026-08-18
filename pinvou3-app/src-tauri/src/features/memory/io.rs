//! 实体存储读写：4 个 JSONL store（recent_work / current_focus / recent_activity /
//! pending / never）+ 2 个目录 store（work_context / preferences）+ profile 单文件。
//!
//! 抽离自 `mod.rs`。每个 store 的 load/upsert/archive/delete/normalize/compact/write
//! 逻辑保持原样；`pub` 入口对外不变，`*_unlocked` helper 在本模块内复用，过期归档与
//! 确认物化等跨 store 检查也集中在此。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;

use crate::platform::{paths, prefs::UserPrefs};

use super::types::{
    looks_like_profile_preference_text, normalize_preference_topic, normalize_profile_label,
    normalize_timed_memory_kind, normalize_timed_memory_topic, normalize_work_context_topic,
    MemoryProfile, MemorySuggestion, MemoryTextPatch, MemoryWriteEvent, NeverMemoryItem,
    PendingMemoryItem, PreferenceFile, ProfilePatch, RecentWorkItem, RecentWorkPatch,
    RuntimeMemorySnapshot, TimedMemoryItem, TopicMigrationJournal, TopicMutation, TopicRead,
    TopicReconciliation, TurnCapture, TurnMemoryCapture, WorkContextFile,
    CURRENT_FOCUS_ACTIVE_MAX_STORED, CURRENT_FOCUS_DEFAULT_TTL_DAYS, NEVER_MEMORY_MAX_STORED,
    PENDING_MEMORY_ACTIVE_MAX_STORED, PENDING_MEMORY_RESOLVED_MAX_STORED, PENDING_STATUS_CONFIRMED,
    PENDING_STATUS_IGNORED, PENDING_STATUS_OBSERVED, PENDING_STATUS_PENDING, PROFILE_VERSION,
    RECENT_ACTIVITY_ACTIVE_MAX_STORED, RECENT_ACTIVITY_DEFAULT_TTL_DAYS,
    RECENT_WORK_ACTIVE_MAX_STORED, RECENT_WORK_ARCHIVED_MAX_STORED, RECENT_WORK_DEFAULT_TTL_DAYS,
    TIMED_MEMORY_ARCHIVED_MAX_STORED,
};
use super::util::{
    clean_candidate_sentence, clean_id, clean_scalar, clean_text, file_lifecycle_lock,
    invalid_data, json_lines_are_valid, looks_sensitive, looks_sensitive_or_task_like, parse_time,
    read_text_recovering, read_text_recovering_unlocked, recover_directory_json_files_unlocked,
    stable_id_from_text, stable_id_with_prefix, write_json_atomic, write_json_atomic_unlocked,
    write_text_atomic,
};

pub(super) fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub(super) fn turn_capture_store() -> &'static Mutex<BTreeMap<String, TurnCapture>> {
    static STORE: OnceLock<Mutex<BTreeMap<String, TurnCapture>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn profile_path() -> PathBuf {
    paths::user_memory_profile()
}

pub fn runtime_prompt_path(session_id: &str) -> PathBuf {
    paths::user_memory_runtime_prompt(session_id)
}

pub fn recent_work_path() -> PathBuf {
    paths::user_memory_recent_work()
}

pub fn work_context_dir() -> PathBuf {
    paths::user_memory_work_context_dir()
}

pub fn current_focus_path() -> PathBuf {
    paths::user_memory_current_focus()
}

pub fn recent_activity_path() -> PathBuf {
    paths::user_memory_recent_activity()
}

pub fn snapshot_path() -> PathBuf {
    paths::user_memory_snapshot()
}

pub fn pending_memory_path() -> PathBuf {
    paths::user_memory_pending()
}

pub fn never_memory_path() -> PathBuf {
    paths::user_memory_never()
}

pub fn record_turn_user(session_id: &str, user: &str) {
    let session_id = clean_id(session_id);
    if session_id.is_empty() {
        return;
    }
    let mut store = turn_capture_store().lock();
    store.insert(
        session_id,
        TurnCapture {
            user: clean_text(user, 4000),
            assistant: String::new(),
            ..TurnCapture::default()
        },
    );
}

pub fn append_turn_assistant(session_id: &str, delta: &str) {
    let session_id = clean_id(session_id);
    if session_id.is_empty() || delta.is_empty() {
        return;
    }
    let mut store = turn_capture_store().lock();
    let capture = store.entry(session_id).or_default();
    capture.assistant.push_str(delta);
    if capture.assistant.chars().count() > 4000 {
        capture.assistant = capture.assistant.chars().take(4000).collect();
    }
}

pub fn record_turn_tool_start(session_id: &str, name: &str, input: &Value) {
    let session_id = clean_id(session_id);
    if session_id.is_empty() {
        return;
    }
    let mut store = turn_capture_store().lock();
    let capture = store.entry(session_id).or_default();
    let summary = summarize_tool_start(name, input);
    if !summary.is_empty() && capture.tool_summaries.len() < 12 {
        capture.tool_summaries.push(summary);
    }
}

fn json_field_string(input: &Value, key: &str, max_chars: usize) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(|s| clean_text(s, max_chars))
        .unwrap_or_default()
}

pub(super) fn summarize_tool_start(name: &str, input: &Value) -> String {
    let name = clean_text(name, 80);
    if is_delivery_tool(&name, input) {
        let path = json_field_string(input, "path", 220);
        let file_path = json_field_string(input, "file_path", 220);
        let filename = json_field_string(input, "filename", 220);
        let title = json_field_string(input, "title", 120);
        let description = json_field_string(input, "description", 180);
        let target = if !path.is_empty() {
            path
        } else if !file_path.is_empty() {
            file_path
        } else {
            filename
        };
        let mut parts = vec![format!("tool_start name={name}")];
        if !target.is_empty() {
            parts.push(format!("path={target}"));
        }
        if !title.is_empty() {
            parts.push(format!("title={title}"));
        }
        if !description.is_empty() {
            parts.push(format!("description={description}"));
        }
        return clean_text(&parts.join(" "), 600);
    }
    clean_text(&format!("tool_start name={name} input={input}"), 600)
}

pub fn record_turn_tool_complete(session_id: &str, name: &str, input: &Value, success: bool) {
    let session_id = clean_id(session_id);
    if session_id.is_empty() {
        return;
    }
    let mut store = turn_capture_store().lock();
    let capture = store.entry(session_id).or_default();
    if success && is_delivery_tool(name, input) {
        capture.delivery_complete = true;
    }
    let summary = clean_text(
        &format!(
            "tool_complete name={} success={}",
            clean_text(name, 80),
            success
        ),
        200,
    );
    if !summary.is_empty() && capture.tool_summaries.len() < 12 {
        capture.tool_summaries.push(summary);
    }
}

pub(super) fn is_delivery_tool(name: &str, input: &Value) -> bool {
    let name = name.trim();
    name == "write_file"
        || name == "edit_file"
        || (name.eq_ignore_ascii_case("File")
            && input
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|action| matches!(action, "write" | "edit" | "patch")))
        || name == "present_artifact"
        || name.ends_with("present_artifact")
}

pub fn take_turn_capture(session_id: &str) -> Option<TurnMemoryCapture> {
    let session_id = clean_id(session_id);
    if session_id.is_empty() {
        return None;
    }
    let mut store = turn_capture_store().lock();
    let capture = store.remove(&session_id)?;
    if capture.user.trim().is_empty() {
        return None;
    }
    Some(TurnMemoryCapture {
        user: capture.user,
        assistant: capture.assistant,
        tool_summaries: capture.tool_summaries,
        delivery_complete: capture.delivery_complete,
    })
}

pub fn load_profile() -> io::Result<MemoryProfile> {
    let path = profile_path();
    match read_text_recovering(&path, |raw| {
        serde_json::from_str::<MemoryProfile>(raw).is_ok()
    }) {
        Ok(raw) => {
            let mut profile: MemoryProfile = serde_json::from_str(&raw).map_err(invalid_data)?;
            profile.normalize();
            Ok(profile)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(MemoryProfile {
            version: PROFILE_VERSION,
            ..MemoryProfile::default()
        }),
        Err(err) => Err(err),
    }
}

pub fn save_profile(profile: &MemoryProfile) -> io::Result<()> {
    let _guard = write_lock().lock();
    let mut normalized = profile.clone();
    normalized.normalize();
    let path = profile_path();
    write_json_atomic(&path, &normalized)
}

pub fn update_profile(patch: ProfilePatch) -> io::Result<MemoryProfile> {
    let _guard = write_lock().lock();
    let mut profile = load_profile()?;
    if let Some(value) = patch.call_name {
        profile.identity.call_name = value;
    }
    if let Some(value) = patch.assistant_alias {
        profile.identity.assistant_alias = value;
    }
    if let Some(value) = patch.language {
        profile.conventions.language = value;
    }
    if let Some(value) = patch.doc_standard {
        profile.conventions.doc_standard = value;
    }
    if let Some(value) = patch.number_usage {
        profile.conventions.number_usage = value;
    }
    if let Some(value) = patch.style_notes {
        profile.conventions.style_notes = value;
    }
    profile.revision = profile.revision.saturating_add(1);
    profile.updated_at = Utc::now().to_rfc3339();
    profile.normalize();
    let path = profile_path();
    write_json_atomic(&path, &profile)?;
    Ok(profile)
}

pub fn clear_profile() -> io::Result<MemoryProfile> {
    let profile = MemoryProfile {
        version: PROFILE_VERSION,
        updated_at: Utc::now().to_rfc3339(),
        revision: load_profile()
            .map(|p| p.revision.saturating_add(1))
            .unwrap_or(1),
        ..MemoryProfile::default()
    };
    save_profile(&profile)?;
    Ok(profile)
}

pub fn load_recent_work() -> io::Result<Vec<RecentWorkItem>> {
    let path = recent_work_path();
    let raw = match read_text_recovering(&path, json_lines_are_valid::<RecentWorkItem>) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut out = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(mut item) = serde_json::from_str::<RecentWorkItem>(line) else {
            continue;
        };
        normalize_recent_work(&mut item);
        if !item.id.is_empty() && !item.title.is_empty() {
            out.push(item);
        }
    }
    Ok(out)
}

pub fn upsert_recent_work(patch: RecentWorkPatch) -> io::Result<RecentWorkItem> {
    let _guard = write_lock().lock();
    upsert_recent_work_unlocked(patch)
}

pub(super) fn upsert_recent_work_unlocked(patch: RecentWorkPatch) -> io::Result<RecentWorkItem> {
    let now = Utc::now();
    let now_s = now.to_rfc3339();
    let ttl_days = patch
        .ttl_days
        .unwrap_or(RECENT_WORK_DEFAULT_TTL_DAYS)
        .clamp(1, 90);
    let mut items = load_recent_work()?;
    let title = clean_text(&patch.title, 50);
    if title.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "recent work title is empty",
        ));
    }
    let id = patch
        .id
        .map(|s| clean_id(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stable_id_from_text(&title));

    let mut item = if let Some(existing) = items.iter_mut().find(|item| item.id == id) {
        existing.title = title;
        existing.summary = patch
            .summary
            .as_deref()
            .map(|s| clean_text(s, 80))
            .unwrap_or_default();
        existing.source = patch
            .source
            .as_deref()
            .map(|s| clean_text(s, 40))
            .unwrap_or_default();
        existing.status = "active".to_string();
        existing.updated_at = now_s.clone();
        existing.last_hit = now_s.clone();
        existing.expires_at = (now + Duration::days(ttl_days)).to_rfc3339();
        existing.clone()
    } else {
        let item = RecentWorkItem {
            id,
            title,
            summary: patch
                .summary
                .as_deref()
                .map(|s| clean_text(s, 80))
                .unwrap_or_default(),
            status: "active".to_string(),
            source: patch
                .source
                .as_deref()
                .map(|s| clean_text(s, 40))
                .unwrap_or_default(),
            created_at: now_s.clone(),
            updated_at: now_s.clone(),
            last_hit: now_s.clone(),
            expires_at: (now + Duration::days(ttl_days)).to_rfc3339(),
        };
        items.push(item.clone());
        item
    };
    normalize_recent_work(&mut item);
    write_recent_work_unlocked(&items)?;
    Ok(item)
}

pub fn archive_recent_work(id: &str) -> io::Result<bool> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    let now = Utc::now().to_rfc3339();
    let mut items = load_recent_work()?;
    let mut changed = false;
    for item in &mut items {
        if item.id == id && item.status != "archived" {
            item.status = "archived".to_string();
            item.updated_at = now.clone();
            changed = true;
        }
    }
    if changed {
        write_recent_work_unlocked(&items)?;
    }
    if !changed {
        changed = archive_timed_memory_unlocked("current_focus", &id)?
            || archive_timed_memory_unlocked("recent_activity", &id)?;
    }
    Ok(changed)
}

fn resolve_topic_authorities<T: Clone>(
    records: Vec<(PathBuf, T)>,
    topic: impl Fn(&T) -> &str,
    id: impl Fn(&T) -> &str,
) -> (Vec<T>, Vec<String>) {
    let mut grouped = BTreeMap::<String, Vec<(PathBuf, T)>>::new();
    for (path, item) in records {
        grouped
            .entry(topic(&item).to_string())
            .or_default()
            .push((path, item));
    }
    let mut authorities = Vec::new();
    let mut cleanup_failures = Vec::new();
    for (_, mut group) in grouped {
        group.sort_by_key(|(path, item)| {
            let canonical = path.file_name().and_then(|value| value.to_str())
                == Some(format!("{}.json", id(item)).as_str());
            let modified = fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok();
            (canonical, modified, path.clone())
        });
        let (authority_path, authority) = group
            .pop()
            .expect("topic groups are constructed from at least one record");
        for (duplicate, _) in group {
            // A duplicate that cannot be removed right now (e.g. an open
            // handle on Windows) must not fail the whole load: keep it on
            // disk so the next read retries, and surface a cleanup warning
            // instead of a hard error — mirroring the soft handling in
            // reconcile_topic_migration_journals_unlocked.
            if let Err(error) = fs::remove_file(&duplicate) {
                cleanup_failures.push(format!("{}: {error}", duplicate.display()));
            }
        }
        debug_assert!(authority_path.is_file());
        authorities.push(authority);
    }
    (authorities, cleanup_failures)
}

pub fn load_work_context() -> io::Result<Vec<WorkContextFile>> {
    load_work_context_with_cleanup().map(|result| result.value)
}

pub fn load_work_context_with_cleanup() -> io::Result<TopicRead<Vec<WorkContextFile>>> {
    let _lifecycle = file_lifecycle_lock().lock();
    load_work_context_with_cleanup_unlocked()
}

fn load_work_context_unlocked() -> io::Result<Vec<WorkContextFile>> {
    load_work_context_with_cleanup_unlocked().map(|result| result.value)
}

fn load_work_context_with_cleanup_unlocked() -> io::Result<TopicRead<Vec<WorkContextFile>>> {
    let dir = work_context_dir();
    recover_directory_json_files_unlocked::<WorkContextFile>(&dir)?;
    let reconciliation = reconcile_topic_migration_journals_unlocked::<WorkContextFile>(&dir)?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(TopicRead {
                value: Vec::new(),
                cleanup_warning: reconciliation.cleanup_warning,
            });
        }
        Err(err) => return Err(err),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if reconciliation.hidden_files.contains(&path) {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = read_text_recovering_unlocked(&path, &|raw| {
            serde_json::from_str::<WorkContextFile>(raw).is_ok()
        })?;
        let mut item = serde_json::from_str::<WorkContextFile>(&raw).map_err(invalid_data)?;
        normalize_work_context(&mut item);
        if !item.topic.is_empty() && !item.text.is_empty() {
            records.push((path, item));
        }
    }
    let (value, authority_cleanup_failures) =
        resolve_topic_authorities(records, |item| &item.topic, |item| &item.id);
    let mut cleanup_parts = Vec::new();
    if let Some(base) = reconciliation.cleanup_warning {
        cleanup_parts.push(base);
    }
    cleanup_parts.extend(authority_cleanup_failures);
    Ok(TopicRead {
        value,
        cleanup_warning: (!cleanup_parts.is_empty()).then(|| cleanup_parts.join("; ")),
    })
}

pub(super) fn upsert_work_context_unlocked(
    suggestion: &MemorySuggestion,
    confidence: f32,
) -> io::Result<WorkContextFile> {
    let now = Utc::now().to_rfc3339();
    let topic = normalize_work_context_topic(&suggestion.topic);
    let id = stable_id_with_prefix("ctx", &topic);
    let mut item = WorkContextFile {
        id: id.clone(),
        kind: "work_context".to_string(),
        topic,
        text: clean_candidate_sentence(&suggestion.content, 160),
        source: clean_text(&suggestion.source, 40),
        confidence,
        created_at: now.clone(),
        updated_at: now,
    };
    normalize_work_context(&mut item);
    if item.text.is_empty() || looks_sensitive(&item.text) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "work context is empty or sensitive",
        ));
    }
    let dir = work_context_dir();
    fs::create_dir_all(&dir)?;
    write_json_atomic(&dir.join(format!("{id}.json")), &item)?;
    Ok(item)
}

pub(super) fn upsert_work_context_locked(
    suggestion: &MemorySuggestion,
    confidence: f32,
) -> io::Result<WorkContextFile> {
    let _guard = write_lock().lock();
    upsert_work_context_unlocked(suggestion, confidence)
}

fn commit_topic_migration_unlocked<T>(
    new_path: &Path,
    value: &T,
    stale_paths: &[PathBuf],
    validate: impl Fn(&T) -> bool,
) -> io::Result<TopicMutation<T>>
where
    T: Clone + Serialize + serde::de::DeserializeOwned,
{
    let journal_path = topic_migration_journal_path(new_path)?;
    commit_topic_migration_unlocked_with(
        &journal_path,
        new_path,
        value,
        stale_paths,
        validate,
        write_json_atomic_unlocked,
        |path| fs::remove_file(path),
    )
}

pub(super) fn commit_topic_migration_unlocked_with<T, W, R>(
    journal_path: &Path,
    new_path: &Path,
    value: &T,
    stale_paths: &[PathBuf],
    validate: impl Fn(&T) -> bool,
    write: W,
    mut remove: R,
) -> io::Result<TopicMutation<T>>
where
    T: Clone + Serialize + serde::de::DeserializeOwned,
    W: FnOnce(&Path, &T) -> io::Result<()>,
    R: FnMut(&Path) -> io::Result<()>,
{
    let authority_file = new_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid authority filename"))?
        .to_string();
    let stale_files = stale_paths
        .iter()
        .filter_map(|path| path.file_name().and_then(|value| value.to_str()))
        .map(str::to_string)
        .collect::<Vec<_>>();
    let authority_json = serde_json::to_string_pretty(value).map_err(invalid_data)? + "\n";
    let journal = TopicMigrationJournal {
        authority_file,
        authority_hash: stable_id_with_prefix("authority", &authority_json),
        stale_files,
    };
    write_topic_migration_journal_unlocked(journal_path, &journal)?;
    write(new_path, value)?;
    let raw = fs::read_to_string(new_path)?;
    let committed = serde_json::from_str::<T>(&raw).map_err(invalid_data)?;
    if !validate(&committed) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "new memory topic authority failed post-commit validation",
        ));
    }

    let mut cleanup_failures = Vec::new();
    for stale in stale_paths {
        if stale == new_path || !stale.exists() {
            continue;
        }
        if let Err(error) = remove(stale) {
            cleanup_failures.push(format!("{}: {error}", stale.display()));
        }
    }
    if cleanup_failures.is_empty() {
        if let Err(error) = remove(journal_path) {
            cleanup_failures.push(format!("{}: {error}", journal_path.display()));
        }
    }
    Ok(TopicMutation {
        value: value.clone(),
        cleanup_warning: (!cleanup_failures.is_empty()).then(|| cleanup_failures.join("; ")),
    })
}

pub(super) fn topic_migration_journal_path(new_path: &Path) -> io::Result<PathBuf> {
    let parent = new_path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "topic path has no parent"))?;
    let name = new_path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid topic filename"))?;
    Ok(parent.join(format!(".topic-migration-{name}.journal")))
}

fn write_topic_migration_journal_unlocked(
    journal_path: &Path,
    journal: &TopicMigrationJournal,
) -> io::Result<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let parent = journal_path.parent().unwrap_or_else(|| Path::new("."));
    let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staging = parent.join(format!(
        ".topic-journal-stage-{}-{sequence}.bin",
        std::process::id()
    ));
    let backup = parent.join(format!(
        ".topic-journal-backup-{}-{sequence}.bin",
        std::process::id()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        serde_json::to_writer_pretty(&mut file, journal).map_err(invalid_data)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        match crate::platform::filesystem::replace_file_atomically(&staging, journal_path, &backup)
        {
            Ok(crate::platform::filesystem::ReplaceState::Committed) => Ok(()),
            Ok(state) => Err(io::Error::other(format!(
                "unexpected journal replacement state: {state:?}"
            ))),
            Err(error) => Err(error.into_io_error()),
        }
    })();
    if result.is_err() {
        // The new authority is not written until the journal succeeds, so
        // these staging paths can never be required to recover user data.
        let _ = fs::remove_file(staging);
        let _ = fs::remove_file(backup);
    }
    result
}

pub(super) fn reconcile_topic_migration_journals_unlocked<T: serde::de::DeserializeOwned>(
    dir: &Path,
) -> io::Result<TopicReconciliation> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(TopicReconciliation::default());
        }
        Err(error) => return Err(error),
    };
    let mut hidden_files = BTreeSet::new();
    let mut cleanup_failures = Vec::new();
    for entry in entries {
        let journal_path = entry?.path();
        let Some(name) = journal_path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.starts_with(".topic-migration-") || !name.ends_with(".journal") {
            continue;
        }
        let journal = match fs::read_to_string(&journal_path).and_then(|raw| {
            serde_json::from_str::<TopicMigrationJournal>(&raw).map_err(invalid_data)
        }) {
            Ok(journal) => journal,
            Err(_) => {
                // An unparsable journal (truncated write or disk corruption)
                // must not permanently block loading preferences / work
                // context. Quarantine it so the next read stops tripping over
                // it while keeping the bytes for diagnosis.
                quarantine_unparsable_journal(&journal_path);
                continue;
            }
        };
        if !is_plain_filename(&journal.authority_file)
            || journal
                .stale_files
                .iter()
                .any(|name| !is_plain_filename(name))
        {
            quarantine_unparsable_journal(&journal_path);
            continue;
        }
        let authority = dir.join(&journal.authority_file);
        let authority_valid = fs::read_to_string(&authority).is_ok_and(|raw| {
            serde_json::from_str::<T>(&raw).is_ok()
                && stable_id_with_prefix("authority", &raw) == journal.authority_hash
        });
        if !authority_valid {
            fs::remove_file(&journal_path)?;
            continue;
        }
        let mut journal_cleanup_pending = false;
        for stale_name in &journal.stale_files {
            let stale = dir.join(stale_name);
            if stale == authority || !stale.exists() {
                continue;
            }
            if let Err(error) = fs::remove_file(&stale) {
                journal_cleanup_pending = true;
                hidden_files.insert(stale.clone());
                cleanup_failures.push(format!("{}: {error}", stale.display()));
            }
        }
        if !journal_cleanup_pending {
            if let Err(error) = fs::remove_file(&journal_path) {
                cleanup_failures.push(format!("{}: {error}", journal_path.display()));
            }
        }
    }
    Ok(TopicReconciliation {
        hidden_files,
        cleanup_warning: (!cleanup_failures.is_empty()).then(|| cleanup_failures.join("; ")),
    })
}

fn is_plain_filename(value: &str) -> bool {
    !value.is_empty()
        && Path::new(value).components().count() == 1
        && matches!(
            Path::new(value).components().next(),
            Some(std::path::Component::Normal(_))
        )
}

fn quarantine_unparsable_journal(journal_path: &Path) {
    let Some(name) = journal_path.file_name().and_then(|value| value.to_str()) else {
        return;
    };
    let quarantined = journal_path.with_file_name(format!("{name}.corrupt-{}", std::process::id()));
    if let Err(error) = fs::rename(journal_path, quarantined) {
        // Rename can fail if the file is momentarily locked (e.g. Windows);
        // the journal then stays in place and is skipped again on the next
        // read without blocking the load.
        let _ = error;
    }
}

fn work_context_stale_paths_unlocked(
    dir: &Path,
    new_path: &Path,
    old_topic: &str,
    new_topic: &str,
) -> io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(stale),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path == new_path || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let same_topic = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WorkContextFile>(&raw).ok())
            .map(|item| {
                let topic = normalize_work_context_topic(&item.topic);
                topic == old_topic || topic == new_topic
            })
            .unwrap_or(false);
        if same_topic {
            stale.push(path);
        }
    }
    Ok(stale)
}

pub fn update_work_context(
    id: &str,
    patch: MemoryTextPatch,
) -> io::Result<Option<TopicMutation<WorkContextFile>>> {
    let _guard = write_lock().lock();
    let _lifecycle = file_lifecycle_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(None);
    }
    let items = load_work_context_unlocked()?;
    let Some(existing) = items
        .into_iter()
        .find(|item| clean_id(&item.id) == id || clean_id(&item.topic) == id)
    else {
        return Ok(None);
    };
    let topic = patch
        .topic
        .as_deref()
        .map(normalize_work_context_topic)
        .unwrap_or_else(|| existing.topic.clone());
    let text = patch
        .text
        .as_deref()
        .map(|s| clean_candidate_sentence(s, 160))
        .unwrap_or_else(|| existing.text.clone());
    if text.is_empty() || looks_sensitive(&text) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "work context is empty or sensitive",
        ));
    }
    let new_id = stable_id_with_prefix("ctx", &topic);
    let mut updated = existing.clone();
    updated.id = new_id.clone();
    updated.topic = topic;
    updated.text = text;
    updated.updated_at = Utc::now().to_rfc3339();
    normalize_work_context(&mut updated);
    let dir = work_context_dir();
    fs::create_dir_all(&dir)?;
    let old_path = dir.join(format!("{}.json", existing.id));
    let new_path = dir.join(format!("{new_id}.json"));
    let mut stale_paths =
        work_context_stale_paths_unlocked(&dir, &new_path, &existing.topic, &updated.topic)?;
    if old_path != new_path && old_path.exists() && !stale_paths.contains(&old_path) {
        stale_paths.push(old_path);
    }
    let mutation =
        commit_topic_migration_unlocked(&new_path, &updated, &stale_paths, |committed| {
            committed.id == updated.id
                && committed.topic == updated.topic
                && committed.text == updated.text
        })?;
    Ok(Some(mutation))
}

pub fn delete_work_context(id: &str) -> io::Result<bool> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(false);
    }
    let dir = work_context_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let matched_by_file = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(clean_id)
            .map(|file_id| file_id == id)
            .unwrap_or(false);
        let matched_by_body = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<WorkContextFile>(&raw).ok())
            .map(|item| clean_id(&item.id) == id || clean_id(&item.topic) == id)
            .unwrap_or(false);
        if matched_by_file || matched_by_body {
            fs::remove_file(path)?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn load_current_focus() -> io::Result<Vec<TimedMemoryItem>> {
    load_timed_memory_file(&current_focus_path(), "current_focus")
}

pub fn load_recent_activity() -> io::Result<Vec<TimedMemoryItem>> {
    load_timed_memory_file(&recent_activity_path(), "recent_activity")
}

pub(super) fn upsert_timed_memory_unlocked(
    kind: &str,
    topic: &str,
    content: &str,
    source: &str,
    ttl_days: Option<i64>,
    confidence: f32,
) -> io::Result<TimedMemoryItem> {
    let kind = normalize_timed_memory_kind(kind);
    let now = Utc::now();
    let now_s = now.to_rfc3339();
    let default_ttl = if kind == "recent_activity" {
        RECENT_ACTIVITY_DEFAULT_TTL_DAYS
    } else {
        CURRENT_FOCUS_DEFAULT_TTL_DAYS
    };
    let ttl_days = ttl_days.unwrap_or(default_ttl).clamp(1, 90);
    let text = clean_candidate_sentence(content, 180);
    if text.is_empty() || looks_sensitive(&text) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "timed memory is empty or sensitive",
        ));
    }
    let topic = normalize_timed_memory_topic(&kind, topic);
    let path = timed_memory_path(&kind);
    let mut items = load_timed_memory_file(&path, &kind)?;
    let id = stable_id_with_prefix(
        if kind == "recent_activity" {
            "act"
        } else {
            "focus"
        },
        &format!("{kind}:{topic}:{text}"),
    );
    let item = if let Some(existing) = items.iter_mut().find(|item| {
        item.id == id
            || timed_memory_text_key(&item.text) == timed_memory_text_key(&text)
            || (item.status == "active"
                && item.kind == kind
                && normalize_timed_memory_topic(&kind, &item.topic) == topic
                && timed_memory_texts_are_related(&item.text, &text))
    }) {
        existing.kind = kind.clone();
        existing.topic = topic.clone();
        existing.text = text.clone();
        existing.source = clean_text(source, 40);
        existing.confidence = confidence;
        existing.status = "active".to_string();
        existing.updated_at = now_s.clone();
        existing.last_hit = now_s.clone();
        existing.ttl_days = ttl_days;
        existing.clone()
    } else {
        let item = TimedMemoryItem {
            id,
            kind: kind.clone(),
            topic,
            text,
            source: clean_text(source, 40),
            confidence,
            created_at: now_s.clone(),
            updated_at: now_s.clone(),
            last_hit: now_s.clone(),
            ttl_days,
            status: "active".to_string(),
        };
        items.push(item.clone());
        item
    };
    write_timed_memory_file(&path, &items, &kind)?;
    Ok(item)
}

pub(super) fn upsert_timed_memory_locked(
    kind: &str,
    topic: &str,
    content: &str,
    source: &str,
    ttl_days: Option<i64>,
    confidence: f32,
) -> io::Result<TimedMemoryItem> {
    let _guard = write_lock().lock();
    upsert_timed_memory_unlocked(kind, topic, content, source, ttl_days, confidence)
}

pub(super) fn archive_timed_memory_unlocked(kind: &str, id: &str) -> io::Result<bool> {
    let kind = normalize_timed_memory_kind(kind);
    let id = clean_id(id);
    let path = timed_memory_path(&kind);
    let now = Utc::now().to_rfc3339();
    let mut items = load_timed_memory_file(&path, &kind)?;
    let mut changed = false;
    for item in &mut items {
        if item.id == id && item.status != "archived" {
            item.status = "archived".to_string();
            item.updated_at = now.clone();
            changed = true;
        }
    }
    if changed {
        write_timed_memory_file(&path, &items, &kind)?;
    }
    Ok(changed)
}

pub fn update_timed_memory(
    kind: &str,
    id: &str,
    patch: MemoryTextPatch,
) -> io::Result<Option<TimedMemoryItem>> {
    let _guard = write_lock().lock();
    let kind = normalize_timed_memory_kind(kind);
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(None);
    }
    let path = timed_memory_path(&kind);
    let mut items = load_timed_memory_file(&path, &kind)?;
    let Some(item) = items.iter_mut().find(|item| clean_id(&item.id) == id) else {
        return Ok(None);
    };
    if let Some(topic) = patch.topic.as_deref() {
        item.topic = normalize_timed_memory_topic(&kind, topic);
    }
    if let Some(text) = patch.text.as_deref() {
        let text = clean_candidate_sentence(text, 180);
        if text.is_empty() || looks_sensitive(&text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "timed memory is empty or sensitive",
            ));
        }
        item.text = text;
    }
    if let Some(ttl_days) = patch.ttl_days {
        item.ttl_days = ttl_days.clamp(1, 90);
    }
    item.updated_at = Utc::now().to_rfc3339();
    item.last_hit = item.updated_at.clone();
    item.status = "active".to_string();
    let updated = item.clone();
    write_timed_memory_file(&path, &items, &kind)?;
    Ok(Some(updated))
}

pub fn delete_timed_memory(kind: &str, id: &str) -> io::Result<bool> {
    let _guard = write_lock().lock();
    let kind = normalize_timed_memory_kind(kind);
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(false);
    }
    let path = timed_memory_path(&kind);
    let mut items = load_timed_memory_file(&path, &kind)?;
    let before = items.len();
    items.retain(|item| clean_id(&item.id) != id);
    if items.len() == before {
        return Ok(false);
    }
    write_timed_memory_file(&path, &items, &kind)?;
    Ok(true)
}

fn load_timed_memory_file(path: &std::path::Path, kind: &str) -> io::Result<Vec<TimedMemoryItem>> {
    let raw = match read_text_recovering(path, json_lines_are_valid::<TimedMemoryItem>) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut out = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(mut item) = serde_json::from_str::<TimedMemoryItem>(line) else {
            continue;
        };
        item.kind = normalize_timed_memory_kind(if item.kind.is_empty() {
            kind
        } else {
            &item.kind
        });
        normalize_timed_memory(&mut item);
        if !item.id.is_empty() && !item.text.is_empty() {
            out.push(item);
        }
    }
    Ok(dedupe_timed_memory_items(out))
}

pub(super) fn write_timed_memory_file(
    path: &std::path::Path,
    items: &[TimedMemoryItem],
    kind: &str,
) -> io::Result<()> {
    let mut normalized = items.to_vec();
    for item in &mut normalized {
        item.kind = normalize_timed_memory_kind(if item.kind.is_empty() {
            kind
        } else {
            &item.kind
        });
        normalize_timed_memory(item);
    }
    normalized = dedupe_timed_memory_items(normalized);
    normalized = compact_timed_memory_items(normalized, kind);
    let mut lines = String::new();
    for item in normalized {
        if item.id.is_empty() || item.text.is_empty() {
            continue;
        }
        lines.push_str(&serde_json::to_string(&item).map_err(invalid_data)?);
        lines.push('\n');
    }
    write_text_atomic(path, &lines)
}

fn compact_timed_memory_items(mut items: Vec<TimedMemoryItem>, kind: &str) -> Vec<TimedMemoryItem> {
    let kind = normalize_timed_memory_kind(kind);
    items.sort_by(|a, b| {
        parse_time(&b.last_hit)
            .cmp(&parse_time(&a.last_hit))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let active_limit = if kind == "recent_activity" {
        RECENT_ACTIVITY_ACTIVE_MAX_STORED
    } else {
        CURRENT_FOCUS_ACTIVE_MAX_STORED
    };
    let mut active = Vec::new();
    let mut archived = Vec::new();
    for item in items {
        if item.status == "active" {
            if active.len() < active_limit {
                active.push(item);
            }
        } else if archived.len() < TIMED_MEMORY_ARCHIVED_MAX_STORED {
            archived.push(item);
        }
    }
    active.extend(archived);
    active
}

fn dedupe_timed_memory_items(mut items: Vec<TimedMemoryItem>) -> Vec<TimedMemoryItem> {
    items.sort_by(|a, b| {
        parse_time(&b.last_hit)
            .cmp(&parse_time(&a.last_hit))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let mut out: Vec<TimedMemoryItem> = Vec::new();
    'items: for item in items {
        for existing in &out {
            if existing.kind == item.kind
                && existing.topic == item.topic
                && existing.status == "active"
                && item.status == "active"
                && (timed_memory_text_key(&existing.text) == timed_memory_text_key(&item.text)
                    || timed_memory_texts_are_related(&existing.text, &item.text))
            {
                continue 'items;
            }
        }
        out.push(item);
    }
    out
}

pub fn load_pending_memory() -> io::Result<Vec<PendingMemoryItem>> {
    let path = pending_memory_path();
    let raw = match read_text_recovering(&path, json_lines_are_valid::<PendingMemoryItem>) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut out = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(mut item) = serde_json::from_str::<PendingMemoryItem>(line) else {
            continue;
        };
        normalize_pending_memory(&mut item);
        if !item.id.is_empty() && !item.content.is_empty() {
            out.push(item);
        }
    }
    Ok(out)
}

pub fn enqueue_memory_candidate(suggestion: MemorySuggestion) -> io::Result<PendingMemoryItem> {
    let _guard = write_lock().lock();
    let mut item = pending_item_from_suggestion(suggestion)?;
    if blocked_by_never_memory_unlocked(&item.content)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory candidate is blocked by user preference",
        ));
    }
    let now = Utc::now().to_rfc3339();
    let mut items = load_pending_memory()?;
    let item_content_key = pending_content_key(&item);
    if let Some(existing) = items.iter_mut().find(|existing| existing.id == item.id) {
        if existing.status != PENDING_STATUS_CONFIRMED
            || !confirmed_pending_memory_is_materialized(existing)
        {
            existing.seen_count = existing.seen_count.saturating_add(1);
            existing.status = PENDING_STATUS_PENDING.to_string();
            if !item.topic.is_empty() {
                existing.topic = item.topic.clone();
            }
            if !item.source.is_empty() {
                existing.source = item.source.clone();
            }
            existing.content = item.content.clone();
            existing.updated_at = now;
            let updated = existing.clone();
            write_pending_memory_unlocked(&items)?;
            return Ok(updated);
        }
        return Ok(existing.clone());
    }
    if let Some(existing) = items.iter_mut().find(|existing| {
        existing.status == PENDING_STATUS_PENDING
            && pending_content_key(existing) == item_content_key
    }) {
        existing.seen_count = existing.seen_count.saturating_add(1);
        existing.status = PENDING_STATUS_PENDING.to_string();
        if existing.topic.is_empty() && !item.topic.is_empty() {
            existing.topic = item.topic.clone();
        }
        if existing.source.is_empty() && !item.source.is_empty() {
            existing.source = item.source.clone();
        }
        existing.updated_at = now;
        let updated = existing.clone();
        write_pending_memory_unlocked(&items)?;
        return Ok(updated);
    }
    item.created_at = now.clone();
    item.updated_at = now;
    items.push(item.clone());
    write_pending_memory_unlocked(&items)?;
    Ok(item)
}

pub(super) fn confirmed_pending_memory_is_materialized(item: &PendingMemoryItem) -> bool {
    if item.status != PENDING_STATUS_CONFIRMED {
        return false;
    }
    match item.kind.as_str() {
        "profile" if item.topic == "call_name" => load_profile()
            .map(|profile| {
                profile.identity.call_name == normalize_profile_label(&item.content, "call_name")
            })
            .unwrap_or(false),
        "profile" if item.topic == "assistant_alias" => load_profile()
            .map(|profile| {
                profile.identity.assistant_alias
                    == normalize_profile_label(&item.content, "assistant_alias")
            })
            .unwrap_or(false),
        "preference" => {
            let topic = normalize_preference_topic(&item.topic);
            load_preferences()
                .map(|prefs| {
                    prefs.iter().any(|pref| {
                        normalize_preference_topic(&pref.topic) == topic
                            && pref.text == item.content
                    })
                })
                .unwrap_or(false)
        }
        "work_context" => {
            let topic = normalize_work_context_topic(&item.topic);
            load_work_context()
                .map(|items| {
                    items.iter().any(|ctx| {
                        normalize_work_context_topic(&ctx.topic) == topic
                            && memory_texts_cover_same_fact(&ctx.text, &item.content)
                    })
                })
                .unwrap_or(false)
        }
        "current_focus" | "recent_activity" => {
            let loader = if item.kind == "recent_activity" {
                load_recent_activity
            } else {
                load_current_focus
            };
            loader()
                .map(|items| {
                    items.iter().any(|memory| {
                        memory.status == "active"
                            && memory_texts_cover_same_fact(&memory.text, &item.content)
                    })
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

pub fn confirm_pending_memory(id: &str) -> io::Result<Option<MemoryWriteEvent>> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    let now = Utc::now().to_rfc3339();
    let mut items = load_pending_memory()?;
    let Some(item) = items.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    if item.status == PENDING_STATUS_CONFIRMED {
        return Ok(Some(MemoryWriteEvent {
            kind: item.kind.clone(),
            action: "confirmed".to_string(),
            id: item.id.clone(),
            text: item.content.clone(),
        }));
    }

    match item.kind.as_str() {
        "preference" => write_preference_unlocked(item)?,
        "profile" if item.topic == "call_name" => {
            let mut profile = load_profile()?;
            profile.identity.call_name = item.content.clone();
            profile.revision = profile.revision.saturating_add(1);
            profile.updated_at = now.clone();
            profile.normalize();
            write_json_atomic(&profile_path(), &profile)?;
        }
        "profile" if item.topic == "assistant_alias" => {
            let mut profile = load_profile()?;
            profile.identity.assistant_alias = item.content.clone();
            profile.revision = profile.revision.saturating_add(1);
            profile.updated_at = now.clone();
            profile.normalize();
            write_json_atomic(&profile_path(), &profile)?;
        }
        "recent_work" => {
            let _ = upsert_recent_work_unlocked(RecentWorkPatch {
                id: None,
                title: item.content.clone(),
                summary: if item.topic.is_empty() {
                    None
                } else {
                    Some(item.topic.clone())
                },
                source: Some(if item.source.is_empty() {
                    "memory_candidate".to_string()
                } else {
                    item.source.clone()
                }),
                ttl_days: None,
            })?;
        }
        "current_focus" | "recent_activity" => {
            let _ = upsert_timed_memory_unlocked(
                &item.kind,
                &item.topic,
                &item.content,
                if item.source.is_empty() {
                    "memory_candidate"
                } else {
                    &item.source
                },
                None,
                0.86,
            )?;
        }
        "work_context" => {
            let _ = upsert_work_context_unlocked(
                &MemorySuggestion {
                    kind: item.kind.clone(),
                    topic: item.topic.clone(),
                    content: item.content.clone(),
                    source: if item.source.is_empty() {
                        "memory_candidate".to_string()
                    } else {
                        item.source.clone()
                    },
                },
                0.86,
            )?;
        }
        _ => write_preference_unlocked(item)?,
    }

    item.status = PENDING_STATUS_CONFIRMED.to_string();
    item.updated_at = now;
    let event = MemoryWriteEvent {
        kind: item.kind.clone(),
        action: "confirmed".to_string(),
        id: item.id.clone(),
        text: item.content.clone(),
    };
    write_pending_memory_unlocked(&items)?;
    Ok(Some(event))
}

pub fn ignore_pending_memory(id: &str) -> io::Result<Option<MemoryWriteEvent>> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    let now = Utc::now().to_rfc3339();
    let mut items = load_pending_memory()?;
    let Some(item) = items.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    item.status = PENDING_STATUS_IGNORED.to_string();
    item.updated_at = now;
    let event = MemoryWriteEvent {
        kind: item.kind.clone(),
        action: "ignored".to_string(),
        id: item.id.clone(),
        text: item.content.clone(),
    };
    write_pending_memory_unlocked(&items)?;
    Ok(Some(event))
}

pub fn never_pending_memory(
    id: &str,
    reason: Option<String>,
) -> io::Result<Option<MemoryWriteEvent>> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    let mut items = load_pending_memory()?;
    let Some(item) = items.iter_mut().find(|item| item.id == id) else {
        return Ok(None);
    };
    let now = Utc::now().to_rfc3339();
    item.status = PENDING_STATUS_IGNORED.to_string();
    item.updated_at = now.clone();

    let mut never_items = load_never_memory_unlocked()?;
    if never_items
        .iter()
        .all(|never| never.pattern != item.content)
    {
        never_items.push(NeverMemoryItem {
            id: stable_id_with_prefix("never", &item.content),
            pattern: item.content.clone(),
            reason: reason
                .as_deref()
                .map(|s| clean_text(s, 80))
                .unwrap_or_default(),
            created_at: now,
        });
        write_never_memory_unlocked(&never_items)?;
    }
    let event = MemoryWriteEvent {
        kind: item.kind.clone(),
        action: "never".to_string(),
        id: item.id.clone(),
        text: item.content.clone(),
    };
    write_pending_memory_unlocked(&items)?;
    Ok(Some(event))
}

pub fn load_never_memory() -> io::Result<Vec<NeverMemoryItem>> {
    load_never_memory_unlocked()
}

pub fn review_turn_candidates(user: &str, _assistant: &str) -> io::Result<Vec<PendingMemoryItem>> {
    let suggestions = super::llm_review::discover_turn_suggestions(user);
    let mut items = Vec::new();
    for suggestion in suggestions {
        match enqueue_memory_candidate(suggestion) {
            Ok(item) => items.push(item),
            Err(err) if err.kind() == io::ErrorKind::InvalidInput => {}
            Err(err) => return Err(err),
        }
    }
    Ok(items)
}

pub fn refresh_recent_work_expiry() -> io::Result<usize> {
    let _guard = write_lock().lock();
    let now = Utc::now();
    Ok(refresh_recent_work_expiry_unlocked(now)?
        + refresh_timed_memory_expiry_unlocked("current_focus", now)?
        + refresh_timed_memory_expiry_unlocked("recent_activity", now)?)
}

pub(super) fn refresh_recent_work_expiry_unlocked(now: DateTime<Utc>) -> io::Result<usize> {
    let mut items = load_recent_work()?;
    let mut changed = 0usize;
    for item in &mut items {
        if item.status == "active" && recent_work_is_expired(item, now) {
            item.status = "archived".to_string();
            item.updated_at = now.to_rfc3339();
            changed += 1;
        }
    }
    if changed > 0 {
        write_recent_work_unlocked(&items)?;
    }
    Ok(changed)
}

pub(super) fn refresh_timed_memory_expiry_unlocked(
    kind: &str,
    now: DateTime<Utc>,
) -> io::Result<usize> {
    let kind = normalize_timed_memory_kind(kind);
    let path = timed_memory_path(&kind);
    let mut items = load_timed_memory_file(&path, &kind)?;
    let mut changed = 0usize;
    for item in &mut items {
        if item.status == "active" && timed_memory_is_expired(item, now) {
            item.status = "archived".to_string();
            item.updated_at = now.to_rfc3339();
            changed += 1;
        }
    }
    if changed > 0 {
        write_timed_memory_file(&path, &items, &kind)?;
    }
    Ok(changed)
}

pub fn memory_enabled() -> bool {
    UserPrefs::load().memory_enabled
}

pub(super) fn disabled_runtime_snapshot(session_id: &str) -> io::Result<RuntimeMemorySnapshot> {
    let path = runtime_prompt_path(session_id);
    write_text_atomic(&path, "")?;
    Ok(RuntimeMemorySnapshot {
        session_id: session_id.to_string(),
        runtime_path: path.display().to_string(),
        block: String::new(),
        items: Vec::new(),
    })
}

pub fn list_preferences() -> io::Result<Vec<PreferenceFile>> {
    load_preferences()
}

pub fn list_preferences_with_cleanup() -> io::Result<TopicRead<Vec<PreferenceFile>>> {
    load_preferences_with_cleanup()
}

fn preference_stale_paths_unlocked(
    dir: &Path,
    new_path: &Path,
    old_topic: &str,
    new_topic: &str,
) -> io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(stale),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if path == new_path || path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let same_topic = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PreferenceFile>(&raw).ok())
            .map(|item| {
                let topic = normalize_preference_topic(&item.topic);
                topic == old_topic || topic == new_topic
            })
            .unwrap_or(false);
        if same_topic {
            stale.push(path);
        }
    }
    Ok(stale)
}

pub fn update_preference(
    id: &str,
    patch: MemoryTextPatch,
) -> io::Result<Option<TopicMutation<PreferenceFile>>> {
    let _guard = write_lock().lock();
    let _lifecycle = file_lifecycle_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(None);
    }
    let prefs = load_preferences_unlocked()?;
    let Some(existing) = prefs.into_iter().find(|pref| clean_id(&pref.id) == id) else {
        return Ok(None);
    };
    let topic = patch
        .topic
        .as_deref()
        .map(normalize_preference_topic)
        .unwrap_or_else(|| existing.topic.clone());
    let text = patch
        .text
        .as_deref()
        .map(|s| clean_candidate_sentence(s, 120))
        .unwrap_or_else(|| existing.text.clone());
    if text.is_empty()
        || looks_sensitive_or_task_like(&text)
        || looks_like_profile_preference_text(&text)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "preference is empty, sensitive, or not a preference",
        ));
    }
    let new_id = stable_id_with_prefix("pref", &topic);
    let updated = PreferenceFile {
        id: new_id.clone(),
        topic: topic.clone(),
        scope: if existing.scope.is_empty() {
            "unconditional".to_string()
        } else {
            existing.scope
        },
        text,
    };
    let dir = paths::user_memory_preferences_dir();
    fs::create_dir_all(&dir)?;
    let old_path = dir.join(format!("{}.json", existing.id));
    let new_path = dir.join(format!("{new_id}.json"));
    let mut stale_paths =
        preference_stale_paths_unlocked(&dir, &new_path, &existing.topic, &updated.topic)?;
    if old_path != new_path && old_path.exists() && !stale_paths.contains(&old_path) {
        stale_paths.push(old_path);
    }
    let mutation =
        commit_topic_migration_unlocked(&new_path, &updated, &stale_paths, |committed| {
            committed.id == updated.id
                && committed.topic == updated.topic
                && committed.text == updated.text
        })?;
    Ok(Some(mutation))
}

pub fn delete_preference(id: &str) -> io::Result<bool> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(false);
    }
    let dir = paths::user_memory_preferences_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let file_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(clean_id)
            .unwrap_or_default();
        let matched_by_file = file_id == id;
        let matched_by_body = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PreferenceFile>(&raw).ok())
            .map(|pref| clean_id(&pref.id) == id)
            .unwrap_or(false);
        if matched_by_file || matched_by_body {
            fs::remove_file(path)?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn load_preferences() -> io::Result<Vec<PreferenceFile>> {
    load_preferences_with_cleanup().map(|result| result.value)
}

fn load_preferences_with_cleanup() -> io::Result<TopicRead<Vec<PreferenceFile>>> {
    let _lifecycle = file_lifecycle_lock().lock();
    load_preferences_with_cleanup_unlocked()
}

fn load_preferences_unlocked() -> io::Result<Vec<PreferenceFile>> {
    load_preferences_with_cleanup_unlocked().map(|result| result.value)
}

fn load_preferences_with_cleanup_unlocked() -> io::Result<TopicRead<Vec<PreferenceFile>>> {
    let dir = paths::user_memory_preferences_dir();
    recover_directory_json_files_unlocked::<PreferenceFile>(&dir)?;
    let reconciliation = reconcile_topic_migration_journals_unlocked::<PreferenceFile>(&dir)?;
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(TopicRead {
                value: Vec::new(),
                cleanup_warning: reconciliation.cleanup_warning,
            });
        }
        Err(err) => return Err(err),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if reconciliation.hidden_files.contains(&path) {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = read_text_recovering_unlocked(&path, &|raw| {
            serde_json::from_str::<PreferenceFile>(raw).is_ok()
        })?;
        let mut pref = serde_json::from_str::<PreferenceFile>(&raw).map_err(invalid_data)?;
        pref.id = clean_scalar(&pref.id);
        if pref.id.is_empty() {
            pref.id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(clean_scalar)
                .unwrap_or_default();
        }
        pref.topic = normalize_preference_topic(&pref.topic);
        pref.scope = clean_scalar(&pref.scope);
        pref.text = clean_scalar(&pref.text);
        if !pref.text.is_empty() && !looks_like_profile_preference_text(&pref.text) {
            records.push((path, pref));
        }
    }
    let (mut out, authority_cleanup_failures) =
        resolve_topic_authorities(records, |item| &item.topic, |item| &item.id);
    out.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.text.cmp(&b.text)));
    let mut cleanup_parts = Vec::new();
    if let Some(base) = reconciliation.cleanup_warning {
        cleanup_parts.push(base);
    }
    cleanup_parts.extend(authority_cleanup_failures);
    Ok(TopicRead {
        value: out,
        cleanup_warning: (!cleanup_parts.is_empty()).then(|| cleanup_parts.join("; ")),
    })
}

fn write_preference_unlocked(item: &PendingMemoryItem) -> io::Result<()> {
    if looks_like_profile_preference_text(&item.content) {
        return Ok(());
    }
    let topic = normalize_preference_topic(&item.topic);
    let id = stable_id_with_prefix("pref", &topic);
    let preference = PreferenceFile {
        id: id.clone(),
        topic: topic.clone(),
        scope: "unconditional".to_string(),
        text: item.content.clone(),
    };
    let dir = paths::user_memory_preferences_dir();
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!("{id}.json"));
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == target || path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let same_topic = fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<PreferenceFile>(&raw).ok())
                .map(|pref| normalize_preference_topic(&pref.topic) == topic)
                .unwrap_or(false);
            if same_topic {
                let _ = fs::remove_file(path);
            }
        }
    }
    write_json_atomic(&target, &preference)
}

fn blocked_by_never_memory_unlocked(content: &str) -> io::Result<bool> {
    let content = clean_text(content, 120);
    if content.is_empty() {
        return Ok(false);
    }
    Ok(load_never_memory_unlocked()?.iter().any(|never| {
        never.pattern == content
            || (!never.pattern.is_empty()
                && (content.contains(&never.pattern) || never.pattern.contains(&content)))
    }))
}

fn load_never_memory_unlocked() -> io::Result<Vec<NeverMemoryItem>> {
    let path = never_memory_path();
    let raw = match read_text_recovering(&path, json_lines_are_valid::<NeverMemoryItem>) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut out = Vec::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(mut item) = serde_json::from_str::<NeverMemoryItem>(line) else {
            continue;
        };
        item.id = clean_id(&item.id);
        item.pattern = clean_text(&item.pattern, 120);
        item.reason = clean_text(&item.reason, 80);
        if !item.id.is_empty() && !item.pattern.is_empty() {
            out.push(item);
        }
    }
    Ok(out)
}

pub(super) fn write_never_memory_unlocked(items: &[NeverMemoryItem]) -> io::Result<()> {
    let normalized = compact_never_memory_items(items);
    let mut lines = String::new();
    for item in normalized {
        if item.id.is_empty() || item.pattern.is_empty() {
            continue;
        }
        lines.push_str(&serde_json::to_string(&item).map_err(invalid_data)?);
        lines.push('\n');
    }
    write_text_atomic(&never_memory_path(), &lines)
}

fn compact_never_memory_items(items: &[NeverMemoryItem]) -> Vec<NeverMemoryItem> {
    let mut normalized: Vec<NeverMemoryItem> = items
        .iter()
        .filter_map(|item| {
            let mut item = item.clone();
            item.id = clean_id(&item.id);
            item.pattern = clean_text(&item.pattern, 120);
            item.reason = clean_text(&item.reason, 80);
            (!item.id.is_empty() && !item.pattern.is_empty()).then_some(item)
        })
        .collect();
    normalized.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut out: Vec<NeverMemoryItem> = Vec::new();
    for item in normalized {
        if out.iter().any(|existing| existing.pattern == item.pattern) {
            continue;
        }
        if out.len() >= NEVER_MEMORY_MAX_STORED {
            break;
        }
        out.push(item);
    }
    out
}

pub(super) fn write_recent_work_unlocked(items: &[RecentWorkItem]) -> io::Result<()> {
    let normalized = compact_recent_work_items(items);
    let mut lines = String::new();
    for item in normalized {
        if item.id.is_empty() || item.title.is_empty() {
            continue;
        }
        lines.push_str(&serde_json::to_string(&item).map_err(invalid_data)?);
        lines.push('\n');
    }
    write_text_atomic(&recent_work_path(), &lines)
}

fn compact_recent_work_items(items: &[RecentWorkItem]) -> Vec<RecentWorkItem> {
    let mut normalized: Vec<RecentWorkItem> = items
        .iter()
        .filter_map(|item| {
            let mut item = item.clone();
            normalize_recent_work(&mut item);
            (!item.id.is_empty() && !item.title.is_empty()).then_some(item)
        })
        .collect();
    normalized.sort_by(|a, b| {
        parse_time(&b.last_hit)
            .cmp(&parse_time(&a.last_hit))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let mut active = Vec::new();
    let mut archived = Vec::new();
    for item in normalized {
        if item.status == "active" {
            if active.len() < RECENT_WORK_ACTIVE_MAX_STORED {
                active.push(item);
            }
        } else if archived.len() < RECENT_WORK_ARCHIVED_MAX_STORED {
            archived.push(item);
        }
    }
    active.extend(archived);
    active
}

fn normalize_recent_work(item: &mut RecentWorkItem) {
    item.id = clean_id(&item.id);
    item.title = clean_text(&item.title, 50);
    item.summary = clean_text(&item.summary, 80);
    item.status = match clean_text(&item.status, 20).as_str() {
        "active" => "active".to_string(),
        _ => "archived".to_string(),
    };
    item.source = clean_text(&item.source, 40);
}

fn normalize_work_context(item: &mut WorkContextFile) {
    item.id = clean_id(&item.id);
    item.kind = "work_context".to_string();
    item.topic = normalize_work_context_topic(&item.topic);
    item.text = clean_text(&item.text, 160);
    item.source = clean_text(&item.source, 40);
    if item.id.is_empty() && !item.topic.is_empty() {
        item.id = stable_id_with_prefix("ctx", &item.topic);
    }
}

fn timed_memory_path(kind: &str) -> PathBuf {
    if normalize_timed_memory_kind(kind) == "recent_activity" {
        recent_activity_path()
    } else {
        current_focus_path()
    }
}

fn normalize_timed_memory(item: &mut TimedMemoryItem) {
    item.id = clean_id(&item.id);
    item.kind = normalize_timed_memory_kind(&item.kind);
    item.topic = normalize_timed_memory_topic(&item.kind, &item.topic);
    item.text = clean_text(&item.text, 180);
    item.source = clean_text(&item.source, 40);
    item.status = match clean_text(&item.status, 20).as_str() {
        "active" => "active".to_string(),
        _ => "archived".to_string(),
    };
    item.ttl_days = item.ttl_days.clamp(1, 90);
}

fn timed_memory_text_key(text: &str) -> String {
    clean_text(text, 120)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("")
        .to_lowercase()
}

fn timed_memory_texts_are_related(left: &str, right: &str) -> bool {
    let left = memory_similarity_bigrams(left);
    let right = memory_similarity_bigrams(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let shared = left.iter().filter(|token| right.contains(token)).count();
    let smaller = left.len().min(right.len()).max(1);
    shared >= 3 && shared * 100 / smaller >= 30
}

fn memory_texts_cover_same_fact(left: &str, right: &str) -> bool {
    let left_key = timed_memory_text_key(left);
    let right_key = timed_memory_text_key(right);
    !left_key.is_empty()
        && !right_key.is_empty()
        && (left_key == right_key
            || left_key.contains(&right_key)
            || right_key.contains(&left_key)
            || timed_memory_texts_are_related(left, right))
}

fn memory_similarity_bigrams(value: &str) -> Vec<String> {
    let normalized: String = clean_text(value, 180)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(c))
        .collect();
    let chars = normalized.chars().collect::<Vec<_>>();
    if chars.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for window in chars.windows(2) {
        let token = window.iter().collect::<String>();
        if !out.contains(&token) {
            out.push(token);
        }
    }
    out
}

fn timed_memory_is_expired(item: &TimedMemoryItem, now: DateTime<Utc>) -> bool {
    parse_time(&item.last_hit)
        .map(|last_hit| now.signed_duration_since(last_hit) > Duration::days(item.ttl_days))
        .unwrap_or(false)
}

pub(super) fn active_timed_memory(
    items: &[TimedMemoryItem],
    now: DateTime<Utc>,
) -> Vec<&TimedMemoryItem> {
    let mut active: Vec<&TimedMemoryItem> = items
        .iter()
        .filter(|item| item.status == "active" && !timed_memory_is_expired(item, now))
        .collect();
    active.sort_by(|a, b| {
        parse_time(&b.last_hit)
            .cmp(&parse_time(&a.last_hit))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    active
}

fn recent_work_is_expired(item: &RecentWorkItem, now: DateTime<Utc>) -> bool {
    if parse_time(&item.expires_at).is_some_and(|expires_at| expires_at <= now) {
        return true;
    }
    parse_time(&item.last_hit)
        .map(|last_hit| {
            now.signed_duration_since(last_hit) > Duration::days(RECENT_WORK_DEFAULT_TTL_DAYS)
        })
        .unwrap_or(false)
}

pub(super) fn active_recent_work(
    items: &[RecentWorkItem],
    now: DateTime<Utc>,
) -> Vec<&RecentWorkItem> {
    let mut active: Vec<&RecentWorkItem> = items
        .iter()
        .filter(|item| item.status == "active" && !recent_work_is_expired(item, now))
        .collect();
    active.sort_by(|a, b| {
        parse_time(&b.last_hit)
            .cmp(&parse_time(&a.last_hit))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    active
}

pub(super) fn pending_item_from_suggestion(
    suggestion: MemorySuggestion,
) -> io::Result<PendingMemoryItem> {
    let kind = match clean_text(&suggestion.kind, 20).as_str() {
        "profile" => "profile".to_string(),
        "work_context" => "work_context".to_string(),
        "current_focus" => "current_focus".to_string(),
        "recent_activity" => "recent_activity".to_string(),
        "recent_work" => "current_focus".to_string(),
        _ => "preference".to_string(),
    };
    let mut topic = clean_text(&suggestion.topic, 40);
    if kind == "preference" {
        topic = normalize_preference_topic(&topic);
    }
    let content = clean_text(&suggestion.content, 120);
    if content.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory candidate content is empty",
        ));
    }
    let invalid = if matches!(
        kind.as_str(),
        "current_focus" | "recent_activity" | "work_context"
    ) {
        looks_sensitive(&content)
    } else {
        kind != "profile" && looks_sensitive_or_task_like(&content)
    };
    if invalid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "memory candidate looks sensitive or task-like",
        ));
    }
    let source = clean_text(&suggestion.source, 40);
    let id = stable_id_with_prefix("pending", &format!("{kind}:{topic}:{content}"));
    Ok(PendingMemoryItem {
        id,
        kind,
        topic,
        content,
        source,
        status: PENDING_STATUS_PENDING.to_string(),
        seen_count: 1,
        created_at: String::new(),
        updated_at: String::new(),
    })
}

fn normalize_pending_memory(item: &mut PendingMemoryItem) {
    item.id = clean_id(&item.id);
    item.kind = match clean_text(&item.kind, 20).as_str() {
        "profile" => "profile".to_string(),
        "work_context" => "work_context".to_string(),
        "current_focus" => "current_focus".to_string(),
        "recent_activity" => "recent_activity".to_string(),
        "recent_work" => "current_focus".to_string(),
        _ => "preference".to_string(),
    };
    item.topic = clean_text(&item.topic, 40);
    item.content = clean_text(&item.content, 120);
    item.source = clean_text(&item.source, 40);
    item.status = match clean_text(&item.status, 30).as_str() {
        PENDING_STATUS_CONFIRMED => PENDING_STATUS_CONFIRMED.to_string(),
        PENDING_STATUS_IGNORED => PENDING_STATUS_IGNORED.to_string(),
        PENDING_STATUS_OBSERVED => PENDING_STATUS_OBSERVED.to_string(),
        _ => PENDING_STATUS_PENDING.to_string(),
    };
}

fn pending_content_key(item: &PendingMemoryItem) -> String {
    format!(
        "{}:{}",
        item.kind,
        item.content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
    .to_lowercase()
}

pub(super) fn write_pending_memory_unlocked(items: &[PendingMemoryItem]) -> io::Result<()> {
    let normalized = compact_pending_memory_items(items);
    let mut lines = String::new();
    for item in normalized {
        if item.id.is_empty() || item.content.is_empty() {
            continue;
        }
        lines.push_str(&serde_json::to_string(&item).map_err(invalid_data)?);
        lines.push('\n');
    }
    write_text_atomic(&pending_memory_path(), &lines)
}

fn compact_pending_memory_items(items: &[PendingMemoryItem]) -> Vec<PendingMemoryItem> {
    let mut normalized: Vec<PendingMemoryItem> = items
        .iter()
        .filter_map(|item| {
            let mut item = item.clone();
            normalize_pending_memory(&mut item);
            (!item.id.is_empty() && !item.content.is_empty()).then_some(item)
        })
        .collect();
    normalized.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.created_at.cmp(&a.created_at))
    });
    let mut active = Vec::new();
    let mut resolved = Vec::new();
    for item in normalized {
        if matches!(
            item.status.as_str(),
            PENDING_STATUS_PENDING | PENDING_STATUS_OBSERVED
        ) {
            if active.len() < PENDING_MEMORY_ACTIVE_MAX_STORED {
                active.push(item);
            }
        } else if resolved.len() < PENDING_MEMORY_RESOLVED_MAX_STORED {
            resolved.push(item);
        }
    }
    active.extend(resolved);
    active
}
