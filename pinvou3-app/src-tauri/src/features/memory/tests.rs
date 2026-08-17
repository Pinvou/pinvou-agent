//! Tests for the memory feature. 抽离自 `mod.rs` 的 `#[cfg(test)] mod tests`，
//! 逐字保留原测试体，仅通过 `use` 把拆分到各子模块的内部 helper 重新引入作用域。
// architecture-guard: allow-target-cfg -- 记忆持久化测试需用 Windows 独占句柄覆盖 ReplaceFileW 恢复路径（自 mod.rs 拆分迁入，原豁免随文件迁移）

use std::fs;
use std::io;
use std::path::PathBuf;

use chrono::{Duration, Utc};
use serde_json::json;

use crate::platform::paths;
use crate::platform::prefs::ModelPreset;

use super::io::{
    commit_topic_migration_unlocked_with, current_focus_path, enqueue_memory_candidate,
    is_delivery_tool, load_preferences, load_profile, pending_item_from_suggestion,
    reconcile_topic_migration_journals_unlocked, summarize_tool_start,
    topic_migration_journal_path, upsert_timed_memory_unlocked, write_never_memory_unlocked,
    write_pending_memory_unlocked, write_recent_work_unlocked, write_timed_memory_file,
};
use super::llm_review::{
    append_memory_review_diagnostic_to, apply_llm_memory_review,
    apply_memory_review_reasoning_controls, assistant_suggests_delivery_complete,
    discover_turn_suggestions, has_memory_review_signal, memory_review_error_stage,
    parse_llm_memory_review, sanitize_llm_memory_item, LLM_REVIEW_PROMPT,
};
use super::render::render_from_parts;
// 引入全部常量（MAX_STORED / PENDING_STATUS_* / PROFILE_VERSION / Llm* 实体）。
use super::types::*;

// 重新暴露 super::* 上的 pub 面（MemoryProfile / ProfileIdentity / ... ）
use super::*;

#[allow(unused_imports)]
use super::util::{
    looks_completed_work_status, looks_recent_work_status, looks_sensitive, looks_task_like,
};

#[allow(unused_imports)]
use super::util::{
    file_lifecycle_lock, is_transient_windows_lock, json_lines_are_valid,
    promote_recovery_candidate, read_text_recovering, read_text_recovering_unlocked_with,
    recover_directory_json_files, recover_directory_json_files_unlocked, stable_id_with_prefix,
    write_json_atomic, write_json_atomic_unlocked, write_text_atomic_unlocked_with,
};

struct IsolatedPinvouHome {
    root: PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn recovery_test_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "pinvou-memory-recovery-{name}-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn generic_recovery_promotes_newest_valid_candidate_without_hard_links() {
    let root = recovery_test_root("multiple");
    let target = root.join("pending.jsonl");
    let backup = target.with_extension("bak");
    let older = target.with_extension("tmp-1-1-1");
    let newer = target.with_extension("tmp-1-2-2");
    fs::write(&backup, "invalid\n").unwrap();
    fs::write(&older, "{\"id\":1}\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    fs::write(&newer, "{\"id\":2}\n").unwrap();

    let restored = read_text_recovering(&target, |raw| {
        json_lines_are_valid::<serde_json::Value>(raw)
    })
    .unwrap();
    assert!(restored.contains("\"id\":2"));
    assert_eq!(fs::read_to_string(&target).unwrap(), restored);
    assert!(!backup.exists());
    assert!(!older.exists());
    assert!(!newer.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn valid_recovery_candidate_with_failed_promotion_is_an_error() {
    let root = recovery_test_root("promotion-failure");
    let target = root.join("profile.json");
    fs::write(target.with_extension("bak"), "{\"version\":1}\n").unwrap();

    let error = read_text_recovering_unlocked_with(
        &target,
        &|raw| serde_json::from_str::<MemoryProfile>(raw).is_ok(),
        &|_, _, _| Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked")),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!target.exists());
    assert!(target.with_extension("bak").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn recovery_prefers_replacefile_backup_over_surviving_replacement() {
    let root = recovery_test_root("replacefile-backup");
    let target = root.join("profile.json");
    let backup = target.with_extension("bak");
    let replacement = target.with_extension("tmp-1-2-3");
    fs::write(&backup, "{\"version\":1,\"revision\":7}\n").unwrap();
    fs::write(&replacement, "{\"version\":1,\"revision\":8}\n").unwrap();

    let restored = read_text_recovering(&target, |raw| {
        serde_json::from_str::<MemoryProfile>(raw).is_ok()
    })
    .unwrap();

    assert!(restored.contains("\"revision\":7"));
    assert!(!backup.exists());
    assert!(!replacement.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn transient_read_error_keeps_authoritative_target_untouched() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = recovery_test_root("transient-read-guard");
    let target = root.join("profile.json");
    let backup = target.with_extension("bak");
    let authoritative = "{\"version\":1,\"revision\":9,\"identity\":{\"call_name\":\"权威\",\"assistant_alias\":\"品悟\"}}\n";
    fs::write(&target, authoritative).unwrap();
    fs::write(
        &backup,
        "{\"version\":1,\"revision\":1,\"identity\":{\"call_name\":\"旧值\",\"assistant_alias\":\"品悟\"}}\n",
    )
    .unwrap();

    // Simulate a transient read failure (e.g. an antivirus lock on
    // Windows): the authoritative file exists but cannot be read right now.
    fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = fs::read_to_string(&target).is_err();
    if !unreadable {
        // Running as root (or on a filesystem that ignores mode bits)
        // bypasses the permission check, so the guard cannot be exercised
        // this way; skip instead of failing spuriously.
        let _ = fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600));
        let _ = fs::remove_dir_all(root);
        return;
    }
    // The target stays unreadable (mode 000) through the recovery call so
    // the transient-read guard is actually exercised: a stale backup must
    // not be promoted over the still-present authoritative file.
    let result = read_text_recovering(&target, |raw| {
        serde_json::from_str::<MemoryProfile>(raw).is_ok()
    });
    // Restore readability for the assertions and cleanup below; the
    // authoritative file was never overwritten by the recovery path.
    fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

    assert!(result.is_err());
    // The authoritative target must be preserved, never overwritten by a
    // stale backup, and the backup stays as a candidate.
    assert_eq!(fs::read_to_string(&target).unwrap(), authoritative);
    assert!(backup.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn windows_sharing_violation_is_transient() {
    // ERROR_SHARING_VIOLATION (32) and ERROR_LOCK_VIOLATION (33) surface as
    // ErrorKind::Uncategorized on Windows, outside the kind-based transient
    // whitelist; the raw-OS-code guard must still recognize them so recovery
    // never promotes a stale backup over a still-valid authoritative file.
    let sharing = io::Error::from_raw_os_error(32);
    let lock = io::Error::from_raw_os_error(33);
    #[cfg(windows)]
    {
        assert!(is_transient_windows_lock(&sharing));
        assert!(is_transient_windows_lock(&lock));
    }
    #[cfg(not(windows))]
    {
        assert!(!is_transient_windows_lock(&sharing));
        assert!(!is_transient_windows_lock(&lock));
    }
}

#[test]
fn memory_write_cleans_tmp_backup_on_permanently_occupied_target() {
    let root = recovery_test_root("dir-occupied");
    let target = root.join("profile.json");
    // A directory occupying the target path is a permanent failure: the
    // replacement can never be promoted, so the staged tmp/backup must be
    // cleaned instead of leaking — mirroring the artifact write path
    // (write_artifact_text_cleans_temp_file_on_error).
    fs::create_dir(&target).unwrap();

    let result = write_text_atomic_unlocked_with(&target, "new", |_, _, _| {
        Err(crate::platform::filesystem::ReplaceError::new(
            crate::platform::filesystem::ReplaceState::RecoveryRequired,
            io::Error::new(io::ErrorKind::AlreadyExists, "target is a directory"),
        ))
    });

    assert!(result.is_err());
    let leftovers: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| {
                    name.starts_with("profile.json.tmp-") || name == "profile.json.bak"
                })
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "staged tmp/backup must be cleaned: {leftovers:?}"
    );
    assert!(target.is_dir());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn recovery_promote_cleans_staged_recover_files_on_permanent_failure() {
    let root = recovery_test_root("recover-cleanup");
    let target = root.join("profile.json");
    let candidate = target.with_extension("tmp-1-1-1");
    fs::write(&candidate, "{\"version\":1}").unwrap();
    // Directory occupies the target: promotion can never succeed, so the
    // staged recover-*/recover-bak files must be dropped instead of
    // leaking one pair per failed read.
    fs::create_dir(&target).unwrap();

    let result = promote_recovery_candidate(&candidate, &target, b"{\"version\":1}");
    assert!(result.is_err());
    let leftovers: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("profile.json.recover-"))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "recover-* staging must be cleaned: {leftovers:?}"
    );
    // The original candidate stays for a later attempt and is re-scanned
    // by the next read.
    assert!(candidate.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn corrupted_authoritative_file_self_heals_from_backup() {
    let root = recovery_test_root("corrupt-self-heal");
    let target = root.join("profile.json");
    let backup = target.with_extension("bak");
    let authoritative = "{\"version\":1,\"revision\":9,\"identity\":{\"call_name\":\"权威\",\"assistant_alias\":\"品悟\"}}\n";
    // The authoritative file is deterministically corrupted (invalid
    // UTF-8) while a valid older backup exists. The deterministic error
    // must still fall through to recovery so the file self-heals.
    fs::write(&target, b"\xff\xfe not utf8").unwrap();
    fs::write(&backup, authoritative).unwrap();

    let restored = read_text_recovering(&target, |raw| {
        serde_json::from_str::<MemoryProfile>(raw).is_ok()
    })
    .unwrap();
    assert_eq!(restored, authoritative);
    assert_eq!(fs::read_to_string(&target).unwrap(), authoritative);
    let _ = fs::remove_dir_all(root);
}

fn preference_fixture(id: &str, topic: &str, text: &str) -> PreferenceFile {
    PreferenceFile {
        id: id.to_string(),
        topic: topic.to_string(),
        scope: "unconditional".to_string(),
        text: text.to_string(),
    }
}

#[test]
fn topic_migration_staging_failure_preserves_old_authority() {
    let root = recovery_test_root("topic-stage-failure");
    let old_path = root.join("old.json");
    let new_path = root.join("new.json");
    let journal = topic_migration_journal_path(&new_path).unwrap();
    let old = preference_fixture("old", "answer_style", "old value");
    let new = preference_fixture("new", "workflow_preference", "new value");
    write_json_atomic(&old_path, &old).unwrap();

    let error = commit_topic_migration_unlocked_with(
        &journal,
        &new_path,
        &new,
        std::slice::from_ref(&old_path),
        |_| true,
        |_, _| Err(io::Error::new(io::ErrorKind::Other, "staging failpoint")),
        |path| fs::remove_file(path),
    )
    .unwrap_err();

    assert!(error.to_string().contains("staging failpoint"));
    assert_eq!(
        serde_json::from_str::<PreferenceFile>(&fs::read_to_string(&old_path).unwrap())
            .unwrap()
            .text,
        "old value"
    );
    assert!(!new_path.exists());
    reconcile_topic_migration_journals_unlocked::<PreferenceFile>(&root).unwrap();
    assert!(!journal.exists());
    assert!(old_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn topic_migration_cleanup_failure_returns_warning_and_retries() {
    let root = recovery_test_root("topic-cleanup-retry");
    let old_path = root.join("old.json");
    let new_path = root.join("new.json");
    let journal = topic_migration_journal_path(&new_path).unwrap();
    let old = preference_fixture("old", "answer_style", "old value");
    let new = preference_fixture("new", "workflow_preference", "new value");
    write_json_atomic(&old_path, &old).unwrap();

    let mutation = commit_topic_migration_unlocked_with(
        &journal,
        &new_path,
        &new,
        std::slice::from_ref(&old_path),
        |value| value.id == "new",
        |path, value| write_json_atomic_unlocked(path, value),
        |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "occupied")),
    )
    .unwrap();

    assert_eq!(mutation.value.text, "new value");
    assert!(mutation
        .cleanup_warning
        .as_deref()
        .is_some_and(|warning| warning.contains("occupied")));
    assert!(old_path.exists());
    assert!(new_path.exists());
    assert!(journal.exists());

    reconcile_topic_migration_journals_unlocked::<PreferenceFile>(&root).unwrap();
    assert!(!old_path.exists());
    assert!(new_path.exists());
    assert!(!journal.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preference_and_work_context_topic_updates_commit_before_old_cleanup() {
    let home = IsolatedPinvouHome::new("topic-public-updates");
    let preference_dir = paths::user_memory_preferences_dir();
    let context_dir = work_context_dir();
    fs::create_dir_all(&preference_dir).unwrap();
    fs::create_dir_all(&context_dir).unwrap();
    let old_preference_id = stable_id_with_prefix("pref", "answer_style");
    let old_preference_path = preference_dir.join(format!("{old_preference_id}.json"));
    write_json_atomic(
        &old_preference_path,
        &preference_fixture(&old_preference_id, "answer_style", "old preference"),
    )
    .unwrap();
    let old_context_id = stable_id_with_prefix("ctx", "role_domain");
    let old_context_path = context_dir.join(format!("{old_context_id}.json"));
    write_json_atomic(
        &old_context_path,
        &WorkContextFile {
            id: old_context_id.clone(),
            kind: "work_context".to_string(),
            topic: "role_domain".to_string(),
            text: "old context".to_string(),
            source: "test".to_string(),
            confidence: 1.0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )
    .unwrap();

    let preference = update_preference(
        &old_preference_id,
        MemoryTextPatch {
            topic: Some("workflow_preference".to_string()),
            text: Some("new preference".to_string()),
            ttl_days: None,
        },
    )
    .unwrap()
    .unwrap();
    let context = update_work_context(
        &old_context_id,
        MemoryTextPatch {
            topic: Some("project_context".to_string()),
            text: Some("new context".to_string()),
            ttl_days: None,
        },
    )
    .unwrap()
    .unwrap();

    assert!(preference.cleanup_warning.is_none());
    assert_eq!(preference.value.topic, "workflow_preference");
    assert!(context.cleanup_warning.is_none());
    assert_eq!(context.value.topic, "project_context");
    assert!(!old_preference_path.exists());
    assert!(!old_context_path.exists());
    assert_eq!(list_preferences().unwrap().len(), 1);
    assert_eq!(load_work_context().unwrap().len(), 1);
    drop(home);
}

#[test]
fn topic_migration_lifecycle_hides_intermediate_duplicate_from_overview() {
    let home = IsolatedPinvouHome::new("topic-concurrent-overview");
    let dir = paths::user_memory_preferences_dir();
    fs::create_dir_all(&dir).unwrap();
    let old = preference_fixture("old", "answer_style", "old value");
    let new = preference_fixture("new", "workflow_preference", "new value");
    let old_path = dir.join("old.json");
    let new_path = dir.join("new.json");
    write_json_atomic(&old_path, &old).unwrap();
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let release = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writer_ready = ready.clone();
    let writer_release = release.clone();
    let writer = std::thread::spawn(move || {
        let _lifecycle = file_lifecycle_lock().lock();
        let journal = topic_migration_journal_path(&new_path).unwrap();
        commit_topic_migration_unlocked_with(
            &journal,
            &new_path,
            &new,
            std::slice::from_ref(&old_path),
            |_| true,
            |path, value| {
                write_json_atomic_unlocked(path, value)?;
                writer_ready.wait();
                writer_release.wait();
                Ok(())
            },
            |path| fs::remove_file(path),
        )
        .unwrap();
    });
    ready.wait();
    let (sent, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || sent.send(list_preferences()).unwrap());
    assert!(received
        .recv_timeout(std::time::Duration::from_millis(30))
        .is_err());
    release.wait();
    writer.join().unwrap();
    let visible = received
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .unwrap();
    reader.join().unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].text, "new value");
    drop(home);
}

#[cfg(windows)]
#[test]
fn topic_migration_preference_pending_cleanup_stays_available_and_hides_stale_id() {
    use std::os::windows::fs::OpenOptionsExt as _;

    let home = IsolatedPinvouHome::new("preference-pending-cleanup");
    let dir = paths::user_memory_preferences_dir();
    fs::create_dir_all(&dir).unwrap();
    let old_id = stable_id_with_prefix("pref", "answer_style");
    let old_path = dir.join(format!("{old_id}.json"));
    write_json_atomic(
        &old_path,
        &preference_fixture(&old_id, "answer_style", "old preference"),
    )
    .unwrap();
    let occupied = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&old_path)
        .unwrap();

    let updated = update_preference(
        &old_id,
        MemoryTextPatch {
            topic: Some("workflow_preference".to_string()),
            text: Some("new preference".to_string()),
            ttl_days: None,
        },
    )
    .unwrap()
    .unwrap();
    assert!(updated.cleanup_warning.is_some());
    let new_id = updated.value.id.clone();

    for _ in 0..2 {
        let read = list_preferences_with_cleanup().unwrap();
        assert!(read.cleanup_warning.is_some());
        assert_eq!(read.value.len(), 1);
        assert_eq!(read.value[0].id, new_id);
        assert_ne!(read.value[0].id, old_id);
    }
    assert!(old_path.exists());

    drop(occupied);
    let read = list_preferences_with_cleanup().unwrap();
    assert!(read.cleanup_warning.is_none());
    assert_eq!(read.value.len(), 1);
    assert_eq!(read.value[0].id, new_id);
    assert!(!old_path.exists());
    assert!(fs::read_dir(&dir).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".topic-migration-")
    }));
    drop(home);
}

#[cfg(windows)]
#[test]
fn topic_migration_work_context_pending_cleanup_stays_available_and_hides_stale_id() {
    use std::os::windows::fs::OpenOptionsExt as _;

    let home = IsolatedPinvouHome::new("work-context-pending-cleanup");
    let dir = work_context_dir();
    fs::create_dir_all(&dir).unwrap();
    let old_id = stable_id_with_prefix("ctx", "role_domain");
    let old_path = dir.join(format!("{old_id}.json"));
    write_json_atomic(
        &old_path,
        &WorkContextFile {
            id: old_id.clone(),
            kind: "work_context".to_string(),
            topic: "role_domain".to_string(),
            text: "old context".to_string(),
            source: "test".to_string(),
            confidence: 1.0,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        },
    )
    .unwrap();
    let occupied = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&old_path)
        .unwrap();

    let updated = update_work_context(
        &old_id,
        MemoryTextPatch {
            topic: Some("project_context".to_string()),
            text: Some("new context".to_string()),
            ttl_days: None,
        },
    )
    .unwrap()
    .unwrap();
    assert!(updated.cleanup_warning.is_some());
    let new_id = updated.value.id.clone();

    for _ in 0..2 {
        let read = load_work_context_with_cleanup().unwrap();
        assert!(read.cleanup_warning.is_some());
        assert_eq!(read.value.len(), 1);
        assert_eq!(read.value[0].id, new_id);
        assert_ne!(read.value[0].id, old_id);
    }
    assert!(old_path.exists());

    drop(occupied);
    let read = load_work_context_with_cleanup().unwrap();
    assert!(read.cleanup_warning.is_none());
    assert_eq!(read.value.len(), 1);
    assert_eq!(read.value[0].id, new_id);
    assert!(!old_path.exists());
    drop(home);
}

#[cfg(windows)]
#[test]
fn topic_migration_replacefile_failure_keeps_old_authority() {
    let root = recovery_test_root("topic-replacefile-failure");
    let old_path = root.join("old.json");
    let new_path = root.join("new.json");
    let journal = topic_migration_journal_path(&new_path).unwrap();
    let old = preference_fixture("old", "answer_style", "old value");
    let prior_new = preference_fixture("new", "workflow_preference", "prior value");
    let expected = preference_fixture("new", "workflow_preference", "expected value");
    write_json_atomic(&old_path, &old).unwrap();
    write_json_atomic(&new_path, &prior_new).unwrap();

    let error = commit_topic_migration_unlocked_with(
        &journal,
        &new_path,
        &expected,
        std::slice::from_ref(&old_path),
        |_| true,
        |path, value| {
            let text = serde_json::to_string_pretty(value).unwrap() + "\n";
            write_text_atomic_unlocked_with(path, &text, |tmp, target, backup| {
                crate::platform::filesystem::replace_file_atomically_with(
                    tmp,
                    target,
                    backup,
                    |_, _, _| Err(io::Error::from_raw_os_error(1175)),
                )
            })
        },
        |path| fs::remove_file(path),
    )
    .unwrap_err();

    assert!(error.to_string().contains("RolledBack"));
    assert!(old_path.exists());
    assert_eq!(
        serde_json::from_str::<PreferenceFile>(&fs::read_to_string(&new_path).unwrap())
            .unwrap()
            .text,
        "prior value"
    );
    reconcile_topic_migration_journals_unlocked::<PreferenceFile>(&root).unwrap();
    assert!(old_path.exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn topic_migration_promotion_failure_recovers_before_old_cleanup() {
    use std::os::windows::fs::OpenOptionsExt as _;

    let root = recovery_test_root("topic-promotion-failure");
    let old_path = root.join("old.json");
    let new_path = root.join("new.json");
    let journal = topic_migration_journal_path(&new_path).unwrap();
    let old = preference_fixture("old", "answer_style", "old value");
    let expected = preference_fixture("new", "workflow_preference", "expected value");
    write_json_atomic(&old_path, &old).unwrap();

    let error = commit_topic_migration_unlocked_with(
        &journal,
        &new_path,
        &expected,
        std::slice::from_ref(&old_path),
        |_| true,
        |path, value| {
            let text = serde_json::to_string_pretty(value).unwrap() + "\n";
            write_text_atomic_unlocked_with(path, &text, |tmp, target, backup| {
                let occupied = fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(tmp)
                    .unwrap();
                let result =
                    crate::platform::filesystem::replace_file_atomically(tmp, target, backup);
                drop(occupied);
                result
            })
        },
        |path| fs::remove_file(path),
    )
    .unwrap_err();

    assert!(error.to_string().contains("RecoveryRequired"));
    assert!(old_path.exists());
    assert!(!new_path.exists());
    recover_directory_json_files_unlocked::<PreferenceFile>(&root).unwrap();
    reconcile_topic_migration_journals_unlocked::<PreferenceFile>(&root).unwrap();
    assert!(!old_path.exists());
    assert_eq!(
        serde_json::from_str::<PreferenceFile>(&fs::read_to_string(&new_path).unwrap())
            .unwrap()
            .text,
        "expected value"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn non_profile_directory_source_recovers_missing_authority() {
    let root = recovery_test_root("directory");
    let target = root.join("pref_answer.json");
    fs::write(
        target.with_extension("bak"),
        serde_json::to_vec(&PreferenceFile {
            id: "pref_answer".to_string(),
            topic: "answer_style".to_string(),
            scope: "unconditional".to_string(),
            text: "concise".to_string(),
        })
        .unwrap(),
    )
    .unwrap();
    recover_directory_json_files::<PreferenceFile>(&root).unwrap();
    let restored: PreferenceFile =
        serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(restored.id, "pref_answer");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn recovery_waits_for_active_writer_lifecycle() {
    let root = recovery_test_root("concurrent");
    let target = root.join("recent.jsonl");
    let guard = file_lifecycle_lock().lock();
    let active = target.with_extension("tmp-9-9-9");
    fs::write(&active, "{\"active\":true}\n").unwrap();
    let target_for_reader = target.clone();
    let reader = std::thread::spawn(move || {
        read_text_recovering(&target_for_reader, |raw| {
            json_lines_are_valid::<serde_json::Value>(raw)
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::remove_file(active).unwrap();
    fs::write(&target, "{\"committed\":true}\n").unwrap();
    drop(guard);
    assert!(reader.join().unwrap().unwrap().contains("committed"));
    let _ = fs::remove_dir_all(root);
}

impl IsolatedPinvouHome {
    fn new(name: &str) -> Self {
        let guard = crate::platform::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "pinvou3-memory-{name}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        std::env::set_var("PINVOU3_HOME", &root);
        Self {
            root,
            prev,
            _guard: guard,
        }
    }
}

#[test]
fn memory_review_diagnostic_rotates_and_avoids_conversation_content() {
    let root =
        std::env::temp_dir().join(format!("pinvou3-memory-review-log-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let path = root.join("memory-review.log");
    fs::write(&path, vec![b'x'; MEMORY_REVIEW_LOG_MAX_BYTES as usize]).unwrap();

    append_memory_review_diagnostic_to(
        &path,
        "session/unsafe",
        "completed",
        json!({
            "result": "candidate_created",
            "pending_candidate_count": 1,
        }),
    )
    .unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content.lines().count(), 1);
    assert!(content.contains("session_unsafe"));
    assert!(content.contains("candidate_created"));
    assert!(!content.contains("current_user_message"));
    assert!(!content.contains("assistant_response"));
    assert!(fs::metadata(&path).unwrap().len() < MEMORY_REVIEW_LOG_MAX_BYTES);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_review_diagnostic_classifies_request_parse_and_apply_failures() {
    assert_eq!(
        memory_review_error_stage(&anyhow::anyhow!("post memory review chat/completions")),
        "request_failed"
    );
    assert_eq!(
        memory_review_error_stage(&anyhow::anyhow!("parse memory review json")),
        "parse_failed"
    );
    assert_eq!(
        memory_review_error_stage(&anyhow::anyhow!("auto write work context")),
        "apply_failed"
    );
}

impl Drop for IsolatedPinvouHome {
    fn drop(&mut self) {
        match &self.prev {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn missing_profile_recovers_latest_valid_interrupted_write() {
    let home = IsolatedPinvouHome::new("profile-interrupted-recovery");
    let path = profile_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path.with_extension("tmp-42-1"), "not json").unwrap();
    let valid_candidate = path.with_extension("tmp-42-2");
    fs::write(
        &valid_candidate,
        r#"{
  "version": 1,
  "revision": 7,
  "identity": { "call_name": "升级用户", "assistant_alias": "小品" }
}"#,
    )
    .unwrap();

    let recovered = load_profile().unwrap();
    assert_eq!(recovered.identity.call_name, "升级用户");
    assert_eq!(recovered.identity.assistant_alias, "小品");
    assert_eq!(recovered.revision, 7);
    assert!(path.is_file());
    assert!(!valid_candidate.exists());
    assert!(fs::read_to_string(&path).unwrap().contains("升级用户"));
    assert_eq!(load_profile().unwrap(), recovered);
    drop(home);
}

#[test]
fn authoritative_writes_commit_when_derived_runtime_source_is_unavailable() {
    let home = IsolatedPinvouHome::new("committed-write-derived-failure");
    let preference = enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "answer_style".to_string(),
        content: "Use concise answers".to_string(),
        source: "test".to_string(),
    })
    .unwrap();
    confirm_pending_memory(&preference.id).unwrap().unwrap();
    let preference_id = list_preferences().unwrap()[0].id.clone();

    fs::create_dir_all(recent_work_path().parent().unwrap()).unwrap();
    fs::write(recent_work_path(), "not-json\n").unwrap();

    let updated = update_preference(
        &preference_id,
        MemoryTextPatch {
            text: Some("Use detailed answers".to_string()),
            topic: None,
            ttl_days: None,
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(updated.value.text, "Use detailed answers");
    assert!(updated.cleanup_warning.is_none());
    assert_eq!(list_preferences().unwrap()[0].text, "Use detailed answers");
    assert!(render_memory_block().is_err());

    assert!(delete_preference(&preference_id).unwrap());
    assert!(list_preferences().unwrap().is_empty());
    assert!(render_memory_block().is_err());

    let profile_candidate = enqueue_memory_candidate(MemorySuggestion {
        kind: "profile".to_string(),
        topic: "call_name".to_string(),
        content: "Ada".to_string(),
        source: "test".to_string(),
    })
    .unwrap();
    confirm_pending_memory(&profile_candidate.id)
        .unwrap()
        .unwrap();
    assert_eq!(load_profile().unwrap().identity.call_name, "Ada");
    assert!(render_memory_block().is_err());
    drop(home);
}

#[test]
fn render_empty_profile_is_empty() {
    let profile = MemoryProfile {
        version: PROFILE_VERSION,
        ..MemoryProfile::default()
    };
    let (block, items) = render_from_parts(&profile, &[], &[], &[], &[], &[], Utc::now());
    assert!(block.is_empty());
    assert!(items.is_empty());
}

#[test]
fn render_profile_block_uses_low_sensitive_fields() {
    let profile = MemoryProfile {
        version: PROFILE_VERSION,
        identity: ProfileIdentity {
            call_name: "王科长".to_string(),
            assistant_alias: "小文".to_string(),
        },
        conventions: ProfileConventions {
            language: "简体中文".to_string(),
            doc_standard: "GB/T 9704".to_string(),
            number_usage: "GB/T 15835".to_string(),
            style_notes: vec!["正文三号仿宋_GB2312".to_string()],
        },
        ..MemoryProfile::default()
    };
    let (block, items) = render_from_parts(&profile, &[], &[], &[], &[], &[], Utc::now());
    assert!(block.contains("<pinvou_user_memory>"));
    assert!(block.contains("称呼：王科长"));
    assert!(block.contains("助手昵称：小文"));
    assert!(block.contains("GB/T 9704"));
    assert_eq!(items.len(), 3);
}

#[test]
fn writes_memory_snapshot_document_for_debugging() {
    let _home = IsolatedPinvouHome::new("snapshot-doc");
    let profile = MemoryProfile {
        version: PROFILE_VERSION,
        identity: ProfileIdentity {
            call_name: "欣哥".to_string(),
            assistant_alias: "小猪".to_string(),
        },
        ..MemoryProfile::default()
    };
    let preferences = vec![PreferenceFile {
        id: "pref_answer_style".to_string(),
        topic: "answer_style".to_string(),
        scope: "unconditional".to_string(),
        text: "回答先给结论，再给步骤".to_string(),
    }];
    let path =
        write_memory_snapshot_document(&profile, &preferences, &[], &[], &[], &[], &[], &[], None)
            .unwrap();

    assert_eq!(path, snapshot_path());
    let doc = fs::read_to_string(path).unwrap();
    assert!(doc.contains("# PINVOU 设备记忆快照"));
    assert!(doc.contains("用户称呼"));
    assert!(doc.contains("回答先给结论"));
    assert!(doc.contains("pinvou-memory-snapshot/v1"));
    assert!(doc.contains("当前没有绑定 session"));
}

#[test]
fn auto_review_discovers_preference_and_recent_work_candidates() {
    let suggestions =
        discover_turn_suggestions("以后回答默认先给结论，再给步骤。这周在做营商环境推进会材料。");
    assert!(suggestions
        .iter()
        .any(|item| item.kind == "preference" && item.content.contains("先给结论")));
    assert!(suggestions
        .iter()
        .any(|item| item.kind == "recent_work" && item.content.contains("营商环境")));
}

#[test]
fn auto_review_skips_one_off_tasks_and_sensitive_text() {
    assert!(discover_turn_suggestions("帮我写一个周报").is_empty());
    assert!(discover_turn_suggestions("我的手机号是 13800138000，以后默认用这个").is_empty());
}

#[test]
fn safety_filters_allow_format_symbols_but_block_real_secrets() {
    assert!(!looks_sensitive("对比时默认使用 A/B 两列"));
    assert!(!looks_sensitive("示例里可以使用 name=value 格式"));
    assert!(!looks_sensitive("文档偏好使用 Markdown/表格"));
    assert!(looks_sensitive("我的邮箱是 user@example.com"));
    assert!(looks_sensitive("文件在 /home/hexin/report.md"));
    assert!(looks_sensitive("api_key=abcdef"));
    assert!(looks_sensitive("我的手机号是 13800138000"));
}

#[test]
fn task_filter_allows_preference_phrasing() {
    assert!(!looks_task_like("回答时先总结重点"));
    assert!(!looks_task_like("生成报告时先给大纲"));
    assert!(looks_task_like("帮我写一个周报"));
    assert!(looks_task_like("写一个周报"));
}

#[test]
fn review_signal_detects_work_background_and_current_focus() {
    assert!(has_memory_review_signal(
        "我长期负责公司内部制度、流程和办公文档建设"
    ));
    assert!(has_memory_review_signal(
        "我长期参与本地 AI 办公助手相关产品设计，经常评审功能方案"
    ));
    assert!(has_memory_review_signal(
        "这周我主要在做欧洲旅游规划，后面还要继续调整城市顺序"
    ));
}

#[test]
fn llm_review_sanitizer_rejects_question_labels() {
    let item = LlmMemoryItem {
        action: "auto_write".to_string(),
        kind: "profile".to_string(),
        topic: "call_name".to_string(),
        content: "谁".to_string(),
        confidence: 0.99,
        ttl_days: None,
        reason: String::new(),
    };
    assert!(sanitize_llm_memory_item(item).is_none());
}

#[test]
fn llm_review_sanitizer_cleans_explicit_profile_labels() {
    let item = LlmMemoryItem {
        action: "auto_write".to_string(),
        kind: "profile".to_string(),
        topic: "call_name".to_string(),
        content: "称呼：欣哥".to_string(),
        confidence: 0.99,
        ttl_days: None,
        reason: String::new(),
    };
    let decision = sanitize_llm_memory_item(item).unwrap();
    let suggestion = decision.suggestion;
    assert_eq!(decision.action, "auto_write");
    assert_eq!(suggestion.kind, "profile");
    assert_eq!(suggestion.topic, "call_name");
    assert_eq!(suggestion.content, "欣哥");
}

#[test]
fn llm_review_parser_accepts_json_object() {
    let parsed = parse_llm_memory_review(
            r#"{"items":[{"action":"pending_confirm","kind":"preference","topic":"output_style","content":"回答默认先给结论","confidence":0.88}]}"#,
        )
        .unwrap();
    assert_eq!(parsed.items.len(), 1);
    let decision = sanitize_llm_memory_item(parsed.items[0].clone()).unwrap();
    let suggestion = decision.suggestion;
    assert_eq!(decision.action, "pending_confirm");
    assert_eq!(suggestion.kind, "preference");
    assert_eq!(suggestion.topic, "answer_style");
    assert_eq!(suggestion.content, "回答默认先给结论");
}

#[test]
fn llm_review_sanitizer_does_not_override_recent_kind_by_status_words() {
    let item = LlmMemoryItem {
        action: "auto_write".to_string(),
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        content: "已生成初稿，正在继续完善人力资源手册".to_string(),
        confidence: 0.9,
        ttl_days: None,
        reason: String::new(),
    };
    let decision = sanitize_llm_memory_item(item).unwrap();
    assert_eq!(decision.suggestion.kind, "current_focus");
    assert_eq!(decision.suggestion.topic, "current_work");
}

#[test]
fn llm_review_prompt_matches_supported_actions() {
    assert!(LLM_REVIEW_PROMPT
        .contains("\"action\": \"skip | pending_confirm | auto_write | auto_update\""));
    assert!(!LLM_REVIEW_PROMPT.contains("archive"));
    assert!(!LLM_REVIEW_PROMPT.contains("must_create_recent_activity"));
}

#[test]
fn scenario_review_writes_long_and_recent_memories() {
    let _home = IsolatedPinvouHome::new("long-recent");
    let review = LlmMemoryReview {
        items: vec![
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "profile".to_string(),
                topic: "call_name".to_string(),
                content: "用户希望被称呼为欣哥".to_string(),
                confidence: 0.98,
                ttl_days: None,
                reason: "明确称呼".to_string(),
            },
            LlmMemoryItem {
                action: "pending_confirm".to_string(),
                kind: "preference".to_string(),
                topic: "answer_style".to_string(),
                content: "回答默认先给结论，再给关键步骤".to_string(),
                confidence: 0.88,
                ttl_days: None,
                reason: "长期回答偏好需确认".to_string(),
            },
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "work_context".to_string(),
                topic: "role_domain".to_string(),
                content: "用户长期负责公司内部制度、流程和办公文档建设".to_string(),
                confidence: 0.96,
                ttl_days: None,
                reason: "稳定工作背景".to_string(),
            },
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "current_focus".to_string(),
                topic: "current_work".to_string(),
                content: "正在推进公司人力资源手册更新，后续可能继续细化结构和页面".to_string(),
                confidence: 0.91,
                ttl_days: Some(21),
                reason: "短期持续事项".to_string(),
            },
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "recent_activity".to_string(),
                topic: "completed_work".to_string(),
                content: "已完成公司人力资源手册 PPT 初稿，包含制度说明和章节结构".to_string(),
                confidence: 0.9,
                ttl_days: Some(14),
                reason: "近期交付结果".to_string(),
            },
        ],
    };

    let outcome = apply_llm_memory_review(review).unwrap();
    assert_eq!(outcome.pending.len(), 1);
    assert!(outcome
        .events
        .iter()
        .any(|event| event.kind == "profile" && event.text.contains("欣哥")));
    assert!(outcome
        .events
        .iter()
        .any(|event| event.kind == "work_context"));
    assert!(outcome
        .events
        .iter()
        .any(|event| event.kind == "current_focus"));
    assert!(outcome
        .events
        .iter()
        .any(|event| event.kind == "recent_activity"));

    let profile = load_profile().unwrap();
    assert_eq!(profile.identity.call_name, "欣哥");
    assert!(load_preferences().unwrap().is_empty());
    let pending = load_pending_memory().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, "preference");

    let work_context = load_work_context().unwrap();
    assert_eq!(work_context.len(), 1);
    assert_eq!(work_context[0].topic, "role_domain");
    assert!(work_context[0].text.contains("内部制度"));

    let current_focus = load_current_focus().unwrap();
    assert_eq!(current_focus.len(), 1);
    assert_eq!(current_focus[0].kind, "current_focus");
    assert_eq!(current_focus[0].topic, "current_work");
    assert_eq!(current_focus[0].ttl_days, 21);
    assert!(current_focus[0].text.contains("人力资源手册更新"));

    let recent_activity = load_recent_activity().unwrap();
    assert_eq!(recent_activity.len(), 1);
    assert_eq!(recent_activity[0].kind, "recent_activity");
    assert_eq!(recent_activity[0].topic, "completed_work");
    assert_eq!(recent_activity[0].ttl_days, 14);
    assert!(recent_activity[0].text.contains("PPT 初稿"));

    let (block_before_confirm, _) = render_memory_block().unwrap();
    assert!(block_before_confirm.contains("称呼：欣哥"));
    assert!(block_before_confirm.contains("工作背景："));
    assert!(block_before_confirm.contains("当前关注（会过期）："));
    assert!(block_before_confirm.contains("近期动态（会过期）："));
    assert!(!block_before_confirm.contains("回答默认先给结论"));

    confirm_pending_memory(&pending[0].id).unwrap().unwrap();
    let preferences = load_preferences().unwrap();
    assert_eq!(preferences.len(), 1);
    assert_eq!(preferences[0].topic, "answer_style");
    assert!(preferences[0].text.contains("先给结论"));

    let (block_after_confirm, _) = render_memory_block().unwrap();
    assert!(block_after_confirm.contains("长期偏好："));
    assert!(block_after_confirm.contains("回答默认先给结论"));
}

#[test]
fn scenario_review_filters_low_quality_memory() {
    let _home = IsolatedPinvouHome::new("filters");
    let review = LlmMemoryReview {
        items: vec![
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "preference".to_string(),
                topic: "answer_style".to_string(),
                content: "帮我写一个周报".to_string(),
                confidence: 0.95,
                ttl_days: None,
                reason: "一次性任务不应记忆".to_string(),
            },
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "current_focus".to_string(),
                topic: "current_work".to_string(),
                content: "api_key=abcdef".to_string(),
                confidence: 0.95,
                ttl_days: None,
                reason: "敏感信息不应记忆".to_string(),
            },
            LlmMemoryItem {
                action: "auto_write".to_string(),
                kind: "recent_activity".to_string(),
                topic: "completed_work".to_string(),
                content: "已完成欧洲旅游规划初稿".to_string(),
                confidence: 0.5,
                ttl_days: Some(14),
                reason: "低置信度不应写入".to_string(),
            },
        ],
    };

    let outcome = apply_llm_memory_review(review).unwrap();
    assert!(outcome.events.is_empty());
    assert!(outcome.pending.is_empty());
    assert!(load_profile().unwrap().identity.call_name.is_empty());
    assert!(load_preferences().unwrap().is_empty());
    assert!(load_work_context().unwrap().is_empty());
    assert!(load_current_focus().unwrap().is_empty());
    assert!(load_recent_activity().unwrap().is_empty());
    assert!(render_memory_block().unwrap().0.is_empty());
}

#[test]
fn confirmed_pending_requires_real_structured_memory() {
    let _home = IsolatedPinvouHome::new("confirmed-materialized");
    let suggestion = MemorySuggestion {
        kind: "work_context".to_string(),
        topic: "task_pattern".to_string(),
        content: "用户长期负责公司内部制度、流程和办公文档建设".to_string(),
        source: "llm_review".to_string(),
    };

    let pending = enqueue_memory_candidate(suggestion.clone()).unwrap();
    assert_eq!(pending.status, PENDING_STATUS_PENDING);
    confirm_pending_memory(&pending.id).unwrap().unwrap();
    assert_eq!(load_work_context().unwrap().len(), 1);

    let covered = enqueue_memory_candidate(suggestion.clone()).unwrap();
    assert_eq!(covered.status, PENDING_STATUS_CONFIRMED);

    fs::remove_dir_all(work_context_dir()).unwrap();
    let reopened = enqueue_memory_candidate(suggestion).unwrap();
    assert_eq!(reopened.status, PENDING_STATUS_PENDING);
    confirm_pending_memory(&reopened.id).unwrap().unwrap();
    assert_eq!(load_work_context().unwrap().len(), 1);
}

#[test]
fn scenario_current_focus_merges_related_updates() {
    let _home = IsolatedPinvouHome::new("focus-merge");
    let first = MemorySuggestion {
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        content: "推进公司人力资源手册更新，重点调整章节结构，计划新增数据合规、灵活用工等章节。"
            .to_string(),
        source: "test".to_string(),
    };
    let second = MemorySuggestion {
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        content: "推进公司人力资源手册更新，后续计划细化章节结构和页面设计。".to_string(),
        source: "test".to_string(),
    };

    let first = upsert_timed_memory_unlocked(
        &first.kind,
        &first.topic,
        &first.content,
        &first.source,
        Some(21),
        0.9,
    )
    .unwrap();
    let second = upsert_timed_memory_unlocked(
        &second.kind,
        &second.topic,
        &second.content,
        &second.source,
        Some(21),
        0.9,
    )
    .unwrap();

    let focus = load_current_focus().unwrap();
    assert_eq!(focus.len(), 1);
    assert_eq!(first.id, second.id);
    assert!(focus[0].text.contains("页面设计"));
    assert!(!focus[0].text.contains("数据合规"));
}

#[test]
fn scenario_existing_current_focus_duplicates_are_deduped_on_load() {
    let _home = IsolatedPinvouHome::new("focus-load-dedupe");
    let now = Utc::now();
    let old = TimedMemoryItem {
        id: "focus_old".to_string(),
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        text: "推进公司人力资源手册更新，重点调整章节结构，计划新增数据合规、灵活用工等章节。"
            .to_string(),
        source: "test".to_string(),
        confidence: 0.9,
        created_at: (now - Duration::days(1)).to_rfc3339(),
        updated_at: (now - Duration::days(1)).to_rfc3339(),
        last_hit: (now - Duration::days(1)).to_rfc3339(),
        ttl_days: 21,
        status: "active".to_string(),
    };
    let new = TimedMemoryItem {
        id: "focus_new".to_string(),
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        text: "推进公司人力资源手册更新，后续计划细化章节结构和页面设计。".to_string(),
        source: "test".to_string(),
        confidence: 0.9,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_hit: now.to_rfc3339(),
        ttl_days: 21,
        status: "active".to_string(),
    };
    let path = current_focus_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&old).unwrap(),
            serde_json::to_string(&new).unwrap()
        ),
    )
    .unwrap();

    let focus = load_current_focus().unwrap();
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0].id, "focus_new");
    assert!(focus[0].text.contains("页面设计"));
}

#[test]
fn memory_jsonl_writes_are_bounded() {
    let _home = IsolatedPinvouHome::new("jsonl-bounds");
    let now = Utc::now();

    let timed: Vec<TimedMemoryItem> = (0..55)
        .map(|i| {
            let active = i < 10;
            let ts = (now - Duration::minutes(i)).to_rfc3339();
            let marker = char::from_u32(0x4e00 + i as u32).unwrap_or('记');
            TimedMemoryItem {
                id: format!("focus_{i}"),
                kind: "current_focus".to_string(),
                topic: "current_work".to_string(),
                text: marker.to_string().repeat(8),
                source: "test".to_string(),
                confidence: 0.9,
                created_at: ts.clone(),
                updated_at: ts.clone(),
                last_hit: ts,
                ttl_days: 21,
                status: if active { "active" } else { "archived" }.to_string(),
            }
        })
        .collect();
    write_timed_memory_file(&current_focus_path(), &timed, "current_focus").unwrap();
    let focus = load_current_focus().unwrap();
    assert_eq!(
        focus.iter().filter(|item| item.status == "active").count(),
        CURRENT_FOCUS_ACTIVE_MAX_STORED
    );
    assert!(focus.len() <= CURRENT_FOCUS_ACTIVE_MAX_STORED + TIMED_MEMORY_ARCHIVED_MAX_STORED);

    let recent: Vec<RecentWorkItem> = (0..40)
        .map(|i| {
            let active = i < 10;
            let ts = (now - Duration::minutes(i)).to_rfc3339();
            RecentWorkItem {
                id: format!("recent_{i}"),
                title: format!("近期工作 {i}"),
                summary: "边界测试".to_string(),
                status: if active { "active" } else { "archived" }.to_string(),
                source: "test".to_string(),
                created_at: ts.clone(),
                updated_at: ts.clone(),
                last_hit: ts,
                expires_at: (now + Duration::days(7)).to_rfc3339(),
            }
        })
        .collect();
    write_recent_work_unlocked(&recent).unwrap();
    let recent = load_recent_work().unwrap();
    assert_eq!(
        recent.iter().filter(|item| item.status == "active").count(),
        RECENT_WORK_ACTIVE_MAX_STORED
    );
    assert!(recent.len() <= RECENT_WORK_ACTIVE_MAX_STORED + RECENT_WORK_ARCHIVED_MAX_STORED);

    let pending: Vec<PendingMemoryItem> = (0..120)
        .map(|i| {
            let pending = i < 25;
            let ts = (now - Duration::minutes(i)).to_rfc3339();
            PendingMemoryItem {
                id: format!("pending_{i}"),
                kind: "preference".to_string(),
                topic: "answer_style".to_string(),
                content: format!("回答风格候选 {i}"),
                source: "test".to_string(),
                status: if pending {
                    PENDING_STATUS_PENDING
                } else {
                    PENDING_STATUS_IGNORED
                }
                .to_string(),
                seen_count: 1,
                created_at: ts.clone(),
                updated_at: ts,
            }
        })
        .collect();
    write_pending_memory_unlocked(&pending).unwrap();
    let pending = load_pending_memory().unwrap();
    assert_eq!(
        pending
            .iter()
            .filter(|item| item.status == PENDING_STATUS_PENDING)
            .count(),
        PENDING_MEMORY_ACTIVE_MAX_STORED
    );
    assert!(pending.len() <= PENDING_MEMORY_ACTIVE_MAX_STORED + PENDING_MEMORY_RESOLVED_MAX_STORED);

    let never: Vec<NeverMemoryItem> = (0..205)
        .map(|i| NeverMemoryItem {
            id: format!("never_{i}"),
            pattern: format!("不再提示内容 {i}"),
            reason: "test".to_string(),
            created_at: (now - Duration::minutes(i)).to_rfc3339(),
        })
        .collect();
    write_never_memory_unlocked(&never).unwrap();
    let never = load_never_memory().unwrap();
    assert_eq!(never.len(), NEVER_MEMORY_MAX_STORED);
    assert!(never.iter().any(|item| item.pattern == "不再提示内容 0"));
    assert!(!never.iter().any(|item| item.pattern == "不再提示内容 204"));
}

#[test]
fn recent_work_suggestion_maps_to_current_focus_kind() {
    let item = pending_item_from_suggestion(MemorySuggestion {
        kind: "recent_work".to_string(),
        topic: "current_work".to_string(),
        content: "这周在做营商环境推进会材料".to_string(),
        source: "test".to_string(),
    })
    .unwrap();
    assert_eq!(item.kind, "current_focus");
    assert_eq!(item.topic, "current_work");
}

#[test]
fn llm_recent_work_accepts_delivery_completion_status() {
    let item = LlmMemoryItem {
        action: "pending_confirm".to_string(),
        kind: "recent_work".to_string(),
        topic: "current_work".to_string(),
        content: "已生成营商环境推进会报告".to_string(),
        confidence: 0.86,
        ttl_days: None,
        reason: String::new(),
    };
    let decision = sanitize_llm_memory_item(item).unwrap();
    let suggestion = decision.suggestion;
    assert_eq!(suggestion.kind, "recent_activity");
    assert_eq!(suggestion.topic, "completed_work");
    assert_eq!(suggestion.content, "已生成营商环境推进会报告");
}

#[test]
fn delivery_tool_summary_keeps_artifact_path_after_long_content() {
    assert!(is_delivery_tool(
        "File",
        &json!({"action": "patch", "path": "italy_travel_guide.md"})
    ));
    let summary = summarize_tool_start(
        "write_file",
        &json!({
            "content": "正文".repeat(2000),
            "path": "italy_travel_guide.md"
        }),
    );
    assert!(summary.contains("name=write_file"));
    assert!(summary.contains("path=italy_travel_guide.md"));
    assert!(!summary.contains("正文正文正文"));

    let presented = summarize_tool_start(
        "mcp_pinvou3_present_artifact",
        &json!({
            "path": "/home/hexin/.pinvou3/sessions/tvqydl2b6sjd0/workspace/italy_travel_guide.md",
            "title": "意大利12天深度慢游攻略",
            "description": "罗马、佛罗伦萨、威尼斯行程规划"
        }),
    );
    assert!(presented.contains("italy_travel_guide.md"));
    assert!(presented.contains("意大利12天深度慢游攻略"));
}

#[test]
fn assistant_delivery_completion_can_trigger_review() {
    assert!(assistant_suggests_delivery_complete(
        "帮我整理一份营商环境推进会材料",
        "已完成营商环境推进会材料整理，核心内容包括会议背景、推进事项和下一步安排。"
    ));
    assert!(assistant_suggests_delivery_complete(
        "修复记忆候选重复弹出的问题",
        "已经修复了重复弹出的问题，并补了去重测试。"
    ));
    assert!(!assistant_suggests_delivery_complete(
        "帮我整理一份营商环境推进会材料",
        "暂时无法完成材料整理，需要你先提供会议背景。"
    ));
}

#[test]
fn render_recent_work_skips_archived_and_expired() {
    let now = Utc::now();
    let active = RecentWorkItem {
        id: "active".to_string(),
        title: "筹备营商环境推进会材料".to_string(),
        summary: "本周完善会议方案".to_string(),
        status: "active".to_string(),
        source: "test".to_string(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_hit: now.to_rfc3339(),
        expires_at: (now + Duration::days(3)).to_rfc3339(),
    };
    let archived = RecentWorkItem {
        status: "archived".to_string(),
        title: "旧材料".to_string(),
        id: "archived".to_string(),
        ..active.clone()
    };
    let expired = RecentWorkItem {
        title: "过期材料".to_string(),
        id: "expired".to_string(),
        expires_at: (now - Duration::days(1)).to_rfc3339(),
        ..active.clone()
    };
    let profile = MemoryProfile {
        version: PROFILE_VERSION,
        ..MemoryProfile::default()
    };
    let (block, items) = render_from_parts(
        &profile,
        &[],
        &[],
        &[],
        &[],
        &[active, archived, expired],
        now,
    );
    assert!(block.contains("筹备营商环境推进会材料"));
    assert!(!block.contains("旧材料"));
    assert!(!block.contains("过期材料"));
    assert_eq!(items.len(), 1);
}

// Wave 3 把 memory 的推理方言判定收敛到共享 core::reasoning_dialect 后，注入
// 行为（body 字段名与取值、URL 嗅探优先于 model 名回退的次序）只有共享纯函数
// 的 URL 分类测试覆盖，memory 路径本身无行为级测试。以下三个测试锁定该契约。

#[test]
fn memory_review_reasoning_controls_inject_for_newly_covered_vendors() {
    // OpenaiCompatible + 各厂商直连 URL：Wave 3 新增覆盖（原实现返回 None 不注参）。
    let cases = [
        (
            "https://ark.cn-volces.com/api/v3",
            "doubao-seed-1-6",
            "thinking",
        ),
        ("https://api.minimax.chat/v1", "abab6.5s-chat", "thinking"),
        (
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-4.5",
            "thinking",
        ),
        ("https://api.xiaomimimo.com/v1", "mimo-7b", "thinking"),
        ("https://api.moonshot.cn/v1", "kimi-k2.6-0908", "thinking"),
    ];
    for (base_url, model, field) in cases {
        let mut body = json!({});
        apply_memory_review_reasoning_controls(
            &mut body,
            ModelPreset::OpenaiCompatible,
            "openai_compatible",
            base_url,
            model,
        );
        assert_eq!(
            body[field],
            json!({ "type": "disabled" }),
            "{model} @ {base_url} 必须注入 thinking disable"
        );
    }
    // Minimax 簇额外带 reasoning_split（与 review 侧同构）。
    let mut minimax = json!({});
    apply_memory_review_reasoning_controls(
        &mut minimax,
        ModelPreset::OpenaiCompatible,
        "openai_compatible",
        "https://api.minimax.chat/v1",
        "abab6.5s-chat",
    );
    assert_eq!(minimax["reasoning_split"], json!(true));
}

#[test]
fn memory_review_reasoning_controls_prefer_url_over_model_fallback() {
    // URL 能识别厂商时按 URL 注参，model 名回退不再参与：
    // deepseek URL + qwen 模型名（main 上的旧 memory 会错注 enable_thinking）。
    let mut body = json!({});
    apply_memory_review_reasoning_controls(
        &mut body,
        ModelPreset::OpenaiCompatible,
        "openai_compatible",
        "https://api.deepseek.com/v1",
        "qwen3-235b",
    );
    assert_eq!(body["thinking"], json!({ "type": "disabled" }));
    assert!(body.get("enable_thinking").is_none());

    // URL 识别不到厂商时回退 model 名：自定义代理网关 + qwen/deepseek 模型名。
    let mut qwen_fallback = json!({});
    apply_memory_review_reasoning_controls(
        &mut qwen_fallback,
        ModelPreset::OpenaiCompatible,
        "openai_compatible",
        "https://internal-gateway.corp/v1",
        "Qwen3-32B",
    );
    assert_eq!(qwen_fallback["enable_thinking"], json!(false));

    let mut deepseek_fallback = json!({});
    apply_memory_review_reasoning_controls(
        &mut deepseek_fallback,
        ModelPreset::OpenaiCompatible,
        "openai_compatible",
        "https://internal-gateway.corp/v1",
        "deepseek-v4-pro",
    );
    assert_eq!(deepseek_fallback["thinking"], json!({ "type": "disabled" }));
}

#[test]
fn memory_review_reasoning_controls_keep_kimi_gate_on_url_path() {
    // Kimi 门控统一后：URL 命中 moonshot 簇时，k2.5/k2.6 注参、k2.7 与
    // thinking 变体不注参（k2.7 是 always-thinking 模型，停注是修复而非回归）。
    let mut k26 = json!({});
    apply_memory_review_reasoning_controls(
        &mut k26,
        ModelPreset::OpenaiCompatible,
        "openai_compatible",
        "https://api.moonshot.cn/v1",
        "kimi-k2.6-0908",
    );
    assert_eq!(k26["thinking"], json!({ "type": "disabled" }));

    for model in ["kimi-k2.7", "kimi-k2.7-code", "kimi-k2.6-thinking"] {
        let mut body = json!({});
        apply_memory_review_reasoning_controls(
            &mut body,
            ModelPreset::OpenaiCompatible,
            "openai_compatible",
            "https://api.moonshot.cn/v1",
            model,
        );
        assert!(
            body.get("thinking").is_none(),
            "{model} 不应注入 thinking disable"
        );
    }
}
