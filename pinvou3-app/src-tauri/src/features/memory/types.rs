//! Memory feature 数据模型：实体 struct/enum、trait、常量与字段规范化。
//!
//! 抽离自 `mod.rs`——这里只放纯数据定义与 `impl MemoryProfile`（依赖 util 的
//! clean_* 分类器做归一化）。topic/kind 归一化是纯字符串映射，被 io 与
//! llm_review 共用，也集中在此。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::platform::prefs::ModelPreset;

use super::util::{clean_id, clean_memory_label, clean_scalar, clean_text, invalid_memory_label};

pub trait MemoryReviewModel {
    fn memory_provider(&self) -> String;
    fn memory_model(&self) -> String;
    fn memory_base_url(&self) -> String;
    fn memory_api_key(&self) -> String;
    fn memory_model_preset(&self) -> ModelPreset;
    /// UI locale（BCP 47 tag，如 "zh-Hans"/"en"/"ja"）。记忆复盘提示词本体是中文，
    /// 非中文 locale 需要追加输出语言指令，否则记忆 content 会被写成中文、
    /// 注入英文 UI 会话后拽偏回复语言（同 review 侧 output_language_directive 先例）。
    fn memory_locale_tag(&self) -> String;
}

pub const PROFILE_VERSION: u32 = 1;
pub const RECENT_WORK_DEFAULT_TTL_DAYS: i64 = 14;
pub const RECENT_WORK_MAX_INJECTED: usize = 3;
pub const RECENT_WORK_ACTIVE_MAX_STORED: usize = 8;
pub const RECENT_WORK_ARCHIVED_MAX_STORED: usize = 24;
pub const CURRENT_FOCUS_DEFAULT_TTL_DAYS: i64 = 21;
pub const RECENT_ACTIVITY_DEFAULT_TTL_DAYS: i64 = 14;
pub const CURRENT_FOCUS_MAX_INJECTED: usize = 3;
pub const RECENT_ACTIVITY_MAX_INJECTED: usize = 5;
pub const CURRENT_FOCUS_ACTIVE_MAX_STORED: usize = 8;
pub const RECENT_ACTIVITY_ACTIVE_MAX_STORED: usize = 20;
pub const TIMED_MEMORY_ARCHIVED_MAX_STORED: usize = 40;
pub const PENDING_MEMORY_ACTIVE_MAX_STORED: usize = 20;
pub const PENDING_MEMORY_RESOLVED_MAX_STORED: usize = 80;
pub const NEVER_MEMORY_MAX_STORED: usize = 200;
pub const MEMORY_REVIEW_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const PENDING_STATUS_OBSERVED: &str = "observed";
pub const PENDING_STATUS_PENDING: &str = "pending_confirm";
pub const PENDING_STATUS_CONFIRMED: &str = "confirmed";
pub const PENDING_STATUS_IGNORED: &str = "ignored";

pub(super) fn default_profile_version() -> u32 {
    PROFILE_VERSION
}

pub(super) fn default_pending_status() -> String {
    "pending_confirm".to_string()
}

pub(super) fn default_recent_status() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryProfile {
    #[serde(default = "default_profile_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub identity: ProfileIdentity,
    #[serde(default)]
    pub conventions: ProfileConventions,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_sensitive_identity: BTreeMap<String, PendingSensitiveIdentity>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileIdentity {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub call_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub assistant_alias: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileConventions {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc_standard: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub number_usage: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub style_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingSensitiveIdentity {
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default = "default_pending_status")]
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilePatch {
    pub call_name: Option<String>,
    pub assistant_alias: Option<String>,
    pub language: Option<String>,
    pub doc_standard: Option<String>,
    pub number_usage: Option<String>,
    pub style_notes: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecentWorkPatch {
    pub id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub source: Option<String>,
    pub ttl_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecentWorkItem {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default = "default_recent_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_hit: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkContextFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub confidence: f32,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedMemoryItem {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub topic: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default)]
    pub confidence: f32,
    pub created_at: String,
    pub updated_at: String,
    pub last_hit: String,
    pub ttl_days: i64,
    #[serde(default = "default_recent_status")]
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryTextPatch {
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub ttl_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryWriteEvent {
    pub kind: String,
    pub action: String,
    pub id: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySuggestion {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub topic: String,
    pub content: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingMemoryItem {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub topic: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub status: String,
    pub seen_count: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryReviewOutcome {
    #[serde(default)]
    pub events: Vec<MemoryWriteEvent>,
    #[serde(default)]
    pub pending: Vec<PendingMemoryItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct LlmMemoryReview {
    #[serde(default)]
    pub(super) items: Vec<LlmMemoryItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(super) struct LlmMemoryItem {
    #[serde(default)]
    pub(super) action: String,
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) topic: String,
    #[serde(default)]
    pub(super) content: String,
    #[serde(default)]
    pub(super) confidence: f32,
    #[serde(default)]
    pub(super) ttl_days: Option<i64>,
    #[serde(default)]
    pub(super) reason: String,
}

#[derive(Debug, Clone)]
pub(super) struct SanitizedMemoryDecision {
    pub(super) action: String,
    pub(super) suggestion: MemorySuggestion,
    pub(super) confidence: f32,
    pub(super) ttl_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NeverMemoryItem {
    pub id: String,
    pub pattern: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InjectedMemoryItem {
    pub id: String,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMemorySnapshot {
    pub session_id: String,
    pub runtime_path: String,
    pub block: String,
    pub items: Vec<InjectedMemoryItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferenceFile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct TopicMutation<T> {
    pub value: T,
    pub cleanup_warning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TopicRead<T> {
    pub value: T,
    pub cleanup_warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct TopicMigrationJournal {
    pub(super) authority_file: String,
    pub(super) authority_hash: String,
    pub(super) stale_files: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct TopicReconciliation {
    pub(super) hidden_files: BTreeSet<PathBuf>,
    pub(super) cleanup_warning: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct TurnCapture {
    pub(super) user: String,
    pub(super) assistant: String,
    pub(super) tool_summaries: Vec<String>,
    pub(super) delivery_complete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TurnMemoryCapture {
    pub user: String,
    pub assistant: String,
    pub tool_summaries: Vec<String>,
    pub delivery_complete: bool,
}

impl MemoryProfile {
    pub(super) fn normalize(&mut self) {
        self.version = PROFILE_VERSION;
        self.identity.call_name = normalize_profile_label(&self.identity.call_name, "call_name");
        self.identity.assistant_alias =
            normalize_profile_label(&self.identity.assistant_alias, "assistant_alias");
        if invalid_memory_label(&self.identity.call_name) {
            self.identity.call_name.clear();
        }
        if invalid_memory_label(&self.identity.assistant_alias) {
            self.identity.assistant_alias.clear();
        }
        self.conventions.language = clean_scalar(&self.conventions.language);
        self.conventions.doc_standard = clean_scalar(&self.conventions.doc_standard);
        self.conventions.number_usage = clean_scalar(&self.conventions.number_usage);
        self.conventions.style_notes = self
            .conventions
            .style_notes
            .iter()
            .map(|s| clean_scalar(s))
            .filter(|s| !s.is_empty())
            .collect();
        self.pending_sensitive_identity.retain(|_, item| {
            item.value = clean_scalar(&item.value);
            item.source = clean_scalar(&item.source);
            item.status = clean_scalar(&item.status);
            !item.value.is_empty()
        });
    }
}

/// profile 记忆内容前缀清洗（去掉"称呼：/叫我"等命令口吻），再做 label 校验。
pub(super) fn clean_profile_memory_content(content: &str, topic: &str) -> String {
    let mut value = clean_text(content, 80);
    for prefix in [
        "称呼：",
        "称呼:",
        "称呼用户为",
        "称呼用户",
        "用户希望被称呼为",
        "用户希望称呼为",
        "用户希望叫做",
        "用户希望叫",
        "用户希望被叫做",
        "用户希望被叫",
        "用户称呼：",
        "用户称呼:",
        "用户称呼为",
        "叫我",
        "称呼我",
        "助手昵称：",
        "助手昵称:",
        "助手名字：",
        "助手名字:",
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.to_string();
            break;
        }
    }
    if topic == "assistant_alias" {
        for prefix in [
            "我叫你",
            "你的名字叫",
            "叫你",
            "助手昵称叫",
            "助手昵称为",
            "用户希望称呼助手为",
            "用户希望叫助手",
            "用户希望把助手叫做",
            "用户希望把助手称为",
            "助手叫",
            "助手名字叫",
            "助手名字为",
        ] {
            if let Some(rest) = value.strip_prefix(prefix) {
                value = rest.to_string();
                break;
            }
        }
    }
    clean_text(&value, 24)
}

pub(super) fn normalize_profile_label(value: &str, topic: &str) -> String {
    clean_memory_label(&clean_profile_memory_content(value, topic)).unwrap_or_default()
}

pub(super) fn normalize_preference_topic(topic: &str) -> String {
    match clean_id(&clean_text(topic, 40)).as_str() {
        "answer_style" | "output_style" | "output_preference" | "reply_style" => {
            "answer_style".to_string()
        }
        "workflow_preference" | "work_style" | "workflow" | "process" | "collaboration" => {
            "workflow_preference".to_string()
        }
        "document_preference"
        | "doc_preference"
        | "office_style"
        | "report_style"
        | "ppt_style"
        | "document_style" => "document_preference".to_string(),
        _ => "answer_style".to_string(),
    }
}

pub(super) fn normalize_work_context_topic(topic: &str) -> String {
    match clean_id(&clean_text(topic, 40)).as_str() {
        "role_domain" | "role" | "domain" => "role_domain".to_string(),
        "project_context" | "project" | "projects" => "project_context".to_string(),
        "task_pattern" | "task" | "tasks" => "task_pattern".to_string(),
        "tooling_context" | "tooling" | "tools" | "workflow_tooling" => {
            "tooling_context".to_string()
        }
        "output_expectation" | "output" | "deliverable" | "delivery" => {
            "output_expectation".to_string()
        }
        _ => "task_pattern".to_string(),
    }
}

pub(super) fn normalize_timed_memory_kind(kind: &str) -> String {
    match clean_text(kind, 40).as_str() {
        "recent_activity" | "completed_work" => "recent_activity".to_string(),
        "current_focus" | "recent_work" | "current_work" => "current_focus".to_string(),
        _ => "current_focus".to_string(),
    }
}

pub(super) fn normalize_timed_memory_topic(kind: &str, topic: &str) -> String {
    let topic = clean_id(&clean_text(topic, 40));
    if kind == "recent_activity" {
        match topic.as_str() {
            "completed_work" | "delivery" | "delivered" | "recent_activity" => {
                "completed_work".to_string()
            }
            _ => "completed_work".to_string(),
        }
    } else {
        match topic.as_str() {
            "current_work" | "current_focus" | "recent_work" => "current_work".to_string(),
            _ => "current_work".to_string(),
        }
    }
}

pub(super) fn looks_like_profile_preference_text(text: &str) -> bool {
    let text = clean_text(text, 120);
    [
        "称呼用户",
        "用户称呼",
        "用户叫我",
        "助手昵称",
        "助手名字",
        "我怎么称呼助手",
        "用户自称",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}
