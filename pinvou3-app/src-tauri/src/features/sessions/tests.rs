//! Session store integration tests.
//!
//! Migrated verbatim from the historical inline `mod tests` of the god-module
//! `sessions/mod.rs`. These tests exercise the full store across every
//! submodule, so they live next to the facade and pull in the re-exported
//! public surface plus the few crate-visible helpers they need directly.

use super::*;
use crate::platform::paths;
use crate::platform::paths::tests::ENV_LOCK;
use crate::platform::prefs::UserPrefs;
use anyhow::Result;
use chrono::Utc;
use deepseek_tui::models::{ContentBlock, ImageUrlContent, Message, SystemPrompt};
use deepseek_tui::session_manager::create_saved_session_with_id_and_mode;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

// Crate-visible helpers exercised directly by the suite (not re-exported by
// the facade because they are internal collaboration seams).
use super::scheduled::ScheduledProfileRegistry;
use super::store::MAX_SESSIONS_PER_KIND;
use super::validators::generate_session_id;

/// 借用 paths 模块的进程级 env 锁——避免与其他 mutate PINVOU3_HOME
/// 的测试并行 race。返回带 guard 的 store；guard drop 后才解锁。
fn isolated_store() -> (SessionStore, std::sync::MutexGuard<'static, ()>) {
    let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let tmp = std::env::temp_dir().join(format!(
        "pinvou3-sessions-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
    unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };
    let store = SessionStore::boot_with_scheduled_root(tmp.join("scheduled")).expect("boot");
    // 注意：不 remove_var——锁还没 drop，下面的断言需要 PINVOU3_HOME 仍是这个值。
    (store, guard)
}

fn record_session_deletions(store: &SessionStore) -> Arc<std::sync::Mutex<Vec<String>>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    store.register_session_deleted_hook(Arc::new(move |session_id| {
        recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(session_id.to_string());
    }));
    seen
}

fn user_text(text: &str) -> Message {
    Message {
        role: "user".into(),
        content: vec![ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    }
}

fn assistant_text(text: &str) -> Message {
    Message {
        role: "assistant".into(),
        content: vec![ContentBlock::Text {
            text: text.into(),
            cache_control: None,
        }],
    }
}

fn assistant_tool_use(id: &str) -> Message {
    Message {
        role: "assistant".into(),
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: "Bash".into(),
            input: serde_json::json!({"command": "printf still-running"}),
            caller: None,
        }],
    }
}

/// Reopen the same on-disk stores without consulting the process-global
/// PINVOU3_HOME again, so restart assertions retain the paths captured at boot.
fn reopen_store(store: &SessionStore) -> Result<SessionStore> {
    let reopened = SessionStore::from_paths(
        store.manager.sessions_dir().to_path_buf(),
        store.scheduled_profiles_path.as_ref().clone(),
        store.scheduled_root.as_ref().clone(),
    )?;
    reopened.load_session_models();
    reopened.load_pinned_sessions();
    reopened.load_hidden_sessions();
    reopened.load_session_mode_states();
    reopened.load_session_workspaces();
    {
        let _mutation = reopened.scheduled_mutation.lock();
        reopened.enforce_session_retention_locked()?;
    }
    reopened.purge_all_scheduled_side_maps();
    Ok(reopened)
}

fn task_workspace(store: &SessionStore, task_id: &str) -> PathBuf {
    store
        .scheduled_workspace_for_task(task_id)
        .expect("valid scheduled task workspace")
}

#[test]
fn list_cache_shares_snapshot_and_invalidates_on_write() {
    let (store, _g) = isolated_store();
    let s1 = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    // 两次读取共享同一 Arc 快照(不重复全目录扫描)
    let a = store.list_sessions_cached().expect("first cached read");
    let b = store.list_sessions_cached().expect("second cached read");
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "cached reads must share one snapshot"
    );
    assert!(a.iter().any(|m| m.id == s1.metadata.id));

    // 写路径(set_title 走 save_session_atomic)使快照失效,新标题可见
    store
        .set_title(&s1.metadata.id, "renamed".into())
        .expect("set title");
    let c = store.list_sessions_cached().expect("read after write");
    assert!(
        !std::sync::Arc::ptr_eq(&a, &c),
        "write must invalidate the snapshot"
    );
    assert!(
        c.iter()
            .any(|m| m.id == s1.metadata.id && m.title == "renamed")
    );

    // 删除路径同样失效
    store.delete(&s1.metadata.id).expect("delete");
    let d = store.list_sessions_cached().expect("read after delete");
    assert!(!d.iter().any(|m| m.id == s1.metadata.id));
}

#[test]
fn list_cache_stale_generation_snapshot_is_never_served() {
    // 竞态回归(list_sessions_cached 的回填守卫):线程 A miss 后扫描目录,
    // 扫描期间线程 B 写盘失效;A 的回填必须被代数比对拒绝,否则陈旧快照
    // 会覆盖 B 触发的重扫并驻留到下一次写。交错无法在单线程测试里真实
    // 还原,改为锁定守卫的可观察契约:过期代数的快照(模拟守卫失效时被
    // 落地的写前扫描产物)对读取路径不可达——命中检查只信「当前代数+
    // 条目代数一致」的元组。
    let (store, _g) = isolated_store();
    let s1 = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");

    // 写后读:失效→reconcile 重扫回填同链路,必须看到新标题。
    store
        .set_title(&s1.metadata.id, "renamed".into())
        .expect("set title");
    let post_write = store.list_sessions_cached().expect("post-write read");
    assert!(
        post_write
            .iter()
            .any(|m| m.id == s1.metadata.id && m.title == "renamed")
    );

    // 模拟守卫失效的落地物:旧标题视图挂在过期代数上,读取不得返回它。
    let generation_now = store
        .list_cache_generation
        .load(std::sync::atomic::Ordering::Acquire);
    let mut poisoned = Vec::clone(&post_write);
    for m in &mut poisoned {
        if m.id == s1.metadata.id {
            m.title = "OLD-STALE".into();
        }
    }
    *store.list_cache.write() = Some((
        generation_now.wrapping_sub(1),
        std::sync::Arc::new(poisoned),
    ));
    let after = store
        .list_sessions_cached()
        .expect("read after poisoned injection");
    let title = after
        .iter()
        .find(|m| m.id == s1.metadata.id)
        .map(|m| m.title.clone());
    assert_ne!(
        title.as_deref(),
        Some("OLD-STALE"),
        "a stale-generation snapshot must never be served"
    );
}

#[test]
fn list_cache_invalidated_when_delete_partially_fails() {
    // 部分失败回归:上游 delete_session 先 remove_file(JSON) 再 remove_dir_all
    // (会话目录)。把 sessions/<id>/ 路径放一个普通文件,让 remove_dir_all 确定性
    // 报 ENOTDIR——JSON 已从盘上消失但 delete 返回 Err。此时列表快照必须已经
    // 失效:若只在 Ok 分支失效,幽灵条目会驻留缓存直到下一次任意写。
    let (store, _g) = isolated_store();
    let s1 = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    // 预热缓存:此刻快照包含该会话。
    let before = store.list_sessions_cached().expect("warm cache");
    assert!(before.iter().any(|m| m.id == s1.metadata.id));

    // 构造部分失败:sessions/<id> 处放普通文件,remove_dir_all 报 ENOTDIR。
    let session_dir = store.manager.sessions_dir().join(&s1.metadata.id);
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    let blocker = session_dir.with_extension("json.blocker");
    std::fs::write(&blocker, b"not a dir").expect("write blocker");
    // 把整个 sessions/<id> 目录替换为同名普通文件:remove_dir_all 必失败。
    std::fs::remove_dir_all(&session_dir).expect("clear dir");
    std::fs::write(&session_dir, b"plain file at dir path").expect("block dir path");

    let result = store.delete(&s1.metadata.id);
    let err = result.expect_err("delete must surface the ENOTDIR error");
    assert!(
        err.to_string().contains(&s1.metadata.id) || err.to_string().contains("delete_session"),
        "unexpected error shape: {err:#}"
    );
    // 会话 JSON 已被上游删除:盘面与缓存必须一致——幽灵不得驻留。
    let after = store
        .list_sessions_cached()
        .expect("read after partial failure");
    assert!(
        !after.iter().any(|m| m.id == s1.metadata.id),
        "phantom entry must not survive a partially-failed delete"
    );
    // 复原环境:blocker 文件不碍事,但普通文件占用的 <id> 路径留着会让后续
    // 测试的目录假设失效,显式清掉。
    let _ = std::fs::remove_file(&session_dir);
    let _ = std::fs::remove_file(&blocker);
}

#[test]
fn session_roots_plain_session_shares_private_root() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let private = paths::session_workspace_dir(&s.metadata.id);
    let roots = store.session_roots(&s.metadata.id).expect("roots");
    assert_eq!(roots.execution, private);
    assert_eq!(roots.ledger, private);
    assert_eq!(
        store.ledger_root(&s.metadata.id).expect("ledger root"),
        private
    );
}

#[test]
fn session_roots_bound_project_keeps_ledger_on_private_root() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let bound_id = s.metadata.id.clone();
    let project = std::env::temp_dir().join("pinvou3-bound-project-roots-test");
    store.set_execution_root_resolver(Arc::new(move |id: &str| {
        (id == bound_id).then(|| project.clone())
    }));
    let roots = store.session_roots(&s.metadata.id).expect("roots");
    assert_eq!(
        roots.execution,
        std::env::temp_dir().join("pinvou3-bound-project-roots-test")
    );
    // 绑了项目目录的原生代码会话：账本根恒为会话私有目录，不污染用户项目。
    let private = paths::session_workspace_dir(&s.metadata.id);
    assert_eq!(roots.ledger, private);
    assert_eq!(
        store.ledger_root(&s.metadata.id).expect("ledger root"),
        private
    );
    // 未绑定的会话不受 resolver 影响，两根仍一致。
    let other = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create other");
    let other_roots = store.session_roots(&other.metadata.id).expect("roots");
    assert_eq!(other_roots.execution, other_roots.ledger);
}

#[test]
fn session_roots_scheduled_run_uses_automation_workspace_for_both_roots() {
    let (store, _g) = isolated_store();
    let saved = store
        .create_scheduled_run(scheduled_profile("task-roots"))
        .expect("scheduled run");
    let workspace = task_workspace(&store, "task-roots");
    let roots = store.session_roots(&saved.metadata.id).expect("roots");
    assert_eq!(roots.execution, workspace);
    assert_eq!(roots.ledger, workspace);
    assert_eq!(
        store.ledger_root(&saved.metadata.id).expect("ledger root"),
        workspace
    );
}

fn scheduled_profile(task_id: &str) -> ScheduledRunProfile {
    ScheduledRunProfile {
        task_id: task_id.to_string(),
        model: "/scheduled-model".to_string(),
        model_id: Some("scheduled-model-id".to_string()),
        workspace: std::env::temp_dir().join("scheduled-workspace"),
        mode: ScheduledRunMode::Plan,
        allow_shell: true,
        trust_mode: false,
        auto_approve: false,
    }
}

fn unique_temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pinvou3-{label}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

#[test]
fn session_roots_user_workspace_binding_uses_bound_execution_root() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let bound_dir = unique_temp_dir("user-workspace-binding");
    std::fs::create_dir_all(&bound_dir).expect("create bound dir");
    store
        .bind_session_workspace(&s.metadata.id, bound_dir.clone())
        .expect("bind");

    let roots = store.session_roots(&s.metadata.id).expect("roots");
    assert_eq!(roots.execution, bound_dir);
    // 账本根恒为会话私有目录,不污染用户选择的目录。
    let private = paths::session_workspace_dir(&s.metadata.id);
    assert_eq!(roots.ledger, private);

    // 未绑定的会话不受影响,两根仍一致。
    let other = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create other");
    let other_roots = store.session_roots(&other.metadata.id).expect("roots");
    assert_eq!(other_roots.execution, other_roots.ledger);

    let _ = std::fs::remove_dir_all(&bound_dir);
}

#[test]
fn session_workspace_binding_survives_reload() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let bound_dir = unique_temp_dir("user-workspace-reload");
    std::fs::create_dir_all(&bound_dir).expect("create bound dir");
    store
        .bind_session_workspace(&s.metadata.id, bound_dir.clone())
        .expect("bind");

    let reopened = reopen_store(&store).expect("reopen");
    assert_eq!(
        reopened.session_workspace_binding(&s.metadata.id),
        Some(bound_dir.clone())
    );
    let roots = reopened.session_roots(&s.metadata.id).expect("roots");
    assert_eq!(roots.execution, bound_dir);

    let _ = std::fs::remove_dir_all(&bound_dir);
}

#[test]
fn delete_session_removes_workspace_binding() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let bound_dir = unique_temp_dir("user-workspace-delete");
    std::fs::create_dir_all(&bound_dir).expect("create bound dir");
    store
        .bind_session_workspace(&s.metadata.id, bound_dir.clone())
        .expect("bind");
    let sidecar = paths::sessions_root().join("_session_workspaces.json");
    assert!(sidecar.is_file());

    store.delete(&s.metadata.id).expect("delete");
    assert!(store.session_workspace_binding(&s.metadata.id).is_none());
    // 最后一条绑定被移除后 sidecar 文件一并删除。
    assert!(!sidecar.exists());

    let _ = std::fs::remove_dir_all(&bound_dir);
}

#[test]
fn load_session_workspaces_drops_ghost_entries() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let bound_dir = unique_temp_dir("user-workspace-ghost");
    std::fs::create_dir_all(&bound_dir).expect("create bound dir");
    store
        .bind_session_workspace(&s.metadata.id, bound_dir.clone())
        .expect("bind");
    // 模拟进程外删除会话后的残留:对应 `<id>.json` 不存在的绑定。
    store
        .bind_session_workspace("ghost-session-id", bound_dir.clone())
        .expect("bind ghost");

    store.load_session_workspaces();
    assert!(
        store
            .session_workspace_binding("ghost-session-id")
            .is_none()
    );
    assert_eq!(
        store.session_workspace_binding(&s.metadata.id),
        Some(bound_dir.clone())
    );
    // ghost 清理后重写文件:盘面与内存一致。
    let sidecar = paths::sessions_root().join("_session_workspaces.json");
    let on_disk: std::collections::HashMap<String, PathBuf> =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).expect("read sidecar"))
            .expect("parse sidecar");
    assert!(!on_disk.contains_key("ghost-session-id"));
    assert_eq!(on_disk.get(&s.metadata.id), Some(&bound_dir));

    let _ = std::fs::remove_dir_all(&bound_dir);
}

#[test]
fn validate_user_workspace_path_rejects_invalid_and_accepts_directory() {
    use super::validators::validate_user_workspace_path;

    assert!(validate_user_workspace_path("").is_err());
    assert!(validate_user_workspace_path("   ").is_err());
    assert!(validate_user_workspace_path("relative/dir").is_err());

    let missing = unique_temp_dir("user-workspace-missing");
    assert!(validate_user_workspace_path(missing.to_str().expect("utf8")).is_err());

    // 文件而非目录 → 拒绝。
    let file = unique_temp_dir("user-workspace-file");
    std::fs::write(&file, b"x").expect("seed file");
    assert!(validate_user_workspace_path(file.to_str().expect("utf8")).is_err());
    let _ = std::fs::remove_file(&file);

    // 合法目录 → canonicalize 后返回。
    let dir = unique_temp_dir("user-workspace-valid");
    std::fs::create_dir_all(&dir).expect("create dir");
    let validated = validate_user_workspace_path(dir.to_str().expect("utf8")).expect("valid dir");
    assert_eq!(validated, dir.canonicalize().expect("canonicalize"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn text_message(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
    }
}

fn scheduled_engine_state(
    messages: Vec<Message>,
    mode: ScheduledRunMode,
    token_accounting: ScheduledTokenAccounting,
) -> ScheduledEngineState {
    ScheduledEngineState {
        messages,
        system_prompt: Some(SystemPrompt::Text("scheduled system prompt".to_string())),
        model: "/engine-model".to_string(),
        workspace: std::env::temp_dir().join("scheduled-engine-workspace"),
        mode,
        token_accounting,
    }
}

fn chat_engine_state(messages: Vec<Message>) -> ChatEngineState {
    ChatEngineState {
        messages,
        system_prompt: Some(SystemPrompt::Text("ordinary system prompt".to_string())),
        model: "/ordinary-engine-model".to_string(),
        workspace: std::env::temp_dir().join("ordinary-engine-workspace"),
    }
}

#[test]
fn ordinary_session_updated_snapshot_is_persisted_authoritatively() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/initial-model".into(), None, std::env::temp_dir())
        .expect("create ordinary chat");
    store
        .update_messages(
            &session.metadata.id,
            vec![user_text("old"), assistant_text("old answer")],
        )
        .expect("seed transcript");

    let authoritative = vec![
        user_text("visible user prompt"),
        assistant_text("authoritative answer"),
    ];
    let saved = store
        .persist_chat_engine_state(
            &session.metadata.id,
            chat_engine_state(authoritative.clone()),
        )
        .expect("persist ordinary SessionUpdated");

    assert_eq!(saved.messages, authoritative);
    assert_eq!(saved.metadata.message_count, 2);
    assert_eq!(saved.metadata.model, "/ordinary-engine-model");
    assert_eq!(
        saved.system_prompt.as_deref(),
        Some("ordinary system prompt")
    );
    let reopened = reopen_store(&store).expect("reopen");
    assert_eq!(
        reopened
            .load(&session.metadata.id)
            .expect("load durable chat")
            .messages,
        authoritative
    );
}

#[test]
fn create_empty_with_id_preserves_requested_identity() {
    let (store, _g) = isolated_store();
    let requested_id = "eval_requested_identity";

    let session = store
        .create_empty_with_id(
            requested_id.to_string(),
            "/model".into(),
            None,
            std::env::temp_dir(),
        )
        .expect("create session with requested id");

    assert_eq!(session.metadata.id, requested_id);
    assert_eq!(
        store.load(requested_id).expect("load session").metadata.id,
        requested_id
    );
}

#[test]
fn admitted_display_fallback_is_revision_guarded_for_append_and_edit() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create chat");
    let baseline = vec![user_text("first"), assistant_text("answer")];
    store
        .update_messages(&session.metadata.id, baseline.clone())
        .unwrap();
    let baseline_revision = transcript_revision(&baseline).unwrap();

    let appended = store
        .persist_admitted_chat_display(
            &session.metadata.id,
            &baseline_revision,
            user_text("second"),
            false,
        )
        .unwrap();
    assert_eq!(
        appended.messages,
        vec![
            user_text("first"),
            assistant_text("answer"),
            user_text("second")
        ]
    );
    let unchanged = store
        .persist_admitted_chat_display(
            &session.metadata.id,
            &baseline_revision,
            user_text("must not duplicate"),
            false,
        )
        .unwrap();
    assert_eq!(unchanged.messages, appended.messages);

    let edit_revision = transcript_revision(&appended.messages).unwrap();
    let edited = store
        .persist_admitted_chat_display(
            &session.metadata.id,
            &edit_revision,
            user_text("edited second"),
            true,
        )
        .unwrap();
    assert_eq!(
        edited.messages,
        vec![
            user_text("first"),
            assistant_text("answer"),
            user_text("edited second")
        ]
    );
}

#[test]
fn forkguard_admitted_display_fallback_edit_cuts_before_trailing_tool_result() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create chat");
    // Tool results are also persisted with role="user". The edit cut must
    // land on the genuine prompt and remove its complete tool round-trip.
    let tool_result = Message {
        role: "user".into(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: "tool output".into(),
            is_error: None,
            content_blocks: None,
        }],
    };
    let baseline = vec![
        user_text("first"),
        assistant_tool_use("call_1"),
        tool_result,
        assistant_text("final answer"),
    ];
    store
        .update_messages(&session.metadata.id, baseline.clone())
        .unwrap();
    let revision = transcript_revision(&baseline).unwrap();

    let edited = store
        .persist_admitted_chat_display(&session.metadata.id, &revision, user_text("edited"), true)
        .unwrap();
    assert_eq!(edited.messages, vec![user_text("edited")]);
}

#[test]
fn forkguard_admitted_display_fallback_does_not_skip_unsupported_user_turn() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create chat");
    let image_only = Message {
        role: "user".into(),
        content: vec![ContentBlock::ImageUrl {
            image_url: ImageUrlContent {
                url: "data:image/png;base64,AAAA".into(),
            },
        }],
    };
    let baseline = vec![
        user_text("older editable prompt"),
        assistant_text("older response"),
        image_only,
    ];
    store
        .update_messages(&session.metadata.id, baseline.clone())
        .unwrap();
    let revision = transcript_revision(&baseline).unwrap();

    let error = store
        .persist_admitted_chat_display(
            &session.metadata.id,
            &revision,
            user_text("must not replace the older prompt"),
            true,
        )
        .expect_err("unsupported latest user content must reject the fallback edit");
    assert!(
        error
            .to_string()
            .contains("latest user content is not editable")
    );
    assert_eq!(
        store.load(&session.metadata.id).unwrap().messages,
        baseline,
        "a rejected fallback must leave the durable transcript unchanged"
    );
}

#[test]
fn scheduled_session_is_isolated_but_directly_loadable() {
    let (store, _g) = isolated_store();
    let chat = store
        .create_new("/chat-model".into(), None, std::env::temp_dir())
        .expect("create chat");
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-isolated"))
        .expect("create scheduled run");
    // 崩溃残留的评测会话(eval_ 前缀,含 GAIA 私有题目)不得进入用户列表。
    let eval_id = "eval_gaia-case-1_crash-leftover".to_string();
    let eval_session = create_saved_session_with_id_and_mode(
        eval_id.clone(),
        &[],
        "/eval-model",
        &paths::sessions_root(),
        0,
        None,
        Some("yolo"),
    );
    store
        .save_session_atomic(&eval_session)
        .expect("persist eval leftover");

    let listed = store.list().expect("list chats");
    assert!(listed.iter().any(|item| item.id == chat.metadata.id));
    assert!(!listed.iter().any(|item| item.id == scheduled.metadata.id));
    #[cfg(feature = "benchmark-hooks")]
    assert!(!listed.iter().any(|item| item.id == eval_id));
    #[cfg(not(feature = "benchmark-hooks"))]
    assert!(listed.iter().any(|item| item.id == eval_id));
    assert!(
        paths::sessions_root()
            .join(format!("{}.json", scheduled.metadata.id))
            .exists()
    );
    assert_eq!(
        store
            .load(&scheduled.metadata.id)
            .expect("direct load")
            .metadata
            .id,
        scheduled.metadata.id
    );
}

#[test]
fn scheduled_profile_survives_restart_and_routes_message_updates() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-restart"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();

    let reloaded = reopen_store(&store).expect("reboot");
    assert_eq!(
        reloaded
            .scheduled_profile(&id)
            .expect("profile after restart")
            .task_id,
        "task-restart"
    );
    reloaded
        .update_messages(&id, Vec::new())
        .expect("route scheduled update");
    assert!(
        reloaded
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists()
    );
}

#[test]
fn scheduled_profile_accepts_persisted_workspace_on_restart() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-legacy-workspace"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();
    let persisted_workspace = store
        .scheduled_root
        .join("automation-legacy")
        .join("workspace");
    let raw = std::fs::read_to_string(store.scheduled_profiles_path.as_ref())
        .expect("read scheduled profile registry");
    let mut registry: ScheduledProfileRegistry =
        serde_json::from_str(&raw).expect("parse scheduled profile registry");
    registry
        .sessions
        .get_mut(&id)
        .expect("scheduled profile")
        .workspace = persisted_workspace.clone();
    std::fs::write(
        store.scheduled_profiles_path.as_ref(),
        serde_json::to_vec_pretty(&registry).expect("serialize scheduled profile registry"),
    )
    .expect("write scheduled profile registry");

    let reloaded = reopen_store(&store).expect("reboot");
    assert_eq!(
        reloaded
            .scheduled_profile(&id)
            .expect("profile after restart")
            .workspace,
        persisted_workspace
    );
    assert!(persisted_workspace.exists());
}

#[test]
fn scheduled_conversation_accepts_interactive_mode_and_model_overrides() {
    let (store, _g) = isolated_store();
    let profile = scheduled_profile("task-interactive-profile");
    let scheduled = store
        .create_scheduled_run(profile.clone())
        .expect("create scheduled run");
    let id = scheduled.metadata.id;

    store
        .set_mode(&id, SerializableMode::Plan)
        .expect("scheduled conversation mode override");
    store
        .set_session_model_id(&id, Some("override-model".to_string()))
        .expect("scheduled conversation model override");
    let mut expected_profile = profile.clone();
    expected_profile.workspace = task_workspace(&store, &profile.task_id);
    assert_eq!(store.scheduled_profile(&id), Some(expected_profile));
    assert_eq!(store.mode_state(&id).mode, SerializableMode::Plan);
    assert_eq!(
        store.session_model_id(&id).as_deref(),
        Some("override-model")
    );
    assert_eq!(
        store.session_model_override(&id).as_deref(),
        Some("override-model")
    );
}

#[test]
fn scheduled_conversation_model_override_precedes_profile_fallback() {
    let (store, _g) = isolated_store();
    let mut profile = scheduled_profile("task-model-authority");
    profile.model_id = None;
    let scheduled = store
        .create_scheduled_run(profile)
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    store
        .session_models
        .write()
        .insert(id.clone(), "legacy-model-id".to_string());

    assert_eq!(
        store.session_model_id(&id).as_deref(),
        Some("legacy-model-id"),
        "an explicit interactive model choice must win after opening the run as a chat"
    );
}

#[test]
fn scheduled_mode_override_preserves_live_auxiliary_session_state() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-live-aux-state"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    store.set_active_persona(&id, Some("scheduled-persona".to_string()));
    store.set_mounted_collection(&id, Some(42));
    store
        .mode_states
        .write()
        .entry(id.clone())
        .or_default()
        .mode = SerializableMode::Plan;

    let state = store.mode_state(&id);
    assert_eq!(state.mode, SerializableMode::Plan);
    assert_eq!(state.active_persona.as_deref(), Some("scheduled-persona"));
    assert_eq!(state.mounted_collection, Some(42));
}

#[test]
fn scheduled_engine_state_persists_full_snapshot_and_preserves_identity_and_profile() {
    let (store, _g) = isolated_store();
    let profile = scheduled_profile("task-engine-state");
    let scheduled = store
        .create_scheduled_run(profile.clone())
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();
    store
        .set_title(&id, "Kept scheduled title".to_string())
        .expect("set scheduled title");
    let before = store.load(&id).expect("load before engine state");
    let messages = vec![
        text_message("user", "run the scheduled task"),
        text_message("assistant", "scheduled result"),
    ];

    let persisted = store
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                messages.clone(),
                ScheduledRunMode::Yolo,
                ScheduledTokenAccounting::EngineCumulative {
                    base_total_tokens: 40,
                    engine_total_tokens: 12,
                },
            ),
        )
        .expect("persist scheduled engine state");

    assert_eq!(persisted.metadata.id, before.metadata.id);
    assert_eq!(persisted.metadata.title, before.metadata.title);
    assert_eq!(persisted.metadata.created_at, before.metadata.created_at);
    assert_eq!(persisted.metadata.message_count, messages.len());
    assert_eq!(persisted.metadata.total_tokens, 52);
    assert_eq!(persisted.metadata.model, "/engine-model");
    assert_eq!(
        persisted.metadata.workspace,
        task_workspace(&store, &profile.task_id)
    );
    assert_eq!(persisted.metadata.mode.as_deref(), Some("yolo"));
    assert_eq!(persisted.messages, messages);
    assert_eq!(
        persisted.system_prompt.as_deref(),
        Some("scheduled system prompt")
    );
    let mut expected_profile = profile.clone();
    expected_profile.workspace = task_workspace(&store, &profile.task_id);
    assert_eq!(store.scheduled_profile(&id), Some(expected_profile.clone()));

    let reloaded = reopen_store(&store).expect("reboot");
    assert_eq!(reloaded.scheduled_profile(&id), Some(expected_profile));
    let from_disk = reloaded.load(&id).expect("load persisted engine state");
    assert_eq!(from_disk.metadata.total_tokens, 52);
    assert_eq!(from_disk.messages, persisted.messages);
    assert_eq!(from_disk.system_prompt, persisted.system_prompt);
}

#[test]
fn scheduled_engine_token_accounting_preserves_updates_and_accumulates_across_restarts() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-token-accounting"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();

    store
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                vec![text_message("user", "first turn")],
                ScheduledRunMode::Plan,
                ScheduledTokenAccounting::EngineCumulative {
                    base_total_tokens: 0,
                    engine_total_tokens: 100,
                },
            ),
        )
        .expect("persist first engine snapshot");
    store
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                vec![
                    text_message("user", "first turn"),
                    text_message("assistant", "incremental update"),
                ],
                ScheduledRunMode::Plan,
                ScheduledTokenAccounting::PreservePersisted,
            ),
        )
        .expect("persist SessionUpdated-equivalent state");
    assert_eq!(
        store
            .load(&id)
            .expect("load after update")
            .metadata
            .total_tokens,
        100
    );

    let reloaded = reopen_store(&store).expect("restart before later turn");
    reloaded
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                vec![text_message("assistant", "later turn")],
                ScheduledRunMode::Yolo,
                ScheduledTokenAccounting::EngineCumulative {
                    base_total_tokens: 100,
                    engine_total_tokens: 25,
                },
            ),
        )
        .expect("persist later engine snapshot");
    reloaded
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                vec![text_message("assistant", "same engine next turn")],
                ScheduledRunMode::Yolo,
                ScheduledTokenAccounting::EngineCumulative {
                    base_total_tokens: 100,
                    engine_total_tokens: 40,
                },
            ),
        )
        .expect("persist cumulative same-engine snapshot");

    assert_eq!(
        reloaded
            .load(&id)
            .expect("load accumulated total")
            .metadata
            .total_tokens,
        140,
        "same-engine cumulative usage must not be added twice"
    );
}

#[test]
fn scheduled_engine_state_entry_rejects_normal_chat_without_mutation() {
    let (store, _g) = isolated_store();
    let chat = store
        .create_new(
            "/chat-model".to_string(),
            None,
            std::env::temp_dir().join("chat-workspace"),
        )
        .expect("create chat");

    let error = store
        .persist_scheduled_engine_state(
            &chat.metadata.id,
            scheduled_engine_state(
                vec![text_message("user", "must not persist")],
                ScheduledRunMode::Plan,
                ScheduledTokenAccounting::EngineCumulative {
                    base_total_tokens: 0,
                    engine_total_tokens: 99,
                },
            ),
        )
        .expect_err("normal chat must not use scheduled persistence");

    assert!(error.to_string().contains("not a scheduled-run session"));
    let token_error = store
        .persist_scheduled_token_total(&chat.metadata.id, 0, 99)
        .expect_err("normal chat must not use scheduled token persistence");
    assert!(
        token_error
            .to_string()
            .contains("not a scheduled-run session")
    );
    let unchanged = store.load(&chat.metadata.id).expect("load unchanged chat");
    assert_eq!(unchanged.metadata.title, chat.metadata.title);
    assert_eq!(unchanged.metadata.model, chat.metadata.model);
    assert_eq!(unchanged.metadata.workspace, chat.metadata.workspace);
    assert_eq!(unchanged.metadata.total_tokens, 0);
    assert!(unchanged.messages.is_empty());
    assert!(unchanged.system_prompt.is_none());
}

#[test]
fn public_artifact_replace_rejects_scheduled_without_mutation() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-artifact-owner"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    let original = std::env::temp_dir().join("scheduled-original-artifact.md");
    store
        .append_scheduled_artifact_path(&id, original.clone())
        .expect("backend artifact append");
    let before = store.load(&id).expect("load before replacement");

    let error = store
        .update_artifacts(
            &id,
            vec![
                std::env::temp_dir()
                    .join("ui-replacement.md")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .expect_err("public replacement must reject scheduled sessions");

    assert!(error.to_string().contains("scheduled-run"));
    let after = store.load(&id).expect("load after rejection");
    assert_eq!(after.artifacts.len(), before.artifacts.len());
    assert_eq!(
        after
            .artifacts
            .iter()
            .map(|artifact| artifact.storage_path.clone())
            .collect::<Vec<_>>(),
        before
            .artifacts
            .iter()
            .map(|artifact| artifact.storage_path.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(after.metadata.updated_at, before.metadata.updated_at);
    assert_eq!(before.artifacts[0].storage_path, original);
}

#[test]
fn ordinary_artifact_replace_behavior_is_unchanged() {
    let (store, _g) = isolated_store();
    let chat = store
        .create_new("/chat-model".into(), None, std::env::temp_dir())
        .expect("create chat");
    let artifact = std::env::temp_dir().join("ordinary-artifact.md");

    store
        .update_artifacts(
            &chat.metadata.id,
            vec![artifact.to_string_lossy().into_owned()],
        )
        .expect("ordinary replacement remains supported");

    assert_eq!(
        store.load(&chat.metadata.id).expect("load chat").artifacts[0].storage_path,
        artifact
    );
}

#[test]
fn scheduled_agent_mode_round_trips_without_collapsing_profile_or_metadata() {
    let (store, _g) = isolated_store();
    let mut profile = scheduled_profile("task-agent-mode");
    profile.mode = ScheduledRunMode::Agent;
    let scheduled = store
        .create_scheduled_run(profile.clone())
        .expect("create agent scheduled run");
    let id = scheduled.metadata.id.clone();

    assert_eq!(scheduled.metadata.mode.as_deref(), Some("agent"));
    assert_eq!(
        profile.mode.to_app_mode(),
        deepseek_tui::tui::app::AppMode::Agent
    );
    let persisted = store
        .persist_scheduled_engine_state(
            &id,
            ScheduledEngineState {
                messages: vec![text_message("assistant", "agent result")],
                system_prompt: Some(SystemPrompt::Text("agent prompt".to_string())),
                model: "/agent-model".to_string(),
                workspace: std::env::temp_dir().join("agent-workspace"),
                mode: ScheduledRunMode::Agent,
                token_accounting: ScheduledTokenAccounting::PreservePersisted,
            },
        )
        .expect("persist agent engine state");

    assert_eq!(persisted.metadata.mode.as_deref(), Some("agent"));
    assert_eq!(
        store.scheduled_profile(&id).expect("agent profile").mode,
        ScheduledRunMode::Agent
    );
    assert_eq!(store.mode_state(&id).mode, SerializableMode::Yolo);

    let reloaded = reopen_store(&store).expect("restart after agent persistence");
    assert_eq!(
        reloaded
            .scheduled_profile(&id)
            .expect("agent profile after restart")
            .mode,
        ScheduledRunMode::Agent
    );
    assert_eq!(reloaded.mode_state(&id).mode, SerializableMode::Yolo);
    assert_eq!(
        reloaded
            .load(&id)
            .expect("agent session after restart")
            .metadata
            .mode
            .as_deref(),
        Some("agent")
    );
}

#[test]
fn scheduled_terminal_token_persistence_does_not_replace_engine_state() {
    let (store, _g) = isolated_store();
    let profile = scheduled_profile("task-terminal-token");
    let scheduled = store
        .create_scheduled_run(profile.clone())
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();
    store
        .persist_scheduled_engine_state(
            &id,
            scheduled_engine_state(
                vec![
                    text_message("user", "retain this request"),
                    text_message("assistant", "retain this response"),
                ],
                ScheduledRunMode::Plan,
                ScheduledTokenAccounting::PreservePersisted,
            ),
        )
        .expect("persist cached SessionUpdated state");
    let before = store.load(&id).expect("load before terminal usage");

    let after = store
        .persist_scheduled_token_total(&id, 40, 9)
        .expect("persist terminal token total");

    assert_eq!(after.metadata.total_tokens, 49);
    assert_eq!(after.metadata.id, before.metadata.id);
    assert_eq!(after.metadata.title, before.metadata.title);
    assert_eq!(after.metadata.created_at, before.metadata.created_at);
    assert_eq!(after.metadata.message_count, before.metadata.message_count);
    assert_eq!(after.metadata.model, before.metadata.model);
    assert_eq!(after.metadata.workspace, before.metadata.workspace);
    assert_eq!(after.metadata.mode, before.metadata.mode);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.system_prompt, before.system_prompt);
    assert_eq!(after.artifacts, before.artifacts);
    let mut expected_profile = profile.clone();
    expected_profile.workspace = task_workspace(&store, &profile.task_id);
    assert_eq!(store.scheduled_profile(&id), Some(expected_profile));

    let reloaded = reopen_store(&store).expect("restart after terminal usage");
    let from_disk = reloaded
        .load(&id)
        .expect("load terminal usage after restart");
    assert_eq!(from_disk.metadata.total_tokens, 49);
    assert_eq!(from_disk.messages, before.messages);
    assert_eq!(from_disk.system_prompt, before.system_prompt);

    let later = reloaded
        .persist_scheduled_token_total(&id, 49, 11)
        .expect("persist later engine token total");
    assert_eq!(later.metadata.total_tokens, 60);
    assert_eq!(later.messages, before.messages);
    assert_eq!(later.system_prompt, before.system_prompt);
}

#[test]
fn checked_scheduled_delete_removes_profile_json_and_runtime_directory() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-delete"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id.clone();
    let runtime_dir = paths::sessions_root().join(&id);
    std::fs::create_dir_all(runtime_dir.join("artifacts")).expect("runtime dir");
    store.set_active(Some(id.clone()));
    store
        .set_session_model_id(&id, Some("override-model".to_string()))
        .expect("scheduled conversation model override");
    store.set_hidden(&id, true);
    store.set_pinned(&id, true);

    let err = store
        .delete(&id)
        .expect_err("ordinary chat deletion must reject scheduled runs");
    assert!(err.to_string().contains("through their automation"));

    let deletions = record_session_deletions(&store);

    let err = store
        .delete_scheduled_run(&id, "another-task")
        .expect_err("wrong owner must fail");
    assert!(err.to_string().contains("task ownership"));
    assert!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
    assert!(runtime_dir.exists());

    store
        .delete_scheduled_run(&id, "task-delete")
        .expect("delete scheduled run");
    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        std::slice::from_ref(&id)
    );
    assert!(store.scheduled_profile(&id).is_none());
    assert!(store.active_id().is_none());
    assert!(store.session_model_id(&id).is_none());
    assert!(!store.is_hidden(&id));
    assert!(!store.is_pinned(&id));
    assert!(!runtime_dir.exists());
    assert!(!paths::sessions_root().join(format!("{id}.json")).exists());
}

#[test]
fn scheduled_delete_notifies_hook_when_record_commit_precedes_cleanup_error() {
    let (store, _g) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-partial-delete"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    let runtime_dir = store.manager.sessions_dir().join(&id);
    std::fs::create_dir_all(&runtime_dir).expect("create scheduled runtime dir");
    std::fs::write(runtime_dir.join("pending.txt"), "pending cleanup")
        .expect("write runtime marker");
    store.set_active(Some(id.clone()));
    store
        .set_session_model_id(&id, Some("partial-delete-model".to_string()))
        .expect("set scheduled model override");
    store.set_hidden(&id, true);
    store.set_pinned(&id, true);
    let deletions = record_session_deletions(&store);
    store
        .inject_post_record_delete_fault(&id, ErrorKind::PermissionDenied)
        .expect("inject post-record cleanup error");

    let error = store
        .delete_scheduled_run(&id, "task-partial-delete")
        .expect_err("cleanup error must remain visible to caller");

    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .expect("original io cleanup error")
            .kind(),
        ErrorKind::PermissionDenied
    );
    assert!(
        !store
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists(),
        "durable session record was committed as deleted"
    );
    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        std::slice::from_ref(&id)
    );
    assert!(store.scheduled_profile(&id).is_none());
    assert!(store.active_id().is_none());
    assert!(store.session_model_id(&id).is_none());
    assert!(!store.is_hidden(&id));
    assert!(!store.is_pinned(&id));
    assert!(!runtime_dir.exists());
}

#[test]
fn scheduled_delete_retry_finishes_runtime_cleanup_after_profile_removal() {
    let (store, _guard) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-runtime-delete-retry"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    let runtime_dir = store.manager.sessions_dir().join(&id);
    std::fs::create_dir_all(&runtime_dir).expect("create scheduled runtime dir");
    std::fs::write(runtime_dir.join("pending.txt"), "pending cleanup")
        .expect("write runtime marker");
    store.set_active(Some(id.clone()));
    store
        .set_session_model_id(&id, Some("stale-model".to_string()))
        .expect("set scheduled model override");
    store.set_hidden(&id, true);
    store.set_pinned(&id, true);
    store
        .inject_post_record_delete_fault(&id, ErrorKind::PermissionDenied)
        .expect("leave the runtime directory after durable record deletion");
    store
        .inject_scheduled_runtime_delete_fault(&id, ErrorKind::PermissionDenied)
        .expect("fail the scheduled runtime cleanup once");

    let error = store
        .delete_scheduled_run(&id, "task-runtime-delete-retry")
        .expect_err("the first runtime cleanup failure must remain visible");

    assert!(error.to_string().contains("runtime cleanup"));
    assert!(
        !store
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists()
    );
    assert!(store.scheduled_profile(&id).is_none());
    assert!(runtime_dir.exists(), "failed cleanup must remain retryable");
    assert!(store.active_id().is_none());
    assert!(store.session_model_id(&id).is_none());
    assert!(!store.is_hidden(&id));
    assert!(!store.is_pinned(&id));

    store
        .delete_scheduled_run(&id, "task-runtime-delete-retry")
        .expect("idempotent retry must finish scheduled runtime cleanup");

    assert!(!runtime_dir.exists());
    assert!(store.scheduled_profile(&id).is_none());
}

#[test]
fn scheduled_delete_without_profile_does_not_misreport_retained_transcript_as_deleted() {
    let (store, _guard) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-orphan-transcript"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    store.scheduled_profiles.write().remove(&id);
    store
        .save_scheduled_profiles()
        .expect("persist missing-profile state");
    let deletions = record_session_deletions(&store);

    store
        .delete_scheduled_run(&id, "task-orphan-transcript")
        .expect("missing profile is an idempotent no-op");

    assert!(
        store.load(&id).is_ok(),
        "an orphan scheduled transcript is deliberately retained"
    );
    assert!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "a retained durable transcript must not emit a deletion lifecycle event"
    );
}

#[test]
fn scheduled_delete_profile_persistence_failure_remains_retryable_after_record_commit() {
    let (store, _guard) = isolated_store();
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-profile-delete-retry"))
        .expect("create scheduled run");
    let id = scheduled.metadata.id;
    store.set_active(Some(id.clone()));
    store
        .set_session_model_id(&id, Some("stale-model".to_string()))
        .expect("set scheduled model override");

    let profile_path = store.scheduled_profiles_path.as_ref();
    std::fs::remove_file(profile_path).expect("remove profile registry");
    std::fs::create_dir(profile_path).expect("replace profile registry with a directory");

    let error = store
        .delete_scheduled_run(&id, "task-profile-delete-retry")
        .expect_err("profile removal persistence must fail");

    assert!(error.to_string().contains("profile persistence"));
    assert!(
        !store
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists(),
        "the durable transcript deletion remains committed"
    );
    assert!(
        store.scheduled_profile(&id).is_some(),
        "in-memory ownership metadata must remain available for retry"
    );
    assert!(store.active_id().is_none());
    assert!(
        store.session_model_override(&id).is_none(),
        "the deleted session's independent model sidecar must be purged"
    );
    assert_eq!(
        store.session_model_id(&id).as_deref(),
        Some("scheduled-model-id"),
        "retry ownership metadata still supplies the scheduled profile model"
    );

    std::fs::remove_dir(profile_path).expect("repair profile registry path");
    store
        .delete_scheduled_run(&id, "task-profile-delete-retry")
        .expect("retry persists profile removal");
    assert!(store.scheduled_profile(&id).is_none());
    assert!(store.session_model_id(&id).is_none());
}

#[test]
fn scheduled_creation_rolls_back_when_profile_write_fails() {
    let root = std::env::temp_dir().join(format!(
        "pinvou3-scheduled-rollback-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let profile_path = root.join("profiles.json");
    let store = SessionStore::from_paths(
        root.join("sessions"),
        profile_path.clone(),
        root.join("scheduled"),
    )
    .expect("store");
    std::fs::create_dir_all(&profile_path).expect("make profile path a directory");
    let deletions = record_session_deletions(&store);

    // 种子会话用于构造完整字段的幽灵元数据(改 id/title),其落盘本身 bump 一次
    // 代数;随后记录调用前代数。
    let seed = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("seed session");
    let generation_before = store
        .list_cache_generation
        .load(std::sync::atomic::Ordering::Acquire);

    let err = store
        .create_scheduled_run(scheduled_profile("task-rollback"))
        .expect_err("profile write must fail");

    assert!(err.to_string().contains("save scheduled session profile"));
    assert!(
        store
            .manager
            .list_sessions()
            .expect("session list")
            .iter()
            .all(|m| !m.id.starts_with("sched-")),
        "the SavedSession must be removed when profile persistence fails"
    );
    // 回滚删除也必须失效列表缓存:并发读者恰在 save 失效与回滚删除之间重扫,
    // 会以「save 失效后的代数」(= 调用前代数 + 1,save_session_atomic 恰好
    // bump 一次)回填含 sched-*.json 的快照。注入该幽灵条目:若回滚路径不
    // 失效(修复前),该代数仍是当前代,幽灵会被永久供应;回滚失效后该代数
    // 已过期,读取触发重扫,幽灵不可见。
    let mut phantom = seed.metadata.clone();
    phantom.id = "sched-phantom".into();
    phantom.title = "Scheduled run".into();
    *store.list_cache.write() = Some((
        generation_before.wrapping_add(1),
        std::sync::Arc::new(vec![phantom]),
    ));
    let cached = store
        .list_sessions_cached()
        .expect("cached list after rollback");
    assert!(
        !cached.iter().any(|m| m.id.starts_with("sched-")),
        "rollback invalidation must prevent a phantom scheduled session from surviving in the cache"
    );
    assert!(store.scheduled_profiles.read().is_empty());
    let rollback_deletions = deletions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(rollback_deletions.len(), 1);
    assert!(rollback_deletions[0].starts_with("sched-"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn scheduled_sessions_wait_for_coordinated_run_retention() {
    let (store, _g) = isolated_store();
    let chat = store
        .create_new("/chat-model".into(), None, std::env::temp_dir())
        .expect("create chat");
    let mut scheduled_ids = Vec::new();

    for index in 0..51 {
        let scheduled = store
            .create_scheduled_run(scheduled_profile(&format!("task-{index}")))
            .expect("create scheduled run");
        std::fs::create_dir_all(paths::sessions_root().join(&scheduled.metadata.id))
            .expect("runtime dir");
        store
            .mode_states
            .write()
            .insert(scheduled.metadata.id.clone(), SessionModeState::default());
        store
            .session_models
            .write()
            .insert(scheduled.metadata.id.clone(), "stale-model".to_string());
        store
            .pinned_sessions
            .write()
            .insert(scheduled.metadata.id.clone(), "stale-pin".to_string());
        store
            .hidden_sessions
            .write()
            .insert(scheduled.metadata.id.clone(), "stale-hidden".to_string());
        scheduled_ids.push(scheduled.metadata.id);
    }

    assert_eq!(
        store.manager.list_sessions().expect("session list").len(),
        52
    );
    assert_eq!(store.scheduled_profiles.read().len(), 51);
    assert_eq!(
        store
            .load(&chat.metadata.id)
            .expect("chat retained")
            .metadata
            .id,
        chat.metadata.id,
        "scheduled retention must not consume the ordinary-chat budget"
    );

    assert!(scheduled_ids.iter().all(|id| {
        store.scheduled_profile(id).is_some()
            && store.mode_states.read().contains_key(id)
            && store.session_models.read().contains_key(id)
            && store.pinned_sessions.read().contains_key(id)
            && store.hidden_sessions.read().contains_key(id)
            && paths::sessions_root().join(id).exists()
    }));
}

#[test]
fn orphan_transcript_does_not_consume_live_scheduled_retention_budget() {
    let (store, _g) = isolated_store();
    let mut live_ids = Vec::new();
    for index in 0..MAX_SESSIONS_PER_KIND {
        let session = store
            .create_scheduled_run(scheduled_profile(&format!("live-task-{index}")))
            .expect("create live scheduled conversation");
        live_ids.push(session.metadata.id);
    }

    let orphan_id = "sched-newer-orphan";
    let mut orphan = create_saved_session_with_id_and_mode(
        orphan_id.to_string(),
        &[],
        "/scheduled-model",
        store.scheduled_root.as_ref(),
        0,
        None,
        Some("yolo"),
    );
    orphan.metadata.updated_at = Utc::now() + chrono::Duration::minutes(1);
    store
        .save_session_atomic(&orphan)
        .expect("persist orphan transcript");
    store
        .enforce_session_retention_locked()
        .expect("enforce retention");

    assert_eq!(store.scheduled_profiles.read().len(), MAX_SESSIONS_PER_KIND);
    assert!(
        live_ids
            .iter()
            .all(|id| store.scheduled_profile(id).is_some() && store.load(id).is_ok())
    );
    assert!(store.load(orphan_id).is_ok(), "orphan must be preserved");
    assert!(store.scheduled_profile(orphan_id).is_none());
}

#[test]
fn chat_retention_does_not_evict_scheduled_conversation() {
    let (store, _g) = isolated_store();
    let deletions = record_session_deletions(&store);
    let scheduled = store
        .create_scheduled_run(scheduled_profile("task-retained-across-chat-pruning"))
        .expect("scheduled conversation");

    for index in 0..51 {
        let mut chat = store
            .create_new(
                "/chat-model".to_string(),
                None,
                std::env::temp_dir().join(format!("chat-{index}")),
            )
            .expect("create chat");
        chat.metadata.title = format!("chat {index}");
        store.save(&chat).expect("persist chat");
    }

    assert!(store.scheduled_session_exists(&scheduled.metadata.id));
    assert!(store.scheduled_profile(&scheduled.metadata.id).is_some());
    assert_eq!(store.list().expect("chat list").len(), 50);
    let pruned = deletions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .first()
        .cloned()
        .expect("retention deletion hook");
    assert!(
        store.load(&pruned).is_err(),
        "retention event must name the deleted chat"
    );
}

#[test]
fn retention_notifies_hook_when_record_commit_precedes_cleanup_error() {
    let (store, _g) = isolated_store();
    let oldest_id = "retention-partial-delete";
    let now = Utc::now();
    for index in 0..=MAX_SESSIONS_PER_KIND {
        let id = if index == MAX_SESSIONS_PER_KIND {
            oldest_id.to_string()
        } else {
            format!("retention-live-{index}")
        };
        let mut session = create_saved_session_with_id_and_mode(
            id,
            &[],
            "/retention-model",
            &std::env::temp_dir(),
            0,
            None,
            None,
        );
        session.metadata.updated_at = now - chrono::Duration::seconds(index as i64);
        store
            .save_session_atomic(&session)
            .expect("seed session without eager retention");
    }
    store
        .session_models
        .write()
        .insert(oldest_id.to_string(), "stale-model".to_string());
    let deletions = record_session_deletions(&store);
    store
        .inject_post_record_delete_fault(oldest_id, ErrorKind::PermissionDenied)
        .expect("inject post-record cleanup error");

    let error = store
        .enforce_session_retention_locked()
        .expect_err("retention must preserve cleanup error");

    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .expect("original io cleanup error")
            .kind(),
        ErrorKind::PermissionDenied
    );
    assert!(
        !store
            .manager
            .sessions_dir()
            .join(format!("{oldest_id}.json"))
            .exists()
    );
    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[oldest_id.to_string()]
    );
    assert!(
        !store.session_models.read().contains_key(oldest_id),
        "committed retention deletions purge process-local side maps"
    );
    assert_eq!(store.list().expect("retained chat list").len(), 50);
}

#[test]
fn boot_prunes_only_stale_scheduled_runtime_sidecars() {
    let (store, _g) = isolated_store();
    let live = store
        .create_scheduled_run(scheduled_profile("task-live-sidecars"))
        .expect("create live scheduled run")
        .metadata
        .id;
    let stale = store
        .create_scheduled_run(scheduled_profile("task-stale-sidecars"))
        .expect("create stale scheduled run")
        .metadata
        .id;
    for (id, suffix) in [(&live, "live"), (&stale, "stale")] {
        store
            .session_models
            .write()
            .insert(id.clone(), format!("{suffix}-model"));
        store
            .pinned_sessions
            .write()
            .insert(id.clone(), format!("{suffix}-pin"));
        store
            .hidden_sessions
            .write()
            .insert(id.clone(), format!("{suffix}-hidden"));
    }
    store.save_session_models();
    store.save_pinned_sessions();
    store.save_hidden_sessions();
    std::fs::remove_file(store.manager.sessions_dir().join(format!("{stale}.json")))
        .expect("simulate stale profile after session loss");
    let reloaded = reopen_store(&store).expect("reboot and prune sidecars");

    assert!(reloaded.session_models.read().contains_key(&live));
    assert!(reloaded.pinned_sessions.read().contains_key(&live));
    assert!(reloaded.hidden_sessions.read().contains_key(&live));
    assert!(!reloaded.session_models.read().contains_key(&stale));
    assert!(!reloaded.pinned_sessions.read().contains_key(&stale));
    assert!(!reloaded.hidden_sessions.read().contains_key(&stale));
    for sidecar in [
        "_session_models.json",
        "_pinned_sessions.json",
        "_hidden_sessions.json",
    ] {
        let path = paths::sessions_root().join(sidecar);
        if let Ok(contents) = std::fs::read_to_string(path) {
            assert!(contents.contains(&live));
            assert!(!contents.contains(&stale));
        }
    }
}

#[test]
fn boot_retains_orphan_transcript_left_before_profile_commit() {
    let (store, _g) = isolated_store();
    let id = "sched-orphan-before-profile";
    let orphan = create_saved_session_with_id_and_mode(
        id.to_string(),
        &[],
        "/scheduled-model",
        &std::env::temp_dir(),
        0,
        None,
        Some("yolo"),
    );
    store.manager.save_session(&orphan).expect("save orphan");
    let runtime_dir = paths::sessions_root().join(id);
    std::fs::create_dir_all(&runtime_dir).expect("runtime dir");

    let reloaded = reopen_store(&store).expect("reboot and reconcile");
    assert!(!reloaded.scheduled_session_exists(id));
    assert!(paths::sessions_root().join(format!("{id}.json")).exists());
    assert!(runtime_dir.exists());
    assert!(
        !reloaded
            .list()
            .expect("ordinary chat list")
            .iter()
            .any(|metadata| metadata.id == id)
    );
}

#[test]
fn concurrent_scheduled_creates_do_not_lose_registry_entries() {
    let (store, _g) = isolated_store();
    let handles: Vec<_> = (0..12)
        .map(|index| {
            let cloned = store.clone();
            std::thread::spawn(move || {
                cloned
                    .create_scheduled_run(scheduled_profile(&format!("task-concurrent-{index}")))
                    .expect("concurrent create")
                    .metadata
                    .id
            })
        })
        .collect();
    let ids: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();

    let reloaded = reopen_store(&store).expect("reboot");
    assert_eq!(ids.len(), 12);
    assert!(
        ids.iter()
            .all(|id| reloaded.scheduled_profile(id).is_some())
    );
}

#[test]
fn scheduled_runs_get_independent_conversations_and_share_the_task_workspace() {
    let (store, _g) = isolated_store();
    let first = store
        .create_scheduled_run(scheduled_profile("task-shared-workspace"))
        .expect("first run session");

    let mut edited = scheduled_profile("task-shared-workspace");
    edited.model = "edited-model".to_string();
    let second = store
        .create_scheduled_run(edited)
        .expect("second run session");

    assert_ne!(
        first.metadata.id, second.metadata.id,
        "every run of a task must create an independent conversation"
    );
    assert_eq!(
        store
            .scheduled_profile(&first.metadata.id)
            .expect("profile")
            .model,
        "/scheduled-model",
        "an earlier run keeps the profile captured for its conversation"
    );
    assert_eq!(
        store
            .scheduled_profile(&second.metadata.id)
            .expect("second profile")
            .model,
        "edited-model",
        "task edits apply to later run conversations"
    );
    assert_eq!(
        first.metadata.workspace, second.metadata.workspace,
        "conversations from one task must share its workspace"
    );
    assert_eq!(
        first.metadata.workspace,
        task_workspace(&store, "task-shared-workspace")
    );

    let other = store
        .create_scheduled_run(scheduled_profile("task-other"))
        .expect("other task session");
    assert_ne!(
        first.metadata.workspace, other.metadata.workspace,
        "different tasks must keep separate workspaces"
    );
}

#[test]
fn corrupt_previous_run_does_not_block_a_new_conversation() {
    let (store, _g) = isolated_store();
    let first = store
        .create_scheduled_run(scheduled_profile("task-corrupt"))
        .expect("create scheduled conversation");
    std::fs::write(
        store
            .manager
            .sessions_dir()
            .join(format!("{}.json", first.metadata.id)),
        b"{not valid json",
    )
    .expect("corrupt transcript fixture");

    let second = store
        .create_scheduled_run(scheduled_profile("task-corrupt"))
        .expect("a new run must not load or reuse a corrupt older conversation");
    assert_ne!(first.metadata.id, second.metadata.id);
    let ids = store.scheduled_session_ids_for_task("task-corrupt");
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&first.metadata.id));
    assert!(ids.contains(&second.metadata.id));
}

#[test]
fn set_title_updates_metadata() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    store
        .set_title(&s.metadata.id, "改个名字".into())
        .expect("rename");
    let loaded = store.load(&s.metadata.id).expect("load");
    assert_eq!(loaded.metadata.title, "改个名字");
}

#[test]
fn touch_activity_updates_timestamp_without_mutating_conversation() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    std::thread::sleep(std::time::Duration::from_millis(2));

    store
        .touch_activity(&s.metadata.id)
        .expect("touch activity");

    let loaded = store.load(&s.metadata.id).expect("load");
    assert!(loaded.metadata.updated_at > s.metadata.updated_at);
    assert_eq!(loaded.metadata.title, s.metadata.title);
    assert_eq!(loaded.metadata.message_count, s.metadata.message_count);
    assert_eq!(loaded.messages, s.messages);
}

#[test]
fn update_messages_rejects_unrelated_short_overwrite() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    store
        .update_messages(
            &s.metadata.id,
            vec![
                user_text("old 1"),
                assistant_text("old 2"),
                user_text("old 3"),
            ],
        )
        .expect("seed messages");

    let result = store.update_messages(
        &s.metadata.id,
        vec![user_text("new unrelated"), assistant_text("new answer")],
    );

    assert!(result.is_err(), "short unrelated overwrite is rejected");
    let loaded = store.load(&s.metadata.id).expect("load");
    assert_eq!(loaded.messages.len(), 3);
}

#[test]
fn forkguard_runtime_snapshot_load_does_not_repair_in_flight_tool_call() {
    let (store, _guard) = isolated_store();
    let session = store
        .create_new("model".into(), None, std::env::temp_dir())
        .expect("create");
    let messages = vec![assistant_tool_use("call-in-flight")];
    store
        .update_messages(&session.metadata.id, messages.clone())
        .expect("persist in-flight call");

    let loaded = store.load(&session.metadata.id).expect("snapshot load");

    assert_eq!(loaded.messages, messages);
    assert_eq!(loaded.metadata.message_count, 1);
    assert!(!loaded.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { content, .. }
                    if content.contains("crashed_and_repaired")
            )
        })
    }));

    let secondary = SessionStore::boot().expect("open secondary runtime store");
    let secondary_loaded = secondary
        .load(&session.metadata.id)
        .expect("secondary snapshot load");
    assert_eq!(secondary_loaded.messages, messages);
    assert_eq!(secondary_loaded.metadata.message_count, 1);
}

#[test]
fn forkguard_boot_repairs_interrupted_tool_call_once() {
    let (store, _guard) = isolated_store();
    let session = store
        .create_new("model".into(), None, std::env::temp_dir())
        .expect("create");
    store
        .update_messages(
            &session.metadata.id,
            vec![assistant_tool_use("call-crashed")],
        )
        .expect("persist interrupted call");

    let recovered = SessionStore::boot_for_process_startup().expect("recover on boot");
    let first = recovered
        .load(&session.metadata.id)
        .expect("load recovered");
    assert_eq!(first.messages.len(), 3);
    assert_eq!(first.metadata.message_count, 3);
    assert!(first.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error: Some(true),
                    ..
                } if tool_use_id == "call-crashed"
                    && content.contains("crashed_and_repaired")
            )
        })
    }));

    let reopened = SessionStore::boot_for_process_startup().expect("recover twice");
    let second = reopened.load(&session.metadata.id).expect("load twice");
    assert_eq!(second.messages, first.messages);
    assert_eq!(second.metadata.message_count, 3);
}

#[test]
fn transcript_cas_rejects_stale_revision_without_overwrite() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let stale = transcript_revision(&session.messages).expect("empty revision");
    let winner = vec![user_text("winner")];
    // first commit 成功并返回新 revision、落盘生效
    // (原 transcript_cas_commits_and_returns_content_revision 的断言)。
    let committed = store
        .compare_and_swap_messages(&session.metadata.id, &stale, winner.clone())
        .expect("first commit");
    assert_eq!(
        committed,
        transcript_revision(&winner).expect("winner revision")
    );

    let error = store
        .compare_and_swap_messages(
            &session.metadata.id,
            &stale,
            vec![user_text("stale overwrite")],
        )
        .expect_err("stale CAS must fail");

    assert!(format!("{error:#}").contains("session_revision_conflict"));
    assert_eq!(
        store.load(&session.metadata.id).expect("load").messages,
        winner
    );
}

#[test]
fn metadata_and_artifacts_do_not_change_transcript_revision() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let messages = vec![user_text("stable transcript")];
    store
        .update_messages(&session.metadata.id, messages.clone())
        .expect("seed transcript");
    let before = transcript_revision(&store.load(&session.metadata.id).unwrap().messages)
        .expect("revision before metadata edits");

    store
        .set_title(&session.metadata.id, "renamed".to_string())
        .expect("rename");
    store
        .update_artifacts(
            &session.metadata.id,
            vec![
                std::env::temp_dir()
                    .join("transcript-revision-artifact.txt")
                    .to_string_lossy()
                    .into_owned(),
            ],
        )
        .expect("update artifacts");

    let after = transcript_revision(&store.load(&session.metadata.id).unwrap().messages)
        .expect("revision after metadata edits");
    assert_eq!(before, after);
    assert_eq!(
        store.load(&session.metadata.id).expect("load").messages,
        messages
    );
}

#[test]
fn concurrent_stale_transcript_write_cannot_overwrite_winner() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let expected = transcript_revision(&session.messages).expect("empty revision");
    let barrier = Arc::new(std::sync::Barrier::new(2));

    let mut handles = Vec::new();
    for text in ["writer one", "writer two"] {
        let thread_store = store.clone();
        let thread_id = session.metadata.id.clone();
        let thread_expected = expected.clone();
        let thread_barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            thread_barrier.wait();
            thread_store.compare_and_swap_messages(
                &thread_id,
                &thread_expected,
                vec![user_text(text)],
            )
        }));
    }

    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread"))
        .collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(outcomes.iter().filter(|result| result.is_err()).count(), 1);

    let durable = store.load(&session.metadata.id).expect("load winner");
    let durable_revision = transcript_revision(&durable.messages).expect("durable revision");
    assert!(
        outcomes
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .any(|revision| revision == &durable_revision)
    );
}

#[test]
fn delete_removes_session() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    store.delete(&s.metadata.id).expect("delete");
    assert!(store.load(&s.metadata.id).is_err(), "load after delete");
}

#[test]
fn delete_notifies_hook_when_record_commit_precedes_cleanup_error() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id;
    let runtime_dir = store.manager.sessions_dir().join(&id);
    std::fs::create_dir_all(&runtime_dir).expect("create runtime dir");
    std::fs::write(runtime_dir.join("pending.txt"), "pending cleanup")
        .expect("write runtime marker");
    let deletions = record_session_deletions(&store);
    let purged = Arc::new(std::sync::Mutex::new(Vec::new()));
    let purge_recorder = Arc::clone(&purged);
    store.register_session_purged_hook(Arc::new(move |session_id| {
        purge_recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(session_id.to_string());
    }));
    store
        .inject_post_record_delete_fault(&id, ErrorKind::PermissionDenied)
        .expect("inject post-record cleanup error");

    let error = store
        .delete(&id)
        .expect_err("cleanup error must remain visible to caller");

    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .expect("original io cleanup error")
            .kind(),
        ErrorKind::PermissionDenied
    );
    assert!(
        !store
            .manager
            .sessions_dir()
            .join(format!("{id}.json"))
            .exists(),
        "durable session record was committed as deleted"
    );
    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        std::slice::from_ref(&id)
    );
    assert_eq!(
        purged
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        std::slice::from_ref(&id),
        "a committed ordinary deletion purges process state even when directory cleanup fails"
    );
    assert!(runtime_dir.exists(), "failed cleanup remains retryable");

    store
        .delete(&id)
        .expect("idempotent retry finishes remaining cleanup");
    assert!(!runtime_dir.exists());
}

#[test]
fn deletion_hooks_are_runtime_only_and_do_not_retain_process_history() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id;

    store.delete(&id).expect("delete before hook exists");

    let deletions = record_session_deletions(&store);
    assert!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
    store
        .delete(&id)
        .expect("idempotent runtime delete notifies the registered hook");
    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[id]
    );
}

#[test]
fn repeated_delete_emits_idempotent_runtime_wakeups() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id;
    let deletions = record_session_deletions(&store);

    store.delete(&id).expect("first delete");
    store.delete(&id).expect("idempotent delete");

    assert_eq!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        &[id.clone(), id]
    );
}

#[test]
fn deletion_hook_invocation_does_not_hold_the_registry_lock() {
    let (store, _g) = isolated_store();
    let nested_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let registered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let store_for_hook = store.clone();
    let registered_for_hook = Arc::clone(&registered);
    let nested_calls_for_hook = Arc::clone(&nested_calls);
    store.register_session_deleted_hook(Arc::new(move |_| {
        if !registered_for_hook.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let nested_calls = Arc::clone(&nested_calls_for_hook);
            store_for_hook.register_session_deleted_hook(Arc::new(move |_| {
                nested_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }));
        }
    }));

    store.notify_session_deleted("first-runtime-deletion");
    assert_eq!(nested_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    store.notify_session_deleted("second-runtime-deletion");
    assert_eq!(nested_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn invalid_precommit_delete_does_not_emit_durable_deletion_hook() {
    let (store, _guard) = isolated_store();
    let deletions = record_session_deletions(&store);

    let (committed, result) = store.delete_session_record("");

    assert!(!committed);
    assert!(result.is_err());
    assert!(
        deletions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
}

#[test]
fn delete_active_clears_active_id() {
    let (store, _g) = isolated_store();
    // set_active/active_id 追踪语义(原 active_id_tracks_set_active 的断言):
    // 初始 None → set Some 后可读回 → set None 复位。
    assert!(store.active_id().is_none());
    store.set_active(Some("abc".into()));
    assert_eq!(store.active_id().as_deref(), Some("abc"));
    store.set_active(None);
    assert!(store.active_id().is_none());

    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    store.set_active(Some(s.metadata.id.clone()));
    store.delete(&s.metadata.id).expect("delete");
    assert!(store.active_id().is_none(), "delete active clears tracker");
}

#[test]
fn delete_missing_session_file_is_idempotent() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let session_file = store
        .manager
        .sessions_dir()
        .join(format!("{}.json", s.metadata.id));
    let session_dir = store.manager.sessions_dir().join(&s.metadata.id);
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::remove_file(&session_file).expect("remove session file");
    store.set_active(Some(s.metadata.id.clone()));
    store.set_pinned(&s.metadata.id, true);

    store.delete(&s.metadata.id).expect("delete missing file");

    assert!(!session_dir.exists(), "stale session dir removed");
    assert!(store.active_id().is_none(), "active tracker cleared");
    assert!(!store.is_pinned(&s.metadata.id), "pinned state cleared");

    store
        .delete(&s.metadata.id)
        .expect("repeated delete remains successful");
}

#[test]
fn pinned_sessions_persist_and_delete_cleans() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");

    store.set_pinned(&s.metadata.id, true);
    assert!(store.is_pinned(&s.metadata.id));
    assert!(
        store.pinned_at(&s.metadata.id).is_some(),
        "pinning records pinned_at"
    );

    let reloaded = SessionStore::boot().expect("reboot");
    reloaded.load_pinned_sessions();
    assert!(reloaded.is_pinned(&s.metadata.id));
    assert!(
        reloaded.pinned_at(&s.metadata.id).is_some(),
        "pinned_at survives reload"
    );

    reloaded.delete(&s.metadata.id).expect("delete");
    assert!(!reloaded.is_pinned(&s.metadata.id));
    assert!(reloaded.pinned_at(&s.metadata.id).is_none());
}

#[test]
fn pinned_sessions_loads_legacy_id_array() {
    let (_store, _g) = isolated_store();
    let file = crate::platform::paths::sessions_root().join("_pinned_sessions.json");
    std::fs::create_dir_all(crate::platform::paths::sessions_root()).expect("mkdir");
    std::fs::write(&file, r#"["legacy-session"]"#).expect("write legacy pins");

    let reloaded = SessionStore::boot().expect("reboot");
    reloaded.load_pinned_sessions();
    assert!(reloaded.is_pinned("legacy-session"));
    assert!(
        reloaded.pinned_at("legacy-session").is_some(),
        "legacy pins receive a migration timestamp"
    );
}

#[test]
fn hidden_sessions_persist_restore_and_delete_cleans() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");

    store.set_hidden(&s.metadata.id, true);
    assert!(store.is_hidden(&s.metadata.id));
    assert!(
        store.hidden_at(&s.metadata.id).is_some(),
        "hiding records hidden_at"
    );

    let reloaded = SessionStore::boot().expect("reboot");
    reloaded.load_hidden_sessions();
    assert!(reloaded.is_hidden(&s.metadata.id));
    assert!(
        reloaded.hidden_at(&s.metadata.id).is_some(),
        "hidden_at survives reload"
    );

    reloaded.set_hidden(&s.metadata.id, false);
    assert!(!reloaded.is_hidden(&s.metadata.id));
    assert!(reloaded.hidden_at(&s.metadata.id).is_none());

    reloaded.set_hidden(&s.metadata.id, true);
    reloaded.delete(&s.metadata.id).expect("delete");
    assert!(!reloaded.is_hidden(&s.metadata.id));
    assert!(reloaded.hidden_at(&s.metadata.id).is_none());
}

#[test]
fn hidden_sessions_loads_legacy_id_array() {
    let (_store, _g) = isolated_store();
    let file = crate::platform::paths::sessions_root().join("_hidden_sessions.json");
    std::fs::create_dir_all(crate::platform::paths::sessions_root()).expect("mkdir");
    std::fs::write(&file, r#"["legacy-hidden-session"]"#).expect("write legacy hidden");

    let reloaded = SessionStore::boot().expect("reboot");
    reloaded.load_hidden_sessions();
    assert!(reloaded.is_hidden("legacy-hidden-session"));
    assert!(
        reloaded.hidden_at("legacy-hidden-session").is_some(),
        "legacy hidden sessions receive a migration timestamp"
    );
}

#[test]
fn hiding_session_clears_pinned_state() {
    let (store, _g) = isolated_store();
    let s = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");

    store.set_pinned(&s.metadata.id, true);
    assert!(store.is_pinned(&s.metadata.id));

    store.set_hidden(&s.metadata.id, true);
    assert!(store.is_hidden(&s.metadata.id));
    assert!(!store.is_pinned(&s.metadata.id));
    assert!(store.pinned_at(&s.metadata.id).is_none());

    let reloaded = SessionStore::boot().expect("reboot");
    reloaded.load_pinned_sessions();
    reloaded.load_hidden_sessions();
    assert!(reloaded.is_hidden(&s.metadata.id));
    assert!(!reloaded.is_pinned(&s.metadata.id));
}

#[test]
fn generate_session_id_url_safe() {
    let id = generate_session_id();
    assert!(
        id.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    );
}

#[test]
fn pending_plan_ticket_is_compare_and_consumed_with_failure_restore() {
    let (store, _g) = isolated_store();
    let sid = "plan-ticket-session";
    store
        .set_mode(sid, SerializableMode::Plan)
        .expect("enter plan");
    let registered = store
        .register_pending_plan(sid, "plan-1".to_string())
        .expect("register plan");
    assert_eq!(registered.pending_plan_id.as_deref(), Some("plan-1"));
    assert!(store.claim_pending_plan(sid, "stale-plan").is_err());

    let claim = store
        .claim_pending_plan(sid, "plan-1")
        .expect("claim current plan");
    assert_eq!(claim.accepted_state().mode, SerializableMode::Yolo);
    assert!(claim.accepted_state().pending_plan_id.is_none());
    assert!(store.claim_pending_plan(sid, "plan-1").is_err());
    drop(claim);
    let restored = store.mode_state(sid);
    assert_eq!(restored.mode, SerializableMode::Plan);
    assert_eq!(restored.pending_plan_id.as_deref(), Some("plan-1"));

    store
        .claim_pending_plan(sid, "plan-1")
        .expect("reclaim current plan")
        .commit();
    let committed = store.mode_state(sid);
    assert_eq!(committed.mode, SerializableMode::Yolo);
    assert!(committed.pending_plan_id.is_none());
    assert!(store.claim_pending_plan(sid, "plan-1").is_err());

    store
        .set_mode(sid, SerializableMode::Plan)
        .expect("re-enter plan");
    store
        .register_pending_plan(sid, "plan-2".to_string())
        .expect("register newer plan");
    assert!(store.discard_pending_plan(sid, "plan-1").is_err());
    let discarded = store
        .discard_pending_plan(sid, "plan-2")
        .expect("discard current plan");
    assert_eq!(discarded.mode, SerializableMode::Plan);
    assert!(discarded.pending_plan_id.is_none());
    assert!(store.discard_pending_plan(sid, "plan-2").is_err());
}

/// 模式切换闭环(回归底座二态后的核心契约):流转命令 set_plan_mode_next(→Plan) /
/// accept_plan / exit_plan_to_yolo(→Yolo) 实质都只调 set_mode,全程**只动 mode**——
/// 待注入人格 body / 挂载知识集 / 人格卡等正交状态必须原样保留。
/// (discard_plan「算了」不在此列:放弃方案但留在当前 mode,不调 set_mode。)
/// 防有人给流转命令加副作用,或把 set_mode 改成整体覆盖式写法时连带清掉这些字段。
#[test]
fn mode_switch_loop_preserves_orthogonal_state() {
    use SerializableMode;
    let (store, _g) = isolated_store();
    let sid = "s-loop";

    // 起始默认 Yolo,挂满正交状态
    assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
    store.set_pending_persona_body(sid, Some("PENDING BODY".into()));
    store.set_mounted_collection(sid, Some(42));
    store.set_active_persona(sid, Some("expert-x".into()));

    // 闭环往返两轮:Yolo →(set_plan_mode_next)→ Plan →(accept/exit)→ Yolo
    for _ in 0..2 {
        store
            .set_mode(sid, SerializableMode::Plan)
            .expect("set chat plan mode");
        assert_eq!(store.mode_state(sid).mode, SerializableMode::Plan);
        store
            .set_mode(sid, SerializableMode::Yolo)
            .expect("set chat yolo mode");
        assert_eq!(store.mode_state(sid).mode, SerializableMode::Yolo);
    }

    // 三个正交字段全保留
    let st = store.mode_state(sid);
    assert_eq!(
        st.pending_persona_body.as_deref(),
        Some("PENDING BODY"),
        "切 mode 清了待注入人格 body"
    );
    assert_eq!(st.mounted_collection, Some(42), "切 mode 卸载了知识集");
    assert_eq!(
        st.active_persona.as_deref(),
        Some("expert-x"),
        "切 mode 清了人格"
    );
}

#[test]
fn pending_turn_injections_restore_on_drop_and_commit_only_after_submission() {
    let (store, _g) = isolated_store();
    store.set_active_persona("s1", Some("persona-a".into()));
    store.set_pending_persona_body("s1", Some("PERSONA BODY".into()));

    {
        let pending = store.take_pending_turn_injections("s1");
        assert_eq!(pending.persona_body(), Some("PERSONA BODY"));
        assert!(store.mode_state("s1").pending_persona_body.is_none());
        // Simulate attachment/build/Engine submission failure.
    }
    assert_eq!(
        store.mode_state("s1").pending_persona_body.as_deref(),
        Some("PERSONA BODY")
    );

    store.set_pending_persona_body("s1", Some("SECOND PERSONA".into()));
    store.take_pending_turn_injections("s1").commit();
    assert!(store.mode_state("s1").pending_persona_body.is_none());
}

#[test]
fn mounted_collections_are_ordered_deduplicated_and_legacy_compatible() {
    let (store, _g) = isolated_store();
    let sid = "s-multi-kb";
    store.set_mounted_collections(
        sid,
        vec![
            MountedCollection {
                collection_id: 7,
                enabled: true,
            },
            MountedCollection {
                collection_id: 7,
                enabled: false,
            },
            MountedCollection {
                collection_id: 8,
                enabled: false,
            },
            MountedCollection {
                collection_id: -1,
                enabled: true,
            },
        ],
    );
    assert_eq!(
        store.mounted_collections(sid),
        vec![
            MountedCollection {
                collection_id: 7,
                enabled: true,
            },
            MountedCollection {
                collection_id: 8,
                enabled: false,
            },
        ]
    );
    assert_eq!(store.mounted_collection_ids(sid), vec![7]);
    assert_eq!(store.mounted_collection(sid), Some(7));

    store.set_mounted_collection(sid, Some(42));
    assert_eq!(
        store.mounted_collections(sid),
        vec![MountedCollection {
            collection_id: 42,
            enabled: true,
        }]
    );
}

#[test]
fn remote_and_local_collections_can_be_mounted_together() {
    let (store, _g) = isolated_store();
    let sid = "s-mixed-kb";
    store.set_mounted_collection(sid, Some(7));
    store.add_mounted_remote_collection(sid, "cube".to_string(), 7);
    store.add_mounted_remote_collection(sid, "cube".to_string(), 7);
    store.add_mounted_remote_collection(sid, "other".to_string(), 7);
    assert_eq!(store.mounted_collection_ids(sid), vec![7]);
    assert_eq!(
        store.mounted_remote_collections(sid),
        vec![
            MountedRemoteCollection {
                server_id: "cube".to_string(),
                collection_id: 7,
                enabled: true,
            },
            MountedRemoteCollection {
                server_id: "other".to_string(),
                collection_id: 7,
                enabled: true,
            },
        ]
    );
    store.set_mounted_remote_collection_enabled(sid, "cube", 7, false);
    assert!(!store.mounted_remote_collections(sid)[0].enabled);
    let changed = store.remove_remote_server_mounts("cube");
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, sid);
    assert_eq!(store.mounted_remote_collections(sid).len(), 1);
}

#[test]
fn disconnecting_remote_server_removes_its_mounts_from_every_affected_session() {
    let (store, _g) = isolated_store();
    store.set_mounted_collection("session-a", Some(7));
    store.add_mounted_remote_collection("session-a", "cube".to_string(), 7);
    store.add_mounted_remote_collection("session-a", "cube".to_string(), 8);
    store.add_mounted_remote_collection("session-a", "other".to_string(), 7);
    store.add_mounted_remote_collection("session-b", "cube".to_string(), 9);
    store.add_mounted_remote_collection("session-unaffected", "other".to_string(), 10);

    let changed = store.remove_remote_server_mounts("cube");

    assert_eq!(
        changed
            .iter()
            .map(|(session_id, _)| session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session-a", "session-b"]
    );
    assert_eq!(
        changed[0].1,
        vec![MountedRemoteCollection {
            server_id: "other".to_string(),
            collection_id: 7,
            enabled: true,
        }],
        "events must receive the authoritative post-disconnect mount list"
    );
    assert!(store.mounted_remote_collections("session-b").is_empty());
    assert_eq!(
        store.mounted_remote_collections("session-unaffected"),
        vec![MountedRemoteCollection {
            server_id: "other".to_string(),
            collection_id: 10,
            enabled: true,
        }]
    );
    assert_eq!(
        store.mounted_collection("session-a"),
        Some(7),
        "disconnecting a remote server must not disturb local mounts"
    );
}

#[test]
fn mounted_collection_item_updates_merge_across_concurrent_clients() {
    let (store, _g) = isolated_store();
    let sid = "s-concurrent-multi-kb";
    store.set_mounted_collection(sid, Some(7));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

    let add_store = store.clone();
    let add_barrier = barrier.clone();
    let add = std::thread::spawn(move || {
        add_barrier.wait();
        add_store.add_mounted_collection(sid, 8);
    });
    let disable_store = store.clone();
    let disable_barrier = barrier.clone();
    let disable = std::thread::spawn(move || {
        disable_barrier.wait();
        disable_store.set_mounted_collection_enabled(sid, 7, false);
    });
    barrier.wait();
    add.join().unwrap();
    disable.join().unwrap();

    assert_eq!(
        store.mounted_collections(sid),
        vec![
            MountedCollection {
                collection_id: 7,
                enabled: false,
            },
            MountedCollection {
                collection_id: 8,
                enabled: true,
            },
        ],
    );
    assert_eq!(store.mounted_collection(sid), Some(8));
}

#[test]
fn deleting_collection_removes_mount_from_every_affected_session() {
    let (store, _g) = isolated_store();
    store.set_mounted_collections(
        "session-a",
        vec![
            MountedCollection {
                collection_id: 7,
                enabled: true,
            },
            MountedCollection {
                collection_id: 8,
                enabled: false,
            },
        ],
    );
    store.set_mounted_collections(
        "session-b",
        vec![
            MountedCollection {
                collection_id: 9,
                enabled: true,
            },
            MountedCollection {
                collection_id: 7,
                enabled: false,
            },
        ],
    );
    store.set_mounted_collection("session-legacy", Some(7));
    store.set_mounted_collection("session-unaffected", Some(9));
    let unaffected_revision = store
        .mounted_collections_snapshot("session-unaffected")
        .revision;

    let changed = store.remove_mounted_collection_from_all(7);

    assert_eq!(
        changed
            .iter()
            .map(|(session_id, _)| session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session-a", "session-b", "session-legacy"]
    );
    assert_eq!(
        store.mounted_collections("session-a"),
        vec![MountedCollection {
            collection_id: 8,
            enabled: false,
        }]
    );
    assert_eq!(
        store.mounted_collections("session-b"),
        vec![MountedCollection {
            collection_id: 9,
            enabled: true,
        }]
    );
    assert!(store.mounted_collections("session-legacy").is_empty());
    assert_eq!(
        store
            .mounted_collections_snapshot("session-unaffected")
            .revision,
        unaffected_revision,
        "unaffected sessions must not receive a spurious revision"
    );
}

#[test]
fn deleting_remote_collection_removes_only_the_exact_mount_from_every_session() {
    let (store, _g) = isolated_store();
    store.set_mounted_collection("session-a", Some(7));
    store.add_mounted_remote_collection("session-a", "cube".to_string(), 7);
    store.add_mounted_remote_collection("session-a", "cube".to_string(), 8);
    store.add_mounted_remote_collection("session-a", "other".to_string(), 7);
    store.add_mounted_remote_collection("session-b", "cube".to_string(), 7);
    store.set_mounted_remote_collection_enabled("session-b", "cube", 7, false);
    store.add_mounted_remote_collection("session-unaffected", "cube".to_string(), 9);

    let changed = store.remove_mounted_remote_collection_from_all("cube", 7);

    assert_eq!(
        changed
            .iter()
            .map(|(session_id, _)| session_id.as_str())
            .collect::<Vec<_>>(),
        vec!["session-a", "session-b"]
    );
    assert_eq!(
        store.mounted_remote_collections("session-a"),
        vec![
            MountedRemoteCollection {
                server_id: "cube".to_string(),
                collection_id: 8,
                enabled: true,
            },
            MountedRemoteCollection {
                server_id: "other".to_string(),
                collection_id: 7,
                enabled: true,
            },
        ]
    );
    assert!(store.mounted_remote_collections("session-b").is_empty());
    assert_eq!(
        store.mounted_remote_collections("session-unaffected"),
        vec![MountedRemoteCollection {
            server_id: "cube".to_string(),
            collection_id: 9,
            enabled: true,
        }]
    );
    assert_eq!(
        store.mounted_collection("session-a"),
        Some(7),
        "remote deletion must not disturb local mounts with the same numeric id"
    );
}

// ============================================================================
// 回迁的回归测试：wave2 拆分时从 god-module `mod tests` 丢失的 17 个用例。
// 覆盖 #162（multi-agent 标志持久化/幽灵清理/写盘收敛）、#190（code 会话
// 双层持久化/默认值解析）、#263（三分 lane 默认与 plan-claim 语义）。
// 逐字节取自拆分前基线，未做语义改动。
// ============================================================================

/// 开关持久化的真实行为回归：落盘 → 新 store 恢复 → 删除/清理同步。
/// （复核指出旧测试只 grep 源码有没有调用，不覆盖真实重启与清理路径。）
#[test]
fn multi_agent_flags_survive_restart_and_follow_deletion() {
    let (store, _guard) = isolated_store();
    let chat = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create chat");
    let id = chat.metadata.id.clone();

    store.set_multi_agent(&id, true).expect("persist flag");
    let file = paths::sessions_root().join("_multi_agent.json");
    assert!(file.is_file(), "开关必须落盘");
    assert!(
        std::fs::read_to_string(&file).unwrap().contains(&id),
        "落盘清单必须包含该会话"
    );

    // "重启"：同一磁盘上重建 store → 开关恢复
    let reloaded = SessionStore::boot_with_scheduled_root(paths::scheduled_tasks_root())
        .expect("reboot store");
    assert!(
        reloaded.mode_state(&id).multi_agent,
        "重启后开关必须恢复（Web 门禁与每轮注入都依据它）"
    );

    // 关闭 → 清单收敛为空 → 文件删除（不留空壳）
    store.set_multi_agent(&id, false).expect("persist off");
    assert!(!file.exists(), "空清单必须删除 sidecar 文件");

    // 再开 → 删除会话 → 清单同步移除
    store
        .set_multi_agent(&id, true)
        .expect("persist flag again");
    store.delete(&id).expect("delete session");
    assert!(
        !file.exists(),
        "删除会话必须同步清掉 _multi_agent.json 条目"
    );
}

/// 删除路径侧车更新失败留下的幽灵 id，必须在下次启动被对账剔除，
/// 且清单当场重写（不再传染后续启动）。
#[test]
fn ghost_ids_are_reconciled_away_on_load() {
    let (store, _guard) = isolated_store();
    let chat = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create chat");
    let real = chat.metadata.id.clone();
    store.set_multi_agent(&real, true).expect("persist flag");

    // 伪造一条幽灵记录（会话 JSON 不存在）
    let file = paths::sessions_root().join("_multi_agent.json");
    let mut ids: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    ids.push("ghost-session".into());
    std::fs::write(&file, serde_json::to_string_pretty(&ids).unwrap()).unwrap();

    let reloaded = SessionStore::boot_with_scheduled_root(paths::scheduled_tasks_root())
        .expect("reboot store");
    assert!(reloaded.mode_state(&real).multi_agent, "真实会话恢复");
    assert!(
        !reloaded.mode_state("ghost-session").multi_agent,
        "幽灵 id 不得恢复开关"
    );
    let rewritten = std::fs::read_to_string(&file).unwrap();
    assert!(
        !rewritten.contains("ghost-session"),
        "清单必须当场重写剔除幽灵 id: {rewritten}"
    );
}

/// 并发「开启/关闭」交错后，落盘结果必须收敛到最终内存状态——保存的
/// 快照与写盘在同一临界区内，旧快照不可能覆盖新快照。
#[test]
fn concurrent_flag_saves_converge_to_final_memory_state() {
    let (store, _guard) = isolated_store();
    let a = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create a")
        .metadata
        .id
        .clone();
    let b = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create b")
        .metadata
        .id
        .clone();

    let threads: Vec<_> = [(a.clone(), true), (b.clone(), true)]
        .into_iter()
        .map(|(id, on)| {
            let store = store.clone();
            std::thread::spawn(move || store.set_multi_agent(&id, on).expect("persist"))
        })
        .collect();
    for t in threads {
        t.join().expect("join");
    }

    let file = paths::sessions_root().join("_multi_agent.json");
    let listed: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(
        listed.contains(&a) && listed.contains(&b),
        "并发保存不得互相丢会话: {listed:?}"
    );
}

/// 保留策略的自动清理同样要移出开关清单：残留幽灵 id 会在重启后复活
/// 开关状态，专家池变更联动还会给它重建工作区。
#[test]
fn retention_purge_also_updates_multi_agent_flags() {
    let (store, _guard) = isolated_store();
    let chat = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create chat");
    let id = chat.metadata.id.clone();
    store.set_multi_agent(&id, true).expect("persist flag");
    let file = paths::sessions_root().join("_multi_agent.json");
    assert!(file.is_file());

    store.purge_session_side_maps(std::slice::from_ref(&id));

    assert!(!store.mode_state(&id).multi_agent, "内存状态已清");
    assert!(
        !file.exists(),
        "自动清理后 _multi_agent.json 不得残留幽灵 id"
    );
}

#[test]
fn retention_purge_notifies_session_purged_hooks() {
    let (store, _guard) = isolated_store();
    let chat = store
        .create_new("m".into(), None, std::env::temp_dir())
        .expect("create chat");
    let id = chat.metadata.id.clone();

    // Dependency inversion: deep retention-policy deletions inside the
    // sessions feature notify process-level state holders via the hook
    // (timing/pending_user_input registered by the composition root); with
    // nobody registered, deletion proceeds as usual.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    store.register_session_purged_hook(std::sync::Arc::new(move |sid: &str| {
        recorder.lock().unwrap().push(sid.to_string());
    }));

    store.purge_session_side_maps(std::slice::from_ref(&id));

    let notified = seen.lock().unwrap().clone();
    assert_eq!(
        notified,
        vec![id],
        "the purge hook must receive the deleted session id"
    );
}

// ===================== code 会话权限模式（两层持久化 + 默认值解析）=====================

/// 注入一个简易 code 会话判定：列表内的 id 视为品悟原生 code 会话。
fn with_code_sessions(store: &SessionStore, ids: &[&str]) {
    let owned: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    store.set_code_session_predicate(Arc::new(move |id: &str| {
        owned.iter().any(|candidate| candidate == id)
    }));
}

#[test]
fn code_session_first_use_defaults_to_plan() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1"]);
    // 从未用过 code 模式（无 per-session 记录、全局 last_mode=None）→ Plan 只读。
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Plan);
    // plain 会话维持 Yolo 现状。
    assert_eq!(store.mode_state("plain-1").mode, SerializableMode::Yolo);
}

/// 谓词未注入时（启动早期/测试）全部按 plain 语义，不误判。拆成独立测试：
/// `isolated_store` 持有进程级 `ENV_LOCK` 直到 guard drop，同一线程内二次调用
/// 会自死锁（`std::sync::Mutex` 不可重入）。每测试只调一次 `isolated_store`。
#[test]
fn code_session_without_predicate_defaults_to_yolo() {
    let (no_predicate, _g) = isolated_store();
    assert_eq!(
        no_predicate.mode_state("code-1").mode,
        SerializableMode::Yolo
    );
}

#[test]
fn code_session_default_follows_code_lane_default() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1", "code-2"]);
    // 已生成会话显式切 yolo：只写 per-session 记录，不碰全局 lane 默认
    // （三分 lane 语义）→ 新 code 会话默认不跟随。
    store
        .set_mode("code-1", SerializableMode::Yolo)
        .expect("switch yolo");
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Yolo);
    assert_eq!(store.mode_state("code-2").mode, SerializableMode::Plan);
    assert!(store.code_permission_prefs().last_mode.is_none());
    // 草稿态写 code lane 全局默认 → 新 code 会话默认跟随；已有会话不受影响。
    store.set_mode_default(ModeLane::Code, SerializableMode::Yolo);
    assert_eq!(store.mode_state("code-2").mode, SerializableMode::Yolo);
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Yolo);
    store.set_mode_default(ModeLane::Code, SerializableMode::Plan);
    assert_eq!(store.mode_state("code-2").mode, SerializableMode::Plan);
}

#[test]
fn code_mode_persists_per_session_across_restart() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1", "code-2", "code-3"]);
    store
        .set_mode("code-1", SerializableMode::Yolo)
        .expect("code-1 yolo");
    store
        .set_mode("code-2", SerializableMode::Plan)
        .expect("code-2 plan");
    // sidecar 只存 code 会话的显式 mode。
    let file = paths::sessions_root().join("_session_mode_states.json");
    let on_disk: HashMap<String, SerializableMode> =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("read sidecar"))
            .expect("parse sidecar");
    assert_eq!(on_disk.len(), 2);
    assert_eq!(on_disk.get("code-1"), Some(&SerializableMode::Yolo));
    assert_eq!(on_disk.get("code-2"), Some(&SerializableMode::Plan));

    // 重启：per-session 恢复各自上次的 mode（code-1 的 yolo 不被全局
    // last_mode=plan 盖掉），新 code 会话回落全局默认。
    let reopened = reopen_store(&store).expect("reboot");
    with_code_sessions(&reopened, &["code-1", "code-2", "code-3"]);
    assert_eq!(reopened.mode_state("code-1").mode, SerializableMode::Yolo);
    assert_eq!(reopened.mode_state("code-2").mode, SerializableMode::Plan);
    assert_eq!(reopened.mode_state("code-3").mode, SerializableMode::Plan);

    // 删除会话清理 per-session 持久化条目。
    reopened.delete("code-1").expect("delete code-1");
    let on_disk: HashMap<String, SerializableMode> =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("read sidecar"))
            .expect("parse sidecar");
    assert!(!on_disk.contains_key("code-1"));
    assert_eq!(on_disk.get("code-2"), Some(&SerializableMode::Plan));
}

/// accept 方案确认提交（commit）后，会话的 Yolo 纳入 per-session 持久化；
/// 不碰任何全局 lane 默认（三分 lane 语义）；提交失败回滚（rollback）不落盘，
/// 内存回 Plan 与磁盘保持一致。
#[test]
fn code_session_accepted_yolo_persists_on_commit_not_rollback() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1", "code-2"]);
    // 从未显式切过 → 首次默认 Plan。
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Plan);
    store
        .register_pending_plan("code-1", "plan-1".to_string())
        .expect("register plan");

    // 回滚（未 commit 就 drop）：内存回 Plan，磁盘不写（last_mode 仍 None）。
    let claim = store
        .claim_pending_plan("code-1", "plan-1")
        .expect("claim plan-1");
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Yolo);
    drop(claim);
    assert_eq!(store.mode_state("code-1").mode, SerializableMode::Plan);
    assert!(store.code_permission_prefs().last_mode.is_none());

    // 提交：重新 claim + commit → per-session 持久化；全局 lane 默认不动。
    store
        .register_pending_plan("code-1", "plan-2".to_string())
        .expect("register plan-2");
    store
        .claim_pending_plan("code-1", "plan-2")
        .expect("claim plan-2")
        .commit();
    assert!(store.code_permission_prefs().last_mode.is_none());

    // 重启：per-session 恢复 Yolo；新 code 会话回落全局默认（未动 → Plan）。
    let reopened = reopen_store(&store).expect("reboot");
    with_code_sessions(&reopened, &["code-1", "code-2"]);
    assert_eq!(reopened.mode_state("code-1").mode, SerializableMode::Yolo);
    assert_eq!(reopened.mode_state("code-2").mode, SerializableMode::Plan);
}

#[test]
fn plain_session_mode_persists_across_restart() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1"]);
    store
        .set_mode("plain-1", SerializableMode::Plan)
        .expect("plain plan");
    assert_eq!(store.mode_state("plain-1").mode, SerializableMode::Plan);
    // 三分 lane 语义：plain 会话也写 sidecar；但不动任何全局 lane 默认。
    let file = paths::sessions_root().join("_session_mode_states.json");
    let on_disk: HashMap<String, SerializableMode> =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("read sidecar"))
            .expect("parse sidecar");
    assert_eq!(on_disk.get("plain-1"), Some(&SerializableMode::Plan));
    assert!(store.code_permission_prefs().last_mode.is_none());
    assert_eq!(store.mode_defaults().work, None);
    assert_eq!(store.mode_defaults().design, None);
    // 重启后 plain 会话恢复自己的 Plan（语义 3：每个对话保存自己的 mode）。
    let reopened = reopen_store(&store).expect("reboot");
    assert_eq!(reopened.mode_state("plain-1").mode, SerializableMode::Plan);
}

/// work/design lane 全局默认的读写与持久化；非法 lane 字符串必须报错。
#[test]
fn mode_lane_defaults_round_trip_and_validate() {
    let (store, _g) = isolated_store();
    assert_eq!(store.mode_defaults().work, None);
    store.set_mode_default(ModeLane::Work, SerializableMode::Plan);
    store.set_mode_default(ModeLane::Design, SerializableMode::Yolo);
    assert_eq!(store.mode_defaults().work, Some(SerializableMode::Plan));
    assert_eq!(store.mode_defaults().design, Some(SerializableMode::Yolo));
    assert_eq!(store.mode_defaults().code, None);
    // 落盘 settings.json + 重启后镜像恢复。
    assert_eq!(
        UserPrefs::load().mode_defaults.work,
        Some(SerializableMode::Plan)
    );
    let reopened = reopen_store(&store).expect("reboot");
    assert_eq!(reopened.mode_defaults().work, Some(SerializableMode::Plan));
    assert_eq!(
        reopened.mode_defaults().design,
        Some(SerializableMode::Yolo)
    );
    // lane 字符串校验（命令层入口防 IPC 直调写未知 lane）。
    assert!(ModeLane::parse("work").is_ok());
    assert!(ModeLane::parse("design").is_ok());
    assert!(ModeLane::parse("code").is_ok());
    assert!(ModeLane::parse(" CodE ").is_err());
    assert!(ModeLane::parse("").is_err());
}

/// 旧版 `_code_mode_states.json`（只含 code 会话的时代产物）在新文件缺失时
/// 回退加载，老用户的 per-session 记录不丢。
#[test]
fn legacy_code_mode_states_file_is_loaded_as_fallback() {
    let (store, _g) = isolated_store();
    let legacy = paths::sessions_root().join("_code_mode_states.json");
    std::fs::write(
        &legacy,
        serde_json::to_string(&HashMap::from([(
            "code-legacy".to_string(),
            SerializableMode::Yolo,
        )]))
        .expect("serialize legacy"),
    )
    .expect("write legacy sidecar");
    store.load_session_mode_states();
    assert_eq!(store.mode_state("code-legacy").mode, SerializableMode::Yolo);
    let _ = std::fs::remove_file(&legacy);
}

#[test]
fn confirm_code_yolo_persists_globally() {
    let (store, _g) = isolated_store();
    assert!(!store.code_permission_prefs().yolo_confirmed);
    let prefs = store.confirm_code_yolo().expect("confirm yolo");
    assert!(prefs.yolo_confirmed);
    assert!(store.code_permission_prefs().yolo_confirmed);
    // 落盘 settings.json；重启后内存镜像仍记得。
    assert!(UserPrefs::load().code_permission.yolo_confirmed);
    let reopened = reopen_store(&store).expect("reboot");
    assert!(reopened.code_permission_prefs().yolo_confirmed);
}

/// reconcile 只修正无持久化记录的 code 会话；显式切过的 mode 必须原样保留。
/// 拆成独立测试：`isolated_store` 持有进程级 ENV_LOCK 直到 guard drop，同一线程
/// 内二次调用会自死锁（`std::sync::Mutex` 不可重入），每测试只调一次。
#[test]
fn reconcile_does_not_overwrite_explicitly_persisted_mode() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-2"]);
    store
        .set_mode("code-2", SerializableMode::Yolo)
        .expect("code-2 explicit yolo");
    let reopened = reopen_store(&store).expect("reboot");
    with_code_sessions(&reopened, &["code-2"]);
    assert_eq!(
        reopened.mode_state("code-2").mode,
        SerializableMode::Yolo,
        "显式切过的 mode 不应被 reconcile 改写"
    );
}

#[test]
fn fresh_code_session_default_plan_registers_pending_plan() {
    let (store, _g) = isolated_store();
    with_code_sessions(&store, &["code-1"]);
    // 首次使用（默认值经解析得到 Plan、尚无内存条目）时出方案必须能登记，
    // 不能被 entry or_default 物化成 Yolo 而静默丢失 Plan 语义。
    let registered = store
        .register_pending_plan("code-1", "plan-1".to_string())
        .expect("register plan on fresh code session");
    assert_eq!(registered.mode, SerializableMode::Plan);
    assert_eq!(registered.pending_plan_id.as_deref(), Some("plan-1"));
}

/// 工作流运行的工作区由 run id 派生，不落在 sessions/ 下。
#[test]
fn session_model_update_rolls_back_memory_when_sidecar_write_fails() {
    let (store, _guard) = isolated_store();
    store
        .set_session_model_id("wf-model-test", Some("old-model".to_string()))
        .expect("persist initial model");
    let sidecar = paths::sessions_root().join("_session_models.json");
    std::fs::remove_file(&sidecar).expect("remove initial sidecar");
    std::fs::create_dir(&sidecar).expect("block sidecar path with a directory");

    let error = store
        .set_session_model_id("wf-model-test", Some("new-model".to_string()))
        .expect_err("an unwritable sidecar must fail the model transaction");

    assert!(
        error
            .to_string()
            .contains("persist per-session model bindings")
    );
    assert_eq!(
        store.session_model_override("wf-model-test").as_deref(),
        Some("old-model"),
        "failed persistence must not leave a memory-only model choice"
    );
}

// ===================== 代码模式回退：对话截断 + sidecar 备份（rewind.rs）=====================

fn tool_result_message(id: &str) -> Message {
    Message {
        role: "user".into(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: "tool output".into(),
            is_error: None,
            content_blocks: None,
        }],
    }
}

/// 读 `_rewound_turns.json` sidecar 中某会话的备份记录。
fn rewound_records(id: &str) -> Vec<super::rewind::RewoundTurnsRecord> {
    let path = paths::sessions_root().join("_rewound_turns.json");
    let bytes = std::fs::read(&path).expect("read rewound turns sidecar");
    let map: std::collections::HashMap<String, Vec<super::rewind::RewoundTurnsRecord>> =
        serde_json::from_slice(&bytes).expect("parse rewound turns sidecar");
    map.get(id).cloned().unwrap_or_default()
}

/// 定位口径：tool_result（同样 role="user"）不得被算作 turn 边界；截断点必须落在
/// 第 N+1 个真实用户 prompt 上，其前的 assistant/tool_result 全部保留。
#[test]
fn rewind_truncates_at_turn_boundary_with_interleaved_tool_results() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    let messages = vec![
        user_text("第一轮"),
        assistant_text("调工具"),
        tool_result_message("call_1"),
        assistant_text("答一"),
        user_text("第二轮"),
        assistant_text("答二"),
        tool_result_message("call_2"),
        user_text("第三轮"),
        assistant_text("答三"),
    ];
    store
        .update_messages(&id, messages.clone())
        .expect("seed transcript");
    let original_revision = transcript_revision(&messages).expect("revision");

    let outcome = store
        .truncate_to_user_turn(&id, 1, None)
        .expect("rewind to turn 1");

    assert_eq!(outcome.rewound_turns, 2);
    assert_eq!(outcome.removed_messages, 5);
    let kept = store.load(&id).expect("load").messages;
    assert_eq!(
        kept,
        messages[..4],
        "保留第 1 轮全部消息（含 tool_result 交错段）"
    );
    assert_eq!(
        outcome.new_revision,
        transcript_revision(&kept).expect("kept revision")
    );

    // sidecar 备份：截断时间、原 revision、被截消息齐全；记录截断后 revision
    // （undo 精确复核条件）与代码回滚点绑定（本次未传 → None）。
    let records = rewound_records(&id);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert!(!record.rewound_at.is_empty());
    assert_eq!(record.original_revision, original_revision);
    assert_eq!(record.kept_turns, 1);
    assert_eq!(record.truncated_revision, outcome.new_revision);
    assert_eq!(record.pre_restore_checkpoint_id, None);
    assert_eq!(record.removed_messages, messages[4..]);
}

/// N=0 = 回退到第一轮之前，transcript 全部截断。
#[test]
fn rewind_to_zero_turns_empties_transcript() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    store
        .update_messages(
            &id,
            vec![
                user_text("第一轮"),
                assistant_text("答一"),
                user_text("第二轮"),
                tool_result_message("call_1"),
            ],
        )
        .expect("seed transcript");

    let outcome = store
        .truncate_to_user_turn(&id, 0, None)
        .expect("rewind to zero");

    assert_eq!(outcome.rewound_turns, 2);
    assert_eq!(outcome.removed_messages, 4);
    assert!(store.load(&id).expect("load").messages.is_empty());
    let records = rewound_records(&id);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kept_turns, 0);
    assert_eq!(records[0].removed_messages.len(), 4);
}

/// N ≥ 当前 turn 数：如实报错，transcript 与 sidecar 都不动。
#[test]
fn rewind_out_of_range_errors_and_leaves_transcript_untouched() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    let messages = vec![user_text("第一轮"), assistant_text("答一")];
    store
        .update_messages(&id, messages.clone())
        .expect("seed transcript");

    // N == 当前 turn 数（无可截内容）与 N > 当前 turn 数都必须报错。
    assert!(store.truncate_to_user_turn(&id, 1, None).is_err());
    assert!(store.truncate_to_user_turn(&id, 7, None).is_err());
    assert_eq!(store.load(&id).expect("load").messages, messages);
    assert!(
        !paths::sessions_root().join("_rewound_turns.json").exists(),
        "失败的回退不得产生 sidecar"
    );
}

/// 守卫放行：回退是 looks_like_truncating_overwrite 的显式放行路径，截断本身不受
/// 拦；但同一守卫对 update_messages 等通用入口的保护不变。
#[test]
fn rewind_bypasses_guard_while_update_messages_stays_protected() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    let messages = vec![
        user_text("第一轮"),
        assistant_text("答一"),
        user_text("第二轮"),
        assistant_text("答二"),
    ];
    store
        .update_messages(&id, messages.clone())
        .expect("seed transcript");

    // 同样的断式覆盖走通用入口仍被守卫拦截。
    assert!(store.update_messages(&id, vec![]).is_err());
    // 回退专用路径放行（N=0 清空全部也允许）。
    store.truncate_to_user_turn(&id, 0, None).expect("rewind");
    assert!(store.load(&id).expect("load").messages.is_empty());
}

/// revision/CAS：截断后 revision 自然变化，持旧 revision 的 CAS 必须失败。
#[test]
fn stale_revision_cas_fails_after_rewind() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    let messages = vec![
        user_text("第一轮"),
        assistant_text("答一"),
        user_text("第二轮"),
        assistant_text("答二"),
    ];
    store
        .update_messages(&id, messages.clone())
        .expect("seed transcript");
    let stale_revision = transcript_revision(&messages).expect("revision");

    store.truncate_to_user_turn(&id, 1, None).expect("rewind");

    let error = store
        .compare_and_swap_messages(&id, &stale_revision, messages)
        .expect_err("stale revision CAS must fail after rewind");
    assert!(error.to_string().contains("session_revision_conflict"));
}

/// 多次回退向 sidecar 追加；超过每会话容量上限时裁掉最老，防无限膨胀。
#[test]
fn rewind_backups_append_and_cap_at_limit() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    for round in 0..21u32 {
        store
            .update_messages(
                &id,
                vec![
                    user_text(&format!("round {round} t1")),
                    assistant_text("答一"),
                    user_text(&format!("round {round} t2")),
                    assistant_text("答二"),
                ],
            )
            .expect("reseed transcript");
        store.truncate_to_user_turn(&id, 0, None).expect("rewind");
    }
    let records = rewound_records(&id);
    assert_eq!(records.len(), 20, "每会话备份条数封顶 20（LRU 裁最老）");
    // 最老的一条（round 0）已被裁掉，剩余的是最后 20 次。
    assert!(
        records
            .iter()
            .all(|record| record.removed_messages.len() == 4)
    );
}

/// 删除会话时回退备份 sidecar 同步清理。
#[test]
fn delete_session_purges_rewound_turns_backup() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    store
        .update_messages(
            &id,
            vec![
                user_text("第一轮"),
                assistant_text("答一"),
                user_text("第二轮"),
            ],
        )
        .expect("seed transcript");
    store.truncate_to_user_turn(&id, 1, None).expect("rewind");
    assert_eq!(rewound_records(&id).len(), 1);

    store.delete(&id).expect("delete session");

    // sidecar 中该会话的备份已清；无其他会话时整个文件被移除。
    assert!(!paths::sessions_root().join("_rewound_turns.json").exists());
}

// ===================== 回退反悔（restore_rewound_turns）+ compaction 标记 =====================

/// 反悔往返：截断后恢复，messages 回到截断前，sidecar 记录被消费删除。
#[test]
fn restore_rewound_turns_round_trips_messages_and_consumes_record() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    let messages = vec![
        user_text("第一轮"),
        assistant_text("答一"),
        tool_result_message("call_1"),
        user_text("第二轮"),
        assistant_text("答二"),
    ];
    store
        .update_messages(&id, messages.clone())
        .expect("seed transcript");
    store.truncate_to_user_turn(&id, 1, None).expect("rewind");
    assert_eq!(store.load(&id).expect("load").messages.len(), 3);

    let restored = store.restore_rewound_turns(&id).expect("undo rewind");

    assert_eq!(restored, 2);
    assert_eq!(
        store.load(&id).expect("load").messages,
        messages,
        "反悔后 transcript 必须逐条回到截断前"
    );
    // 记录已被消费：再次反悔如实报错。
    assert!(
        store
            .latest_rewound_turns_record(&id)
            .expect("read")
            .is_none()
    );
    assert!(store.restore_rewound_turns(&id).is_err());
}

/// 回退后发过新轮次 → 不可反悔，如实报错且 transcript 不动。
#[test]
fn restore_rewound_turns_rejects_after_new_turn() {
    let (store, _g) = isolated_store();
    let session = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create");
    let id = session.metadata.id.clone();
    store
        .update_messages(
            &id,
            vec![
                user_text("第一轮"),
                assistant_text("答一"),
                user_text("第二轮"),
                assistant_text("答二"),
            ],
        )
        .expect("seed transcript");
    store.truncate_to_user_turn(&id, 1, None).expect("rewind");
    // 回退后重新创作：追加新轮次（turn 数变为 2 ≠ kept_turns 1）。
    store
        .update_messages(
            &id,
            vec![
                user_text("第一轮"),
                assistant_text("答一"),
                user_text("新分支"),
                assistant_text("新答"),
            ],
        )
        .expect("append new branch turn");

    let error = store
        .restore_rewound_turns(&id)
        .expect_err("new turn after rewind must block undo");
    assert!(error.to_string().contains("不可反悔"), "{error:#}");
    assert_eq!(store.load(&id).expect("load").messages.len(), 4);
    // 记录保留（未消费），数据不丢。
    assert!(
        store
            .latest_rewound_turns_record(&id)
            .expect("read")
            .is_some()
    );
}

/// had_compaction：system_prompt 含/不含底座压缩摘要标记两例。
#[test]
fn truncate_reports_compaction_summary_residue_in_system_prompt() {
    let (store, _g) = isolated_store();
    let with_marker = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create with_marker");
    let without_marker = store
        .create_new("/model".into(), None, std::env::temp_dir())
        .expect("create without_marker");

    let messages = || {
        vec![
            user_text("第一轮"),
            assistant_text("答一"),
            user_text("第二轮"),
            assistant_text("答二"),
        ]
    };
    // 含标记：模拟底座 compaction 后持久化的 system_prompt。
    let mut state = chat_engine_state(messages());
    state.system_prompt = Some(SystemPrompt::Text(
        "前文摘要：Conversation Summary (Auto-Generated)\n……".to_string(),
    ));
    store
        .persist_chat_engine_state(&with_marker.metadata.id, state)
        .expect("persist with marker");
    store
        .update_messages(&without_marker.metadata.id, messages())
        .expect("seed plain");

    let outcome = store
        .truncate_to_user_turn(&with_marker.metadata.id, 1, None)
        .expect("rewind with marker");
    assert!(outcome.had_compaction, "含标记必须上报 had_compaction");

    let outcome = store
        .truncate_to_user_turn(&without_marker.metadata.id, 1, None)
        .expect("rewind without marker");
    assert!(!outcome.had_compaction, "普通 system_prompt 不得误报");
}
