//! Memory E2E tests.
//!
//! These tests intentionally use an isolated PINVOU3_HOME so they never touch a
//! real user's `~/.pinvou3`. The LLM test is ignored by default, matching the
//! existing L1 harness convention.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use deepseek_tui::core::engine::spawn_engine;
use deepseek_tui::core::events::Event;
use deepseek_tui::tui::app::AppMode;
use pinvou3_lib::features::assistant::platform::bridge::{paths, Pinvou3Bridge};
use pinvou3_lib::features::memory::{
    self, MemoryProfile, MemorySuggestion, PendingSensitiveIdentity, ProfileConventions,
    ProfileIdentity, ProfilePatch, RecentWorkItem, RecentWorkPatch, TimedMemoryItem,
    WorkContextFile,
};

const DEFAULT_VLLM_BASE_URL: &str = "http://127.0.0.1:8000/v1";

struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn set(&mut self, key: &'static str, value: impl Into<OsString>) {
        if !self.saved.iter().any(|(k, _)| *k == key) {
            self.saved.push((key, std::env::var_os(key)));
        }
        std::env::set_var(key, value.into());
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.iter().rev() {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn temp_root(name: &str) -> PathBuf {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("pinvou3-memory-e2e-{ns}-{name}"));
    std::fs::create_dir_all(&root).expect("create temp root");
    root
}

fn setup_isolated_home(name: &str) -> (EnvGuard, PathBuf) {
    let root = temp_root(name);
    let mut env = EnvGuard::new();
    env.set("PINVOU3_HOME", root.as_os_str().to_os_string());
    (env, root)
}

fn setup_memory_fixture(name: &str) -> (EnvGuard, PathBuf) {
    let (env, root) = setup_isolated_home(name);

    let mut pending = BTreeMap::new();
    pending.insert(
        "id_card".to_string(),
        PendingSensitiveIdentity {
            value: "身份证 110101199001010000".to_string(),
            source: "test".to_string(),
            status: "pending_confirm".to_string(),
        },
    );

    let profile = MemoryProfile {
        version: 1,
        updated_at: "2026-07-06T00:00:00Z".to_string(),
        revision: 1,
        identity: ProfileIdentity {
            call_name: "林主任".to_string(),
            assistant_alias: "小林".to_string(),
        },
        conventions: ProfileConventions {
            language: "使用简体中文".to_string(),
            doc_standard: "GB/T 9704".to_string(),
            number_usage: "GB/T 15835".to_string(),
            style_notes: vec!["回答先给结论".to_string(), "称呼用户为林主任".to_string()],
        },
        pending_sensitive_identity: pending,
    };
    memory::save_profile(&profile).expect("save memory profile");

    let pref_dir = paths::user_memory_preferences_dir();
    std::fs::create_dir_all(&pref_dir).expect("create preferences dir");
    let pref = serde_json::json!({
        "id": "pref.office.answer_style",
        "topic": "office_style",
        "scope": "unconditional",
        "text": "默认先给结论，再列 2-3 个要点。"
    });
    std::fs::write(
        pref_dir.join("office_style.json"),
        serde_json::to_vec_pretty(&pref).unwrap(),
    )
    .expect("write preference");

    (env, root)
}

fn write_preference(id: &str, scope: &str, text: &str) {
    let pref_dir = paths::user_memory_preferences_dir();
    std::fs::create_dir_all(&pref_dir).expect("create preferences dir");
    let pref = serde_json::json!({
        "id": id,
        "topic": id,
        "scope": scope,
        "text": text
    });
    std::fs::write(
        pref_dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&pref).unwrap(),
    )
    .expect("write preference");
}

fn write_recent_work_fixture(items: &[RecentWorkItem]) {
    let mut jsonl = String::new();
    for item in items {
        jsonl.push_str(&serde_json::to_string(item).unwrap());
        jsonl.push('\n');
    }
    if let Some(parent) = memory::recent_work_path().parent() {
        std::fs::create_dir_all(parent).expect("create recent work dir");
    }
    std::fs::write(memory::recent_work_path(), jsonl).expect("write recent work fixture");
}

fn write_timed_memory_fixture(path: PathBuf, items: &[TimedMemoryItem]) {
    let mut jsonl = String::new();
    for item in items {
        jsonl.push_str(&serde_json::to_string(item).unwrap());
        jsonl.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create timed memory dir");
    }
    std::fs::write(path, jsonl).expect("write timed memory fixture");
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn pending_memory_deduplicates_same_content_candidates() {
    let (_env, _root) = setup_isolated_home("pending-dedupe");

    let first = memory::enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "output_style".to_string(),
        content: "回答风格要求简明可爱".to_string(),
        source: "test".to_string(),
    })
    .expect("enqueue first candidate");
    let second = memory::enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "answer_style".to_string(),
        content: "回答风格要求简明可爱".to_string(),
        source: "test".to_string(),
    })
    .expect("enqueue duplicate candidate");

    assert_eq!(first.id, second.id);
    assert_eq!(second.seen_count, 2);
    let pending = memory::load_pending_memory().expect("load pending");
    assert_eq!(pending.len(), 1);
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn confirmed_preferences_upsert_by_standard_topic() {
    let (_env, _root) = setup_isolated_home("preference-upsert");

    let first = memory::enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "output_style".to_string(),
        content: "回答风格要求简洁".to_string(),
        source: "test".to_string(),
    })
    .expect("enqueue first preference");
    memory::confirm_pending_memory(&first.id).expect("confirm first preference");

    let second = memory::enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "answer_style".to_string(),
        content: "回答风格要求简洁、俏皮可爱".to_string(),
        source: "test".to_string(),
    })
    .expect("enqueue updated preference");
    memory::confirm_pending_memory(&second.id).expect("confirm updated preference");

    let preferences = memory::list_preferences().expect("list preferences");
    assert_eq!(preferences.len(), 1);
    assert_eq!(preferences[0].topic, "answer_style");
    assert_eq!(preferences[0].text, "回答风格要求简洁、俏皮可爱");
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_overview_filters_legacy_profile_preferences_and_cleans_profile_labels() {
    let (_env, _root) = setup_isolated_home("legacy-profile-pref-cleanup");

    memory::update_profile(ProfilePatch {
        call_name: Some("称呼用户为\"老板\"".to_string()),
        assistant_alias: Some("助手昵称叫“小小”".to_string()),
        language: None,
        doc_standard: None,
        number_usage: None,
        style_notes: None,
    })
    .expect("update legacy profile labels");

    let profile = memory::load_profile().expect("load profile");
    assert_eq!(profile.identity.call_name, "老板");
    assert_eq!(profile.identity.assistant_alias, "小小");

    write_preference(
        "legacy.profile.call_name",
        "unconditional",
        "称呼用户为\"老板\"",
    );
    write_preference(
        "legacy.profile.alias",
        "unconditional",
        "助手昵称叫\"小小\"",
    );
    write_preference("pref.answer", "unconditional", "回答风格要求简洁");

    let preferences = memory::list_preferences().expect("list preferences");
    assert_eq!(preferences.len(), 1);
    assert_eq!(preferences[0].text, "回答风格要求简洁");
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_prompt_injection_and_quality() {
    let (_env, root) = setup_memory_fixture("prompt");
    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");

    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let cfg = bridge.build_engine_config_for_session("memory/e2e:prompt");

    let runtime_path = memory::runtime_prompt_path("memory/e2e:prompt");
    let runtime = std::fs::read_to_string(&runtime_path).expect("read runtime memory prompt");

    assert!(
        cfg.instructions.iter().any(|source| {
            matches!(source, deepseek_tui::prompts::InstructionSource::File(path) if path == &runtime_path)
        }),
        "session instructions must include the memory runtime file"
    );
    assert!(runtime.contains("<pinvou_user_memory>"));
    assert!(runtime.contains("权威层级：低于用户当前指令"));
    assert!(runtime.contains("称呼：林主任"));
    assert!(runtime.contains("GB/T 9704"));
    assert!(runtime.contains("GB/T 15835"));
    assert!(runtime.contains("默认先给结论"));
    assert!(!runtime.contains("身份证"));
    assert!(!runtime.contains("110101199001010000"));
    assert!(
        runtime.chars().count() < 600,
        "P0 runtime memory should stay compact, got {} chars:\n{runtime}",
        runtime.chars().count()
    );
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_profile_correction_and_clear_updates_files() {
    let (_env, _root) = setup_isolated_home("profile-correction");

    memory::update_profile(ProfilePatch {
        call_name: Some("王主任".to_string()),
        assistant_alias: None,
        language: Some("使用简体中文".to_string()),
        doc_standard: Some("GB/T 9704".to_string()),
        number_usage: None,
        style_notes: Some(vec!["先给结论".to_string()]),
    })
    .expect("initial profile update");
    let first = memory::runtime_snapshot("correction-a").expect("first runtime");
    assert!(first.block.contains("王主任"));
    assert!(std::fs::read_to_string(memory::profile_path())
        .expect("read profile")
        .contains("王主任"));

    memory::update_profile(ProfilePatch {
        call_name: Some("林主任".to_string()),
        assistant_alias: None,
        language: None,
        doc_standard: None,
        number_usage: None,
        style_notes: None,
    })
    .expect("correct profile");
    let corrected = memory::runtime_snapshot("correction-a").expect("corrected runtime");
    assert!(corrected.block.contains("林主任"));
    assert!(!corrected.block.contains("王主任"));

    memory::clear_profile().expect("clear profile");
    let cleared = memory::runtime_snapshot("correction-a").expect("cleared runtime");
    assert!(!cleared.block.contains("林主任"));
    assert!(!cleared.block.contains("画像："));
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_preference_scope_and_recent_work_ttl_quality() {
    let (_env, _root) = setup_isolated_home("recent-work");
    write_preference(
        "pref.always",
        "unconditional",
        "默认先给结论，再列 2-3 个要点。",
    );
    write_preference(
        "pref.conditional",
        "scene:ppt",
        "只有做 PPT 时才使用 5 页以内结构。",
    );

    let now = Utc::now();
    let make_recent = |id: &str, title: &str, days_offset: i64, status: &str| RecentWorkItem {
        id: id.to_string(),
        title: title.to_string(),
        summary: "用于记忆质量测试".to_string(),
        status: status.to_string(),
        source: "test".to_string(),
        created_at: (now + ChronoDuration::days(days_offset)).to_rfc3339(),
        updated_at: (now + ChronoDuration::days(days_offset)).to_rfc3339(),
        last_hit: (now + ChronoDuration::days(days_offset)).to_rfc3339(),
        expires_at: (now + ChronoDuration::days(days_offset + 7)).to_rfc3339(),
    };
    write_recent_work_fixture(&[
        make_recent("old", "已经过期的旧总结", -30, "active"),
        make_recent("archived", "已经归档的会议材料", 0, "archived"),
        make_recent("a", "筹备营商环境推进会材料", 0, "active"),
        make_recent("b", "起草上半年工作总结", -1, "active"),
        make_recent("c", "整理招商项目清单", -2, "active"),
        make_recent("d", "第四条不应注入", -3, "active"),
    ]);

    let s1 = memory::runtime_snapshot("recent-session-a").expect("runtime a");
    let s2 = memory::runtime_snapshot("recent-session-b").expect("runtime b");
    assert_ne!(s1.runtime_path, s2.runtime_path);
    assert_eq!(s1.block, s2.block);

    assert!(s1.block.contains("默认先给结论"));
    assert!(!s1.block.contains("只有做 PPT"));
    assert!(s1.block.contains("筹备营商环境推进会材料"));
    assert!(s1.block.contains("起草上半年工作总结"));
    assert!(s1.block.contains("整理招商项目清单"));
    assert!(!s1.block.contains("第四条不应注入"));
    assert!(!s1.block.contains("已经过期"));
    assert!(!s1.block.contains("已经归档"));
    assert_eq!(
        s1.items
            .iter()
            .filter(|item| item.kind == "current_focus")
            .count(),
        3
    );

    let recent_file = std::fs::read_to_string(memory::recent_work_path()).expect("read recent");
    assert!(recent_file.contains("\"status\":\"archived\""));
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_manual_recent_work_update_is_compact_and_actionable() {
    let (_env, _root) = setup_isolated_home("recent-upsert");
    let item = memory::upsert_recent_work(RecentWorkPatch {
        id: Some("current-summary".to_string()),
        title: "正在写上半年全委工作总结，需要重点突出项目建设和营商环境成效，标题很长也要被截断"
            .to_string(),
        summary: Some(
            "下一步补齐数据表和三条问题建议，摘要也不能太长，否则会污染 prompt".to_string(),
        ),
        source: Some("user_declared".to_string()),
        ttl_days: Some(7),
    })
    .expect("upsert recent work");
    assert!(item.title.chars().count() <= 50);
    assert!(item.summary.chars().count() <= 80);

    let snapshot = memory::runtime_snapshot("recent-upsert").expect("runtime");
    assert!(snapshot.block.contains("当前关注"));
    assert!(snapshot.block.contains("正在处理："));
    assert!(snapshot.block.chars().count() < 700);

    memory::archive_recent_work("current-summary").expect("archive");
    let archived = memory::runtime_snapshot("recent-upsert").expect("archived runtime");
    assert!(!archived.block.contains("上半年全委工作总结"));
}

#[test]
#[ignore = "memory E2E mutates process env; run explicitly with --test-threads=1"]
fn memory_runtime_injects_effective_five_layer_memory_only() {
    let (_env, _root) = setup_isolated_home("five-layer-runtime");
    let now = Utc::now();
    let work_context = WorkContextFile {
        id: "ctx_role_domain".to_string(),
        kind: "work_context".to_string(),
        topic: "role_domain".to_string(),
        text: "用户长期参与本地 AI 办公助手开发，关注产品体验和记忆系统。".to_string(),
        source: "test".to_string(),
        confidence: 0.9,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };
    let work_dir = memory::work_context_dir();
    std::fs::create_dir_all(&work_dir).expect("create work context dir");
    std::fs::write(
        work_dir.join("ctx_role_domain.json"),
        serde_json::to_vec_pretty(&work_context).unwrap(),
    )
    .expect("write work context");

    let focus = TimedMemoryItem {
        id: "focus_memory".to_string(),
        kind: "current_focus".to_string(),
        topic: "current_work".to_string(),
        text: "正在完善 pinvou 记忆系统的自动写入机制。".to_string(),
        source: "test".to_string(),
        confidence: 0.91,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_hit: now.to_rfc3339(),
        ttl_days: 21,
        status: "active".to_string(),
    };
    let activity = TimedMemoryItem {
        id: "activity_memory".to_string(),
        kind: "recent_activity".to_string(),
        topic: "completed_work".to_string(),
        text: "已修复记忆候选重复弹出的问题。".to_string(),
        source: "test".to_string(),
        confidence: 0.9,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_hit: now.to_rfc3339(),
        ttl_days: 14,
        status: "active".to_string(),
    };
    write_timed_memory_fixture(memory::current_focus_path(), &[focus]);
    write_timed_memory_fixture(memory::recent_activity_path(), &[activity]);

    let pending = memory::enqueue_memory_candidate(MemorySuggestion {
        kind: "preference".to_string(),
        topic: "answer_style".to_string(),
        content: "这条 pending 不应注入主 prompt".to_string(),
        source: "test".to_string(),
    })
    .expect("enqueue pending");
    memory::never_pending_memory(&pending.id, Some("test".to_string())).expect("never pending");

    let snapshot = memory::runtime_snapshot("five-layer").expect("runtime");
    assert!(snapshot.block.contains("工作背景"));
    assert!(snapshot.block.contains("当前关注"));
    assert!(snapshot.block.contains("近期动态"));
    assert!(snapshot.block.contains("本地 AI 办公助手开发"));
    assert!(snapshot.block.contains("自动写入机制"));
    assert!(snapshot.block.contains("重复弹出的问题"));
    assert!(!snapshot.block.contains("pending 不应注入"));
    assert!(snapshot
        .items
        .iter()
        .any(|item| item.kind == "work_context"));
    assert!(snapshot
        .items
        .iter()
        .any(|item| item.kind == "current_focus"));
    assert!(snapshot
        .items
        .iter()
        .any(|item| item.kind == "recent_activity"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_uses_profile_in_fresh_session() {
    let (mut env, root) = setup_memory_fixture("llm");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!("SKIP memory_llm_uses_profile_in_fresh_session: vLLM endpoint unreachable");
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let mut cfg = bridge.build_engine_config_for_session("memory/e2e:llm");
    cfg.disallowed_tools = Some(vec!["*".to_string()]);
    let dt_config = bridge.build_dt_config();
    let handle = spawn_engine(cfg, &dt_config);

    let op = bridge.build_send_message_op(
        "memory/e2e:llm",
        "我没有在本轮告诉你我的称呼。请只根据你能看到的长期记忆，用一句话回答：你应该怎么称呼我？不要解释。"
            .to_string(),
        AppMode::Yolo,
        None,
        true,
    )
    .expect("build memory probe op");
    handle.send(op).await.expect("send memory probe");

    let (answer, elapsed) = collect_answer(&handle, Duration::from_secs(90)).await;
    let transcript_dir = root.join("transcripts");
    std::fs::create_dir_all(&transcript_dir).expect("create transcript dir");
    std::fs::write(
        transcript_dir.join("memory_llm_uses_profile_in_fresh_session.txt"),
        &answer,
    )
    .expect("write transcript");

    eprintln!(
        "[memory_llm_uses_profile_in_fresh_session] elapsed={:.1}s answer={answer:?}",
        elapsed.as_secs_f64()
    );
    assert!(
        answer.contains("林主任"),
        "answer should use saved call_name: {answer}"
    );
    assert!(
        !answer.contains("不知道") && !answer.contains("无法"),
        "answer should not claim memory is unavailable: {answer}"
    );
    assert!(
        answer.chars().count() <= 80,
        "answer should be concise, got {} chars: {answer}",
        answer.chars().count()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_current_instruction_overrides_memory() {
    let (mut env, root) = setup_memory_fixture("llm-override");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!(
            "SKIP memory_llm_current_instruction_overrides_memory: vLLM endpoint unreachable"
        );
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let mut cfg = bridge.build_engine_config_for_session("memory/e2e:override");
    cfg.disallowed_tools = Some(vec!["*".to_string()]);
    let dt_config = bridge.build_dt_config();
    let handle = spawn_engine(cfg, &dt_config);

    let op = bridge.build_send_message_op(
        "memory/e2e:override",
        "长期记忆里可能有默认称呼，但本轮请称呼我周老师。只回答：本轮应该称呼我什么？不要解释。"
            .to_string(),
        AppMode::Yolo,
        None,
        true,
    )
    .expect("build override probe op");
    handle.send(op).await.expect("send override probe");
    let (answer, elapsed) = collect_answer(&handle, Duration::from_secs(90)).await;

    eprintln!(
        "[memory_llm_current_instruction_overrides_memory] elapsed={:.1}s answer={answer:?}",
        elapsed.as_secs_f64()
    );
    assert!(
        answer.contains("周老师"),
        "current instruction should win: {answer}"
    );
    assert!(
        !answer.contains("林主任"),
        "answer should not use stale memory when current instruction conflicts: {answer}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_one_off_task_does_not_create_long_term_memory() {
    let (mut env, root) = setup_isolated_home("llm-one-off");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!(
            "SKIP memory_llm_one_off_task_does_not_create_long_term_memory: vLLM endpoint unreachable"
        );
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let mut cfg = bridge.build_engine_config_for_session("memory/e2e:one-off");
    cfg.disallowed_tools = Some(vec!["*".to_string()]);
    let dt_config = bridge.build_dt_config();
    let handle = spawn_engine(cfg, &dt_config);

    let op = bridge
        .build_send_message_op(
            "memory/e2e:one-off",
            "这是一次性临时任务：今天午饭我可能吃面。不要把它当长期偏好。请只回答“收到”。"
                .to_string(),
            AppMode::Yolo,
            None,
            true,
        )
        .expect("build one-off probe op");
    handle.send(op).await.expect("send one-off probe");
    let (_answer, _elapsed) = collect_answer(&handle, Duration::from_secs(90)).await;

    assert!(
        !memory::profile_path().exists(),
        "one-off chat must not create profile.json"
    );
    assert!(
        !memory::recent_work_path().exists(),
        "one-off chat must not create recent_work.jsonl"
    );
    assert!(
        !paths::user_memory_preferences_dir().exists(),
        "one-off chat must not create preferences"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_review_cleans_profile_and_rejects_question() {
    let (mut env, root) = setup_isolated_home("llm-review");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!(
            "SKIP memory_llm_review_cleans_profile_and_rejects_question: vLLM endpoint unreachable"
        );
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let session_id = "memory/e2e:review-clean";

    let question = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "我是谁".to_string(),
            assistant: "你是小猪。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review question");
    assert!(question.events.is_empty());
    assert!(question.pending.is_empty());
    assert!(memory::load_profile()
        .expect("load profile after question")
        .identity
        .call_name
        .is_empty());

    let remembered = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "以后你都叫我欣哥，我叫你小猪".to_string(),
            assistant: "好的，我记住了。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review explicit profile");
    assert!(remembered
        .events
        .iter()
        .any(|event| event.id == "profile.call_name" && event.text.contains("欣哥")));
    assert!(remembered
        .events
        .iter()
        .any(|event| event.id == "profile.assistant_alias" && event.text.contains("小猪")));

    let profile = memory::load_profile().expect("load profile after explicit profile");
    assert_eq!(profile.identity.call_name, "欣哥");
    assert_eq!(profile.identity.assistant_alias, "小猪");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_realistic_effect_snapshot() {
    let (mut env, root) = setup_isolated_home("llm-realistic-effect");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!("SKIP memory_llm_realistic_effect_snapshot: vLLM endpoint unreachable");
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let session_id = "memory/e2e:realistic-effect";

    let question = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "我是谁？".to_string(),
            assistant: "你是小猪。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review question");
    assert!(question.events.is_empty());
    assert!(question.pending.is_empty());

    let profile = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "以后叫我欣哥，我叫你小猪。".to_string(),
            assistant: "好的，以后我称呼你欣哥，你叫我小猪。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review profile");

    let preference = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "以后回答问题默认先给结论，再列 2-3 个要点，不要写长篇。".to_string(),
            assistant: "收到，后续我会按这个回答风格处理。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review preference");

    let current_focus = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "我最近主要在推进 pinvou 记忆系统，要把自动发现、确认和近期工作状态做稳定。"
                .to_string(),
            assistant: "明白，这属于你近期正在推进的 pinvou 记忆系统工作。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review current focus");

    let delivery = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "帮我把 pinvou 记忆系统设计文档改成可开发方案。".to_string(),
            assistant: "已完成：设计文档已经改成可开发方案，并补充了分类、写入、确认和过期策略。"
                .to_string(),
            tool_summaries: vec![
                "tool_complete name=edit_file success=true path=docs/记忆功能-架构设计.md"
                    .to_string(),
            ],
            delivery_complete: true,
        },
        session_id,
    )
    .await
    .expect("review delivery");

    let pending = memory::load_pending_memory().expect("load pending");
    for item in pending.iter().filter(|item| item.kind == "preference") {
        memory::confirm_pending_memory(&item.id).expect("confirm preference candidate");
    }

    let profile_state = memory::load_profile().expect("load profile");
    let preferences = memory::list_preferences().expect("list preferences");
    let work_context = memory::load_work_context().expect("load work context");
    let focus = memory::load_current_focus().expect("load current focus");
    let activity = memory::load_recent_activity().expect("load recent activity");
    let snapshot = memory::runtime_snapshot("realistic-effect").expect("runtime snapshot");

    eprintln!(
        "\n[realistic-effect] profile events:\n{}",
        serde_json::to_string_pretty(&profile).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] preference outcome:\n{}",
        serde_json::to_string_pretty(&preference).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] current focus outcome:\n{}",
        serde_json::to_string_pretty(&current_focus).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] delivery outcome:\n{}",
        serde_json::to_string_pretty(&delivery).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] profile file:\n{}",
        serde_json::to_string_pretty(&profile_state).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] preferences:\n{}",
        serde_json::to_string_pretty(&preferences).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] work_context:\n{}",
        serde_json::to_string_pretty(&work_context).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] current_focus:\n{}",
        serde_json::to_string_pretty(&focus).unwrap()
    );
    eprintln!(
        "\n[realistic-effect] recent_activity:\n{}",
        serde_json::to_string_pretty(&activity).unwrap()
    );
    eprintln!("\n[realistic-effect] runtime block:\n{}", snapshot.block);

    if profile_state.identity.call_name.is_empty()
        || profile_state.identity.assistant_alias.is_empty()
    {
        eprintln!("[realistic-effect] quality note: profile was not produced in this run");
    }
    if preferences.is_empty() {
        eprintln!("[realistic-effect] quality note: preference was not produced in this run");
    } else {
        assert!(
            preferences
                .iter()
                .any(|item| item.text.contains("先给结论") || item.text.contains("2-3")),
            "preference should stay compact and relevant: {preferences:#?}"
        );
    }
    if focus.is_empty() {
        eprintln!("[realistic-effect] quality note: current_focus was not produced in this run");
    } else {
        assert!(
            focus
                .iter()
                .any(|item| item.text.contains("pinvou") || item.text.contains("记忆系统")),
            "current focus should stay relevant: {focus:#?}"
        );
    }
    if activity.is_empty() {
        eprintln!("[realistic-effect] quality note: recent_activity was not produced in this run");
    } else {
        assert!(
            activity
                .iter()
                .any(|item| item.text.contains("设计文档") || item.text.contains("可开发方案")),
            "recent activity should stay relevant: {activity:#?}"
        );
    }
    assert!(!snapshot.block.contains("_pending"));
    if !profile_state.identity.call_name.is_empty() {
        assert!(snapshot.block.contains("画像"));
    }
    if !preferences.is_empty() {
        assert!(snapshot.block.contains("长期偏好"));
    }
    if !focus.is_empty() {
        assert!(snapshot.block.contains("当前关注"));
    }
    if !activity.is_empty() {
        assert!(snapshot.block.contains("近期动态"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires live local vLLM endpoint; run explicitly with --test-threads=1"]
async fn memory_llm_background_project_midterm_snapshot() {
    let (mut env, root) = setup_isolated_home("llm-background-project-midterm");
    setup_vllm_env(&mut env);
    if !vllm_alive().await {
        eprintln!("SKIP memory_llm_background_project_midterm_snapshot: vLLM endpoint unreachable");
        return;
    }

    let ws = root.join("workspace");
    std::fs::create_dir_all(&ws).expect("create workspace");
    let bridge = Pinvou3Bridge::boot_with_workspace(ws).expect("boot bridge");
    let session_id = "memory/e2e:background-project-midterm";

    let user_background = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "请记住我的长期工作背景：我长期负责企业内部办公自动化和本地 AI 助手落地，经常需要把需求评审、技术方案和测试结论整理成可执行文档。"
                .to_string(),
            assistant: "收到，这属于你的长期工作背景，我会按记忆策略整理。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review user background");

    let project_background = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "也请记住一个长期项目背景：我长期维护 pinvou，它是本地 AI 通用办公助手，重点关注桌面体验、记忆系统、本地模型和办公工作流。这里说的是我长期做的项目，不是当前运行环境。"
                .to_string(),
            assistant: "明白，这会作为你的长期项目背景候选，不会当作 pinvou 当前运行环境。"
                .to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review project background");

    let current_focus = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "这两周我正在做 pinvou 的记忆系统和设置页联动，重点验证中期记忆、项目记忆、用户背景能不能正确生成。"
                .to_string(),
            assistant: "明白，这是你近期正在推进的 pinvou 记忆系统验证工作。".to_string(),
            ..memory::TurnMemoryCapture::default()
        },
        session_id,
    )
    .await
    .expect("review midterm current focus");

    let recent_activity = memory::review_turn_candidates_with_llm(
        &bridge,
        &memory::TurnMemoryCapture {
            user: "刚刚已经完成了中期记忆、项目记忆、用户背景这三类测试场景设计。"
                .to_string(),
            assistant: "已完成：已经整理出中期记忆、项目记忆和用户背景三类测试场景，并准备用它们验证生成质量。"
                .to_string(),
            tool_summaries: vec![
                "tool_complete name=edit_file success=true path=pinvou3-app/src-tauri/tests/memory_e2e.rs"
                    .to_string(),
            ],
            delivery_complete: true,
        },
        session_id,
    )
    .await
    .expect("review midterm recent activity");

    let pending = memory::load_pending_memory().expect("load pending");
    for item in pending
        .iter()
        .filter(|item| item.kind == "work_context" || item.kind == "preference")
    {
        memory::confirm_pending_memory(&item.id).expect("confirm background candidate");
    }

    let work_context = memory::load_work_context().expect("load work context");
    let focus = memory::load_current_focus().expect("load current focus");
    let activity = memory::load_recent_activity().expect("load recent activity");
    let snapshot = memory::runtime_snapshot("background-project-midterm").expect("runtime");

    eprintln!(
        "\n[background-project-midterm] user background outcome:\n{}",
        serde_json::to_string_pretty(&user_background).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] project background outcome:\n{}",
        serde_json::to_string_pretty(&project_background).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] current focus outcome:\n{}",
        serde_json::to_string_pretty(&current_focus).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] recent activity outcome:\n{}",
        serde_json::to_string_pretty(&recent_activity).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] work_context:\n{}",
        serde_json::to_string_pretty(&work_context).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] current_focus:\n{}",
        serde_json::to_string_pretty(&focus).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] recent_activity:\n{}",
        serde_json::to_string_pretty(&activity).unwrap()
    );
    eprintln!(
        "\n[background-project-midterm] runtime block:\n{}",
        snapshot.block
    );

    assert!(
        work_context.iter().any(|item| {
            item.text.contains("办公自动化")
                || item.text.contains("本地 AI 助手")
                || item.text.contains("可执行文档")
        }),
        "user background should become work_context: {work_context:#?}"
    );
    assert!(
        work_context
            .iter()
            .any(|item| item.text.contains("pinvou") && item.text.contains("本地 AI")),
        "project background should become work_context: {work_context:#?}"
    );
    if focus.is_empty() {
        eprintln!(
            "[background-project-midterm] quality note: current_focus was not produced in this run"
        );
    } else {
        assert!(
            focus
                .iter()
                .any(|item| item.text.contains("设置页") || item.text.contains("中期记忆")),
            "current focus should capture active midterm work: {focus:#?}"
        );
    }
    if activity.is_empty() {
        eprintln!(
            "[background-project-midterm] quality note: recent_activity was not produced in this run"
        );
    } else {
        assert!(
            activity
                .iter()
                .any(|item| item.text.contains("测试场景") || item.text.contains("用户背景")),
            "recent activity should capture completed test-design work: {activity:#?}"
        );
    }
    assert!(snapshot.block.contains("工作背景"));
    if !focus.is_empty() {
        assert!(snapshot.block.contains("当前关注"));
    }
    if !activity.is_empty() {
        assert!(snapshot.block.contains("近期动态"));
    }
    assert!(!snapshot.block.contains("_pending"));
}

fn setup_vllm_env(env: &mut EnvGuard) {
    env.set("DEEPSEEK_PROVIDER", "vllm");
    env.set("DEEPSEEK_API_KEY", "local-no-auth");
    if std::env::var_os("DEEPSEEK_BASE_URL").is_none() {
        env.set("DEEPSEEK_BASE_URL", DEFAULT_VLLM_BASE_URL);
    }
    if std::env::var_os("DEEPSEEK_MODEL").is_none() {
        env.set("DEEPSEEK_MODEL", "qwen36_35b_256k");
    }
    env.set("DEEPSEEK_ALLOW_INSECURE_HTTP", "1");
    env.set("DEEPSEEK_FORCE_HTTP1", "1");
    env.set("DEEPSEEK_MAX_OUTPUT_TOKENS", "2048");
    // 真 vLLM 测试与正式 App、CodeWhale 默认值保持一致。
    env.set("DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS", "300");
    env.set("DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS", "180");
}

async fn vllm_alive() -> bool {
    let base = std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_VLLM_BASE_URL.into());
    let probe = format!("{}/models", base.trim_end_matches('/'));
    match tokio::time::timeout(Duration::from_secs(3), reqwest::get(&probe)).await {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

async fn collect_answer(
    handle: &deepseek_tui::core::engine::EngineHandle,
    timeout: Duration,
) -> (String, Duration) {
    let start = Instant::now();
    let mut answer = String::new();
    let mut rx = handle.rx_event.write().await;
    loop {
        let remaining = timeout.checked_sub(start.elapsed()).unwrap_or_default();
        assert!(
            !remaining.is_zero(),
            "timeout waiting for memory answer: {answer}"
        );
        let event = tokio::time::timeout(remaining, rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("engine event channel closed");
        match event {
            Event::MessageDelta { content, .. } => answer.push_str(&content),
            Event::TurnComplete { error, .. } => {
                assert!(error.is_none(), "turn completed with error: {error:?}");
                break;
            }
            Event::Error { envelope, .. } => {
                panic!("engine error {}: {}", envelope.code, envelope.message);
            }
            _ => {}
        }
    }
    (answer.trim().to_string(), start.elapsed())
}
