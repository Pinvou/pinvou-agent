//! Tests for the memory feature. 抽离自 `mod.rs` 的 `#[cfg(test)] mod tests`，
//! 逐字保留原测试体，仅通过 `use` 把拆分到各子模块的内部 helper 重新引入作用域。
//! 4 个仅测试用的确定性提取器（`extract_deterministic_profile` 等）也搬到了这里。

use std::fs;
use std::path::PathBuf;

use chrono::{Duration, Utc};
use serde_json::json;

use super::io::{
    current_focus_path, enqueue_memory_candidate, load_preferences, load_profile,
    pending_item_from_suggestion, summarize_tool_start, upsert_timed_memory_unlocked,
    write_never_memory_unlocked, write_pending_memory_unlocked, write_recent_work_unlocked,
    write_timed_memory_file,
};
use super::llm_review::{
    append_memory_review_diagnostic_to, apply_llm_memory_review,
    assistant_suggests_delivery_complete, discover_turn_suggestions, has_memory_review_signal,
    memory_review_error_stage, parse_llm_memory_review, sanitize_llm_memory_item,
    LLM_REVIEW_PROMPT,
};
use super::render::render_from_parts;
// 引入全部常量（MAX_STORED / PENDING_STATUS_* / PROFILE_VERSION / Llm* 实体）。
use super::types::*;
use super::util::{clean_memory_label, clean_scalar};

// 重新暴露 super::* 上的 pub 面（MemoryProfile / ProfileIdentity / ... ）
use super::*;

#[allow(unused_imports)]
use super::util::{
    looks_completed_work_status, looks_recent_work_status, looks_sensitive, looks_task_like,
};

fn extract_deterministic_profile(message: &str) -> Option<(Option<String>, Option<String>)> {
    let mut call_name = None;
    let mut assistant_alias = None;

    for clause in split_memory_clauses(message) {
        if call_name.is_none() {
            call_name = extract_call_name(&clause);
        }
        if assistant_alias.is_none() {
            assistant_alias = extract_assistant_alias(&clause);
        }
    }

    if call_name.is_some() || assistant_alias.is_some() {
        Some((call_name, assistant_alias))
    } else {
        None
    }
}

fn split_memory_clauses(message: &str) -> Vec<String> {
    message
        .split(|c| {
            matches!(
                c,
                '。' | '，' | ',' | '；' | ';' | '\n' | '！' | '!' | '？' | '?'
            )
        })
        .map(clean_scalar)
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_call_name(clause: &str) -> Option<String> {
    if clause.starts_with("我叫你") {
        return None;
    }
    if let Some(after) = clause
        .strip_prefix("别叫我")
        .or_else(|| clause.strip_prefix("不要叫我"))
    {
        if let Some((_, new_name)) = after
            .split_once("叫我")
            .or_else(|| after.split_once("称呼我"))
        {
            return clean_memory_label(new_name);
        }
    }

    for prefix in [
        "以后你都叫我",
        "以后都叫我",
        "以后叫我",
        "之后叫我",
        "以后称呼我",
        "你可以叫我",
        "请叫我",
        "叫我",
        "我叫",
    ] {
        if let Some(value) = clause.strip_prefix(prefix) {
            return clean_memory_label(value);
        }
    }
    None
}

fn extract_assistant_alias(clause: &str) -> Option<String> {
    for prefix in [
        "以后我都叫你",
        "我都叫你",
        "以后我叫你",
        "我叫你",
        "你的名字叫",
    ] {
        if let Some(value) = clause.strip_prefix(prefix) {
            return clean_memory_label(value);
        }
    }
    for prefix in ["以后你都叫", "以后你叫", "你叫"] {
        if let Some(value) = clause.strip_prefix(prefix) {
            if value.starts_with('我') {
                return None;
            }
            return clean_memory_label(value);
        }
    }
    None
}

struct IsolatedPinvouHome {
    root: PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
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
fn deterministic_capture_extracts_names_only_from_explicit_phrases() {
    let extracted = extract_deterministic_profile("以后你都叫我欣哥，我都叫你小猪").unwrap();
    assert_eq!(extracted.0.as_deref(), Some("欣哥"));
    assert_eq!(extracted.1.as_deref(), Some("小猪"));

    assert!(extract_deterministic_profile("帮我写一个周报").is_none());
    assert!(extract_deterministic_profile("叫我写一个周报").is_none());
    assert!(extract_deterministic_profile("我是谁").is_none());
    assert!(extract_deterministic_profile("我叫什么").is_none());

    let alias_only = extract_deterministic_profile("我叫你小猪").unwrap();
    assert_eq!(alias_only.0, None);
    assert_eq!(alias_only.1.as_deref(), Some("小猪"));
    assert!(extract_deterministic_profile("我是贺欣").is_none());
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
