//! pinvou3 user memory P0: profile storage + per-session runtime prompt.
//!
//! The structured files are the source of truth. `runtime/<session_id>.md` is
//! only a prompt cache consumed through `InstructionSource::File`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration as StdDuration;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bridge::paths;
use crate::bridge::{
    prefs::{ModelPreset, UserPrefs},
    Pinvou3Bridge,
};

const PROFILE_VERSION: u32 = 1;
const RECENT_WORK_DEFAULT_TTL_DAYS: i64 = 14;
const RECENT_WORK_MAX_INJECTED: usize = 3;
const RECENT_WORK_ACTIVE_MAX_STORED: usize = 8;
const RECENT_WORK_ARCHIVED_MAX_STORED: usize = 24;
const CURRENT_FOCUS_DEFAULT_TTL_DAYS: i64 = 21;
const RECENT_ACTIVITY_DEFAULT_TTL_DAYS: i64 = 14;
const CURRENT_FOCUS_MAX_INJECTED: usize = 3;
const RECENT_ACTIVITY_MAX_INJECTED: usize = 5;
const CURRENT_FOCUS_ACTIVE_MAX_STORED: usize = 8;
const RECENT_ACTIVITY_ACTIVE_MAX_STORED: usize = 20;
const TIMED_MEMORY_ARCHIVED_MAX_STORED: usize = 40;
const PENDING_MEMORY_ACTIVE_MAX_STORED: usize = 20;
const PENDING_MEMORY_RESOLVED_MAX_STORED: usize = 80;
const NEVER_MEMORY_MAX_STORED: usize = 200;
const MEMORY_REVIEW_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
const PENDING_STATUS_OBSERVED: &str = "observed";
const PENDING_STATUS_PENDING: &str = "pending_confirm";
const PENDING_STATUS_CONFIRMED: &str = "confirmed";
const PENDING_STATUS_IGNORED: &str = "ignored";
const LLM_REVIEW_TIMEOUT: StdDuration = StdDuration::from_secs(75);
const LLM_REVIEW_PROMPT: &str = r#"你是 pinvou 的后台记忆整理器。你只做一件事：复盘刚刚这一轮对话，并对照已有记忆，输出是否需要保存、更新或跳过记忆。不要回答用户问题，不要解释你的判断。

你必须只输出 JSON，不要解释。格式：
{
  "items": [
    {
      "action": "skip | pending_confirm | auto_write | auto_update",
      "kind": "profile | preference | work_context | current_focus | recent_activity",
      "topic": "call_name | assistant_alias | answer_style | workflow_preference | document_preference | role_domain | project_context | task_pattern | tooling_context | output_expectation | current_work | completed_work",
      "content": "整理后的完整记忆内容",
      "confidence": 0.0,
      "ttl_days": null,
      "reason": "一句话说明"
    }
  ]
}

你会收到：
- trigger：触发原因，可能是 explicit_user_signal 或 delivery_complete。
- delivery_complete_hint：如果为 true，表示本轮可能完成了交付。你需要评估是否值得形成 recent_activity，但不要为了有交付就硬写记忆。
- current_user_message：本轮用户消息。
- assistant_response：本轮助手回复。
- delivery_summary：本轮交付物相关工具摘要，例如写入文件、修改文件、展示产物。只用于理解交付结果，不要保存工具过程。
- current_memory：当前注入给主模型的记忆摘要，仅作参考；结构化字段优先。
- existing_profile：已生效的用户资料。
- existing_preferences：已生效的长期偏好。
- existing_work_context：已生效的用户工作背景。
- active_current_focus：未过期的当前关注。
- active_recent_activity：未过期的近期动态。
- pending_memory：待用户确认的候选记忆。
- never_memory：用户不希望再提示的记忆。

记忆类别：
- profile：稳定、低敏的用户资料，例如用户希望被如何称呼、用户如何称呼助手。
- preference：长期使用习惯，例如回答风格、工作方式、文档偏好。
- work_context：用户长期工作背景，例如长期角色、领域、项目、任务类型、工具流、交付物期待。它描述用户，不描述 pinvou 的运行环境。
- current_focus：用户最近正在推进、后续短期内可能继续聊的事项，会过期。
- recent_activity：用户最近刚完成的交付、修复、报告、文档或调研，会过期。

topic 规则：
- profile 只使用 call_name 或 assistant_alias。
- preference 只使用 answer_style、workflow_preference、document_preference。
- work_context 只使用 role_domain、project_context、task_pattern、tooling_context、output_expectation。
- current_focus 使用 current_work。
- recent_activity 使用 completed_work。

判断原则：
1. 只记录以后仍然有用的信息。一次性问答、普通闲聊、临时情绪、问题本身、模型猜测都不要记。
2. 记忆必须以用户为中心。不要把 pinvou 当前模型、临时路径、调试状态、工具日志、文件原文当作用户记忆。
3. 不记录密码、手机号、证件号、token、API key、地址等敏感信息。
4. content 必须是清洗后的事实摘要，不要照抄整句，不要包含“请记住/以后你要”等命令口吻。
5. 同一主题已有记忆或 pending_memory 已覆盖时输出 skip。
6. 新信息修正或补充已有记忆时输出 auto_update 或 pending_confirm，并在 content 中给出合并后的完整版本，不要新增重复条目。
7. never_memory 中已有的内容不要再输出。
8. 不确定是否长期稳定时，优先 pending_confirm；不确定是否值得记时，输出 skip。

动作选择：
- skip：没有值得保存的信息，或已有记忆已经覆盖。
- pending_confirm：信息可能有用，但属于长期偏好、工作背景、敏感边界或判断不够确定，需要用户确认。
- auto_write：低敏、高置信、未来明显有用，且不会打扰用户确认也能安全保存。
- auto_update：低敏、高置信，且是对已有同主题记忆的合并或修正。

ttl_days 规则：
- profile / preference / work_context 使用 null。
- current_focus 默认使用 21。
- recent_activity 默认使用 14。

自动写入边界：
- profile 只有在用户非常明确表达，且 confidence >= 0.92 时才允许 auto_write。
- preference 默认 pending_confirm。
- work_context 默认 pending_confirm；只有用户明确要求记住、内容低敏且 confidence >= 0.94 时，才允许 auto_write 或 auto_update。
- current_focus / recent_activity 内容清楚、低敏且 confidence >= 0.86 时，默认使用 auto_write 或 auto_update；只有不确定、较敏感或用户可能不希望记录时才使用 pending_confirm。

近期记忆质量：
- current_focus 要写“用户正在推进什么，以及为什么后续还可能有用”。
- recent_activity 要写“完成了什么、交付物或结果是什么、后续继续该主题时有什么线索”。
- 不要只写“完成了某某某”，也不要记录普通工具过程。
- delivery_complete_hint=true 时要认真评估 recent_activity；如果交付结果清楚、低敏、对未来有用，优先 auto_write，不要仅因为它是近期动态就要求用户确认。

如果没有值得记的内容，输出 {"items":[]}。
"#;

fn write_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
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
struct LlmMemoryReview {
    #[serde(default)]
    items: Vec<LlmMemoryItem>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LlmMemoryItem {
    #[serde(default)]
    action: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    ttl_days: Option<i64>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Clone)]
struct SanitizedMemoryDecision {
    action: String,
    suggestion: MemorySuggestion,
    confidence: f32,
    ttl_days: Option<i64>,
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

#[derive(Debug, Clone, Default)]
struct TurnCapture {
    user: String,
    assistant: String,
    tool_summaries: Vec<String>,
    delivery_complete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TurnMemoryCapture {
    pub user: String,
    pub assistant: String,
    pub tool_summaries: Vec<String>,
    pub delivery_complete: bool,
}

fn turn_capture_store() -> &'static Mutex<BTreeMap<String, TurnCapture>> {
    static STORE: OnceLock<Mutex<BTreeMap<String, TurnCapture>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn memory_review_log_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 写入不含对话原文的记忆复盘诊断信息。日志达到 2 MiB 后从空文件重新开始，
/// 避免后台复盘长期运行无限占用磁盘。
pub(crate) fn append_memory_review_diagnostic(session_id: &str, stage: &str, detail: Value) {
    let _guard = memory_review_log_lock().lock();
    let path = paths::memory_review_log();
    if let Err(err) = append_memory_review_diagnostic_to(&path, session_id, stage, detail) {
        eprintln!(
            "[pinvou3-app] append memory review diagnostic failed ({}): {err}",
            path.display()
        );
    }
}

fn append_memory_review_diagnostic_to(
    path: &Path,
    session_id: &str,
    stage: &str,
    detail: Value,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path)
        .map(|metadata| metadata.len() >= MEMORY_REVIEW_LOG_MAX_BYTES)
        .unwrap_or(false)
    {
        fs::remove_file(path)?;
    }
    let mut line = json!({
        "ts": Utc::now().to_rfc3339(),
        "session_id": clean_id(session_id),
        "stage": clean_text(stage, 48),
        "detail": detail,
    })
    .to_string();
    line.push('\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

fn memory_review_error_stage(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("parse memory review") || message.contains("memory review response json") {
        "parse_failed"
    } else if message.contains("chat/completions")
        || message.contains("memory review client")
        || message.contains("error sending request")
    {
        "request_failed"
    } else {
        "apply_failed"
    }
}

fn default_profile_version() -> u32 {
    PROFILE_VERSION
}

fn default_pending_status() -> String {
    "pending_confirm".to_string()
}

fn default_recent_status() -> String {
    "active".to_string()
}

impl MemoryProfile {
    fn normalize(&mut self) {
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

fn summarize_tool_start(name: &str, input: &Value) -> String {
    let name = clean_text(name, 80);
    if is_delivery_tool_name(&name) {
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

pub fn record_turn_tool_complete(session_id: &str, name: &str, success: bool) {
    let session_id = clean_id(session_id);
    if session_id.is_empty() {
        return;
    }
    let mut store = turn_capture_store().lock();
    let capture = store.entry(session_id).or_default();
    if success && is_delivery_tool_name(name) {
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

fn is_delivery_tool_name(name: &str) -> bool {
    let name = name.trim();
    name == "write_file"
        || name == "append_file"
        || name == "edit_file"
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
    match fs::read_to_string(&path) {
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

pub fn capture_deterministic_memory(message: &str) -> io::Result<Vec<MemoryWriteEvent>> {
    let _ = message;
    Ok(Vec::new())
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

fn clean_memory_label(value: &str) -> Option<String> {
    let cleaned = value
        .trim()
        .trim_matches(|c: char| {
            c.is_ascii_punctuation()
                || matches!(
                    c,
                    '“' | '”' | '‘' | '’' | '《' | '》' | '「' | '」' | '：' | ':'
                )
        })
        .trim_end_matches(|c| matches!(c, '吧' | '呗' | '哦' | '哈' | '啦' | '呀'))
        .trim();
    let cleaned = clean_text(cleaned, 24);
    if cleaned.is_empty() || cleaned.chars().count() > 12 {
        return None;
    }
    if invalid_memory_label(&cleaned) || looks_sensitive_or_task_like(&cleaned) {
        return None;
    }
    Some(cleaned)
}

fn invalid_memory_label(value: &str) -> bool {
    let value = clean_text(value, 24);
    if value.is_empty() {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    matches!(
        value.as_str(),
        "谁" | "什么" | "啥" | "哪位" | "哪个" | "哪里" | "哪儿" | "为什么" | "怎么"
    ) || value.contains('？')
        || value.contains('?')
        || value.ends_with('吗')
        || value.ends_with('呢')
        || lower == "who"
        || lower == "what"
        || lower == "which"
        || lower == "why"
        || lower == "how"
}

fn looks_sensitive_or_task_like(value: &str) -> bool {
    looks_sensitive(value) || looks_task_like(value)
}

fn looks_sensitive(value: &str) -> bool {
    let value = clean_text(value, 500);
    let lower = value.to_ascii_lowercase();
    let digit_count = value.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count >= 11 {
        return true;
    }
    if [
        "身份证",
        "手机号",
        "手机号码",
        "电话号码",
        "联系电话",
        "密码",
        "口令",
        "密钥",
        "私钥",
        "api_key",
        "apikey",
        "api key",
        "secret",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return true;
    }
    if lower.contains("token")
        && (lower.contains('=')
            || lower.contains(':')
            || value.contains('是')
            || value.contains('为'))
    {
        return true;
    }
    looks_like_url(&lower)
        || looks_like_email(&value)
        || looks_like_filesystem_path(&value)
        || looks_like_credential_assignment(&value)
}

fn looks_like_url(lower: &str) -> bool {
    lower.contains("http://") || lower.contains("https://")
}

fn looks_like_email(value: &str) -> bool {
    value
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '，' | '。' | '；' | ';' | ',' | '<' | '>' | '"' | '\'' | '(' | ')' | '[' | ']'
                )
        })
        .any(|token| {
            let Some((left, right)) = token.split_once('@') else {
                return false;
            };
            !left.is_empty() && right.contains('.') && right.len() >= 3
        })
}

fn looks_like_filesystem_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let trimmed = lower.trim();
    let unix_roots = ["/home/", "/tmp/", "/users/", "/var/", "/etc/", "/opt/"];
    if unix_roots
        .iter()
        .any(|root| trimmed.starts_with(root) || lower.contains(root))
    {
        return true;
    }
    trimmed.starts_with("~/")
        || lower.contains("c:\\")
        || lower.contains("c:/")
        || lower.contains("\\users\\")
        || lower.contains("\\appdata\\")
}

fn looks_like_credential_assignment(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let has_assignment =
        lower.contains('=') || lower.contains(':') || value.contains('是') || value.contains('为');
    has_assignment
        && [
            "token", "api_key", "apikey", "api key", "secret", "password", "passwd", "密钥",
            "私钥", "口令", "密码",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn looks_task_like(value: &str) -> bool {
    let text = clean_text(value, 160);
    if text.is_empty() || looks_like_stable_instruction(&text) {
        return false;
    }
    let text = text.trim_start();
    if ["帮我", "请帮我", "麻烦", "麻烦你"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        return contains_one_off_action(text);
    }
    starts_with_one_off_action(text)
}

fn looks_like_stable_instruction(value: &str) -> bool {
    [
        "以后",
        "默认",
        "每次",
        "总是",
        "回答时",
        "回复时",
        "生成报告时",
        "写报告时",
        "做文档时",
        "尽量",
        "不要",
        "别太",
        "优先",
        "习惯",
        "偏好",
        "风格",
        "先给",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn starts_with_one_off_action(value: &str) -> bool {
    [
        "写", "查", "生成", "总结", "翻译", "安装", "打开", "搜索", "创建", "修复", "做", "整理",
        "规划",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}

fn contains_one_off_action(value: &str) -> bool {
    [
        "写", "查", "生成", "总结", "翻译", "安装", "打开", "搜索", "创建", "修复", "做", "整理",
        "规划",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn looks_recent_work_status(value: &str) -> bool {
    looks_ongoing_work_status(value) || looks_completed_work_status(value)
}

fn looks_ongoing_work_status(value: &str) -> bool {
    ["正在", "最近", "本周", "这周", "目前", "推进", "处理中"]
        .iter()
        .any(|needle| value.contains(needle))
}

fn looks_completed_work_status(value: &str) -> bool {
    [
        "刚完成",
        "已完成",
        "完成了",
        "已生成",
        "生成了",
        "已实现",
        "实现了",
        "已修复",
        "修复了",
        "已交付",
        "交付了",
        "写完",
        "整理完",
        "已整理",
        "整理好了",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

pub fn load_recent_work() -> io::Result<Vec<RecentWorkItem>> {
    let path = recent_work_path();
    let raw = match fs::read_to_string(&path) {
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

fn upsert_recent_work_unlocked(patch: RecentWorkPatch) -> io::Result<RecentWorkItem> {
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

pub fn load_work_context() -> io::Result<Vec<WorkContextFile>> {
    let dir = work_context_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut by_topic: BTreeMap<String, WorkContextFile> = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut item) = serde_json::from_str::<WorkContextFile>(&raw) else {
            continue;
        };
        normalize_work_context(&mut item);
        if !item.topic.is_empty() && !item.text.is_empty() {
            by_topic.insert(item.topic.clone(), item);
        }
    }
    Ok(by_topic.into_values().collect())
}

fn upsert_work_context_unlocked(
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

fn upsert_work_context_locked(
    suggestion: &MemorySuggestion,
    confidence: f32,
) -> io::Result<WorkContextFile> {
    let _guard = write_lock().lock();
    upsert_work_context_unlocked(suggestion, confidence)
}

pub fn update_work_context(
    id: &str,
    patch: MemoryTextPatch,
) -> io::Result<Option<WorkContextFile>> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(None);
    }
    let items = load_work_context()?;
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
    if old_path != new_path && old_path.exists() {
        let _ = fs::remove_file(old_path);
    }
    write_json_atomic(&new_path, &updated)?;
    Ok(Some(updated))
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

fn upsert_timed_memory_unlocked(
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

fn upsert_timed_memory_locked(
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

fn archive_timed_memory_unlocked(kind: &str, id: &str) -> io::Result<bool> {
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

fn load_timed_memory_file(path: &Path, kind: &str) -> io::Result<Vec<TimedMemoryItem>> {
    let raw = match fs::read_to_string(path) {
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

fn write_timed_memory_file(path: &Path, items: &[TimedMemoryItem], kind: &str) -> io::Result<()> {
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
    let raw = match fs::read_to_string(&path) {
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

fn confirmed_pending_memory_is_materialized(item: &PendingMemoryItem) -> bool {
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
    let suggestions = discover_turn_suggestions(user);
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

pub async fn review_turn_candidates_with_llm(
    bridge: &Pinvou3Bridge,
    capture: &TurnMemoryCapture,
    session_id: &str,
) -> Result<MemoryReviewOutcome> {
    let user = clean_text(&capture.user, 4000);
    let assistant = clean_text(&capture.assistant, 4000);
    let delivery_summary = capture
        .tool_summaries
        .iter()
        .map(|s| clean_text(s, 600))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let explicit_signal = has_memory_review_signal(&user);
    let delivery_complete =
        capture.delivery_complete || assistant_suggests_delivery_complete(&user, &assistant);
    let skip_reason = if user.is_empty() {
        Some("empty_user_message")
    } else if looks_sensitive(&user) {
        Some("sensitive_content")
    } else if !explicit_signal && !delivery_complete {
        Some("no_review_signal")
    } else {
        None
    };
    if let Some(reason) = skip_reason {
        append_memory_review_diagnostic(
            session_id,
            "skipped",
            json!({
                "reason": reason,
                "user_chars": user.chars().count(),
                "assistant_chars": assistant.chars().count(),
                "tool_summary_count": delivery_summary.len(),
            }),
        );
        return Ok(MemoryReviewOutcome::default());
    }

    let trigger = if delivery_complete {
        "delivery_complete"
    } else {
        "explicit_user_signal"
    };
    append_memory_review_diagnostic(
        session_id,
        "triggered",
        json!({
            "trigger": trigger,
            "provider": bridge.provider(),
            "model": bridge.model(),
            "user_chars": user.chars().count(),
            "assistant_chars": assistant.chars().count(),
            "tool_summary_count": delivery_summary.len(),
        }),
    );
    let review = match request_llm_memory_review(
        bridge,
        &user,
        &assistant,
        trigger,
        &delivery_summary,
    )
    .await
    {
        Ok(review) => review,
        Err(error) => {
            append_memory_review_diagnostic(
                session_id,
                memory_review_error_stage(&error),
                json!({ "error": clean_text(&format!("{error:#}"), 500) }),
            );
            return Err(error);
        }
    };
    let received_items = review.items.len();
    let mut action_counts = BTreeMap::<String, usize>::new();
    for item in &review.items {
        *action_counts
            .entry(clean_text(&item.action, 24))
            .or_default() += 1;
    }
    let outcome = match apply_llm_memory_review(review) {
        Ok(outcome) => outcome,
        Err(error) => {
            append_memory_review_diagnostic(
                session_id,
                memory_review_error_stage(&error),
                json!({ "error": clean_text(&format!("{error:#}"), 500) }),
            );
            return Err(error);
        }
    };
    append_memory_review_diagnostic(
        session_id,
        "completed",
        json!({
            "received_items": received_items,
            "model_action_counts": action_counts,
            "auto_event_count": outcome.events.len(),
            "pending_candidate_count": outcome.pending.len(),
            "result": if outcome.pending.is_empty() && outcome.events.is_empty() {
                "no_memory_change"
            } else if outcome.pending.is_empty() {
                "auto_written"
            } else {
                "candidate_created"
            },
        }),
    );
    Ok(outcome)
}

fn has_memory_review_signal(user: &str) -> bool {
    [
        "记住",
        "以后",
        "之后",
        "叫我",
        "称呼我",
        "我叫你",
        "你的名字",
        "我喜欢",
        "我不喜欢",
        "我偏好",
        "我的习惯",
        "默认",
        "优先",
        "尽量",
        "别太",
        "不要太",
        "长期",
        "经常",
        "负责",
        "参与",
        "最近在",
        "最近",
        "这周",
        "这周在",
        "本周",
        "本周在",
        "目前在",
        "主要在",
        "正在",
        "最近主要",
        "后面还",
        "继续",
    ]
    .iter()
    .any(|needle| user.contains(needle))
}

fn assistant_suggests_delivery_complete(user: &str, assistant: &str) -> bool {
    let user = user.trim();
    let assistant = assistant.trim();
    if user.is_empty() || assistant.is_empty() {
        return false;
    }
    if assistant.chars().count() < 8 || looks_sensitive(assistant) {
        return false;
    }
    let negative = [
        "无法完成",
        "不能完成",
        "没完成",
        "未完成",
        "还没完成",
        "没有完成",
        "无法生成",
        "不能生成",
        "没生成",
        "无法修复",
        "不能修复",
    ];
    if negative.iter().any(|needle| assistant.contains(needle)) {
        return false;
    }
    [
        "已完成",
        "已经完成",
        "完成了",
        "已生成",
        "已经生成",
        "生成了",
        "写好了",
        "整理好了",
        "已整理",
        "已经整理",
        "已实现",
        "已经实现",
        "实现了",
        "已修复",
        "已经修复",
        "修复了",
        "已交付",
        "已经交付",
        "交付了",
        "已更新",
        "已经更新",
        "更新了",
    ]
    .iter()
    .any(|needle| assistant.contains(needle))
}

async fn request_llm_memory_review(
    bridge: &Pinvou3Bridge,
    user: &str,
    assistant: &str,
    trigger: &str,
    delivery_summary: &[String],
) -> Result<LlmMemoryReview> {
    let client = Client::builder()
        .timeout(LLM_REVIEW_TIMEOUT)
        .build()
        .context("build memory review client")?;
    let base_url = bridge.base_url();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let provider = bridge.provider();
    let preset = memory_model_preset(bridge);
    let model_name = if provider == "vllm" {
        crate::features::monitor::probe_vllm_model_info(&base_url)
            .await
            .0
            .unwrap_or_else(|| bridge.model())
    } else {
        bridge.model()
    };
    let current_memory = render_memory_block()
        .map(|(block, _)| block)
        .unwrap_or_default();
    let existing_profile = load_profile().unwrap_or_default();
    let existing_preferences = load_preferences().unwrap_or_default();
    let existing_work_context = load_work_context().unwrap_or_default();
    let focus_items = load_current_focus().unwrap_or_default();
    let active_current_focus = active_timed_memory(&focus_items, Utc::now())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let activity_items = load_recent_activity().unwrap_or_default();
    let active_recent_activity = active_timed_memory(&activity_items, Utc::now())
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let pending = load_pending_memory()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.status == PENDING_STATUS_PENDING)
        .collect::<Vec<_>>();
    let never = load_never_memory().unwrap_or_default();
    let user_content = json!({
        "trigger": trigger,
        "delivery_complete_hint": trigger == "delivery_complete",
        "current_user_message": user,
        "assistant_response": assistant,
        "delivery_summary": delivery_summary,
        "current_memory": current_memory,
        "existing_profile": existing_profile,
        "existing_preferences": existing_preferences,
        "existing_work_context": existing_work_context,
        "active_current_focus": active_current_focus,
        "active_recent_activity": active_recent_activity,
        "pending_memory": pending,
        "never_memory": never,
    })
    .to_string();
    let mut body = json!({
        "model": model_name,
        "messages": [
            { "role": "system", "content": LLM_REVIEW_PROMPT },
            { "role": "user", "content": user_content }
        ],
        "temperature": 0,
        "max_tokens": 900,
        "stream": false,
        "response_format": { "type": "json_object" }
    });
    apply_memory_review_reasoning_controls(&mut body, preset, &provider, &base_url, &model_name);
    let resp = client
        .post(url)
        .bearer_auth(bridge.api_key())
        .json(&body)
        .send()
        .await
        .context("post memory review chat/completions")?
        .error_for_status()
        .context("memory review chat/completions status")?;
    let value: Value = resp
        .json()
        .await
        .context("parse memory review response json")?;
    let content = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_llm_memory_review(content)
}

fn apply_llm_memory_review(review: LlmMemoryReview) -> Result<MemoryReviewOutcome> {
    let mut outcome = MemoryReviewOutcome::default();
    for raw in review.items {
        let Some(decision) = sanitize_llm_memory_item(raw) else {
            continue;
        };
        let suggestion = decision.suggestion;
        if decision.action == "auto_write" && suggestion.kind == "profile" {
            if let Some(event) = auto_write_profile_suggestion(&suggestion)? {
                outcome.events.push(event);
            }
            continue;
        }
        if matches!(
            suggestion.kind.as_str(),
            "current_focus" | "recent_activity"
        ) && matches!(decision.action.as_str(), "auto_write" | "auto_update")
            && decision.confidence >= 0.86
        {
            match upsert_timed_memory_locked(
                &suggestion.kind,
                &suggestion.topic,
                &suggestion.content,
                &suggestion.source,
                decision.ttl_days,
                decision.confidence,
            ) {
                Ok(item) => outcome.events.push(MemoryWriteEvent {
                    kind: item.kind,
                    action: "remembered".to_string(),
                    id: item.id,
                    text: item.text,
                }),
                Err(err) if err.kind() == io::ErrorKind::InvalidInput => {}
                Err(err) => return Err(err).context("auto write timed memory"),
            }
            continue;
        }
        if suggestion.kind == "work_context"
            && matches!(decision.action.as_str(), "auto_write" | "auto_update")
            && decision.confidence >= 0.94
        {
            match upsert_work_context_locked(&suggestion, decision.confidence) {
                Ok(item) => outcome.events.push(MemoryWriteEvent {
                    kind: item.kind,
                    action: "remembered".to_string(),
                    id: item.id,
                    text: item.text,
                }),
                Err(err) if err.kind() == io::ErrorKind::InvalidInput => {}
                Err(err) => return Err(err).context("auto write work context"),
            }
            continue;
        }
        match enqueue_memory_candidate(suggestion) {
            Ok(item) => outcome.pending.push(item),
            Err(err) if err.kind() == io::ErrorKind::InvalidInput => {}
            Err(err) => return Err(err).context("enqueue llm memory candidate"),
        }
    }
    Ok(outcome)
}

fn sanitize_llm_memory_item(raw: LlmMemoryItem) -> Option<SanitizedMemoryDecision> {
    let action = clean_text(&raw.action, 24);
    if action == "skip" || raw.confidence < 0.70 {
        return None;
    }
    if !matches!(
        action.as_str(),
        "pending_confirm" | "auto_write" | "auto_update"
    ) {
        return None;
    }
    let raw_kind = clean_text(&raw.kind, 24);
    let mut kind = match raw_kind.as_str() {
        "profile" => "profile".to_string(),
        "work_context" => "work_context".to_string(),
        "current_focus" => "current_focus".to_string(),
        "recent_activity" => "recent_activity".to_string(),
        "recent_work" => {
            if looks_completed_work_status(&raw.content) {
                "recent_activity".to_string()
            } else {
                "current_focus".to_string()
            }
        }
        _ => "preference".to_string(),
    };
    let mut topic = clean_text(&raw.topic, 40);
    let mut content = clean_candidate_sentence(&raw.content, 180);
    let _reason = clean_text(&raw.reason, 120);
    if content.is_empty() || looks_sensitive(&content) {
        return None;
    }
    if raw_kind == "recent_work" {
        topic = if kind == "recent_activity" {
            "completed_work".to_string()
        } else {
            "current_work".to_string()
        };
    }
    if topic == "call_name" || topic == "assistant_alias" {
        kind = "profile".to_string();
    }
    if kind == "current_focus" && topic == "completed_work" {
        kind = "recent_activity".to_string();
    }
    if kind == "recent_activity" && topic == "current_work" {
        kind = "current_focus".to_string();
    }

    if kind == "profile" {
        topic = match topic.as_str() {
            "assistant_alias" => "assistant_alias".to_string(),
            "call_name" => "call_name".to_string(),
            _ => return None,
        };
        content = clean_memory_label(&clean_profile_memory_content(&content, &topic))?;
        if action == "auto_write" && raw.confidence < 0.92 {
            return None;
        }
    } else if matches!(kind.as_str(), "current_focus" | "recent_activity") {
        if looks_task_like(&content) && !looks_recent_work_status(&content) {
            return None;
        }
        topic = normalize_timed_memory_topic(&kind, &topic);
    } else if kind == "work_context" {
        if content.chars().count() < 8 {
            return None;
        }
        topic = normalize_work_context_topic(&topic);
    } else {
        kind = "preference".to_string();
        if looks_sensitive_or_task_like(&content) || content.chars().count() < 6 {
            return None;
        }
        topic = normalize_preference_topic(&topic);
    }

    Some(SanitizedMemoryDecision {
        action,
        suggestion: MemorySuggestion {
            kind,
            topic,
            content,
            source: "llm_review".to_string(),
        },
        confidence: raw.confidence,
        ttl_days: raw.ttl_days,
    })
}

fn clean_profile_memory_content(content: &str, topic: &str) -> String {
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

fn normalize_profile_label(value: &str, topic: &str) -> String {
    clean_memory_label(&clean_profile_memory_content(value, topic)).unwrap_or_default()
}

fn normalize_preference_topic(topic: &str) -> String {
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

fn auto_write_profile_suggestion(
    suggestion: &MemorySuggestion,
) -> Result<Option<MemoryWriteEvent>> {
    let mut patch = ProfilePatch::default();
    let current = load_profile().context("load profile for auto memory write")?;
    let (id, text) = match suggestion.topic.as_str() {
        "call_name" if suggestion.content != current.identity.call_name => {
            patch.call_name = Some(suggestion.content.clone());
            (
                "profile.call_name".to_string(),
                format!("称呼：{}", suggestion.content),
            )
        }
        "assistant_alias" if suggestion.content != current.identity.assistant_alias => {
            patch.assistant_alias = Some(suggestion.content.clone());
            (
                "profile.assistant_alias".to_string(),
                format!("助手昵称：{}", suggestion.content),
            )
        }
        _ => return Ok(None),
    };
    update_profile(patch).context("auto write profile memory")?;
    Ok(Some(MemoryWriteEvent {
        kind: "profile".to_string(),
        action: "remembered".to_string(),
        id,
        text,
    }))
}

fn parse_llm_memory_review(content: &str) -> Result<LlmMemoryReview> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(LlmMemoryReview::default());
    }
    match serde_json::from_str::<LlmMemoryReview>(trimmed) {
        Ok(review) => Ok(review),
        Err(first_err) => {
            let Some(json_text) = extract_json_object(trimmed) else {
                return Err(first_err).context("parse memory review json");
            };
            serde_json::from_str::<LlmMemoryReview>(json_text)
                .context("parse extracted memory review json")
        }
    }
}

fn extract_json_object(value: &str) -> Option<&str> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    (start <= end).then(|| &value[start..=end])
}

fn memory_model_preset(bridge: &Pinvou3Bridge) -> ModelPreset {
    bridge
        .effective_model_owned()
        .map(|m| m.preset)
        .unwrap_or_else(|| bridge.prefs.advanced.model_preset.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryReviewReasoningDialect {
    None,
    ThinkingDisabled,
    QwenEnableThinking,
    VllmChatTemplate,
    Minimax,
}

fn apply_memory_review_reasoning_controls(
    body: &mut Value,
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) {
    match memory_review_reasoning_dialect(preset, provider, base_url, model) {
        MemoryReviewReasoningDialect::ThinkingDisabled => {
            body["thinking"] = json!({ "type": "disabled" });
        }
        MemoryReviewReasoningDialect::QwenEnableThinking => {
            body["enable_thinking"] = json!(false);
        }
        MemoryReviewReasoningDialect::VllmChatTemplate => {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        MemoryReviewReasoningDialect::Minimax => {
            body["thinking"] = json!({ "type": "disabled" });
            body["reasoning_split"] = json!(true);
        }
        MemoryReviewReasoningDialect::None => {}
    }
}

fn memory_review_reasoning_dialect(
    preset: ModelPreset,
    provider: &str,
    base_url: &str,
    model: &str,
) -> MemoryReviewReasoningDialect {
    if provider == "vllm" || preset == ModelPreset::LocalVllm {
        return MemoryReviewReasoningDialect::VllmChatTemplate;
    }
    if provider == "deepseek" || preset == ModelPreset::Deepseek {
        return MemoryReviewReasoningDialect::ThinkingDisabled;
    }
    match preset {
        ModelPreset::Qwen => MemoryReviewReasoningDialect::QwenEnableThinking,
        ModelPreset::Doubao | ModelPreset::Glm | ModelPreset::Mimo => {
            MemoryReviewReasoningDialect::ThinkingDisabled
        }
        ModelPreset::Minimax => MemoryReviewReasoningDialect::Minimax,
        ModelPreset::Kimi => {
            if model.contains("k2.6") || model.contains("kimi-k2") {
                MemoryReviewReasoningDialect::ThinkingDisabled
            } else {
                MemoryReviewReasoningDialect::None
            }
        }
        ModelPreset::OpenaiCompatible | ModelPreset::LocalVllm | ModelPreset::Deepseek => {
            memory_review_reasoning_dialect_from_base_url(base_url, model)
        }
    }
}

fn memory_review_reasoning_dialect_from_base_url(
    base_url: &str,
    model: &str,
) -> MemoryReviewReasoningDialect {
    let normalized = base_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .to_ascii_lowercase();
    let model = model.to_ascii_lowercase();
    if normalized.contains("dashscope.aliyuncs.com") || model.contains("qwen") {
        MemoryReviewReasoningDialect::QwenEnableThinking
    } else if normalized.contains("deepseek.com") || model.contains("deepseek") {
        MemoryReviewReasoningDialect::ThinkingDisabled
    } else {
        MemoryReviewReasoningDialect::None
    }
}

fn discover_turn_suggestions(user: &str) -> Vec<MemorySuggestion> {
    let text = clean_text(user, 500);
    if text.is_empty() || looks_sensitive(&text) {
        return Vec::new();
    }

    let clauses: Vec<String> = text
        .split(|c| matches!(c, '。' | '！' | '？' | '；' | ';' | '\n'))
        .map(|s| clean_text(s, 160))
        .filter(|s| !s.is_empty())
        .collect();

    let mut out = Vec::new();
    for clause in clauses {
        if let Some(content) = preference_candidate_text(&clause) {
            out.push(MemorySuggestion {
                kind: "preference".to_string(),
                topic: "output_preference".to_string(),
                content,
                source: "auto_review".to_string(),
            });
            continue;
        }
        if let Some(content) = recent_work_candidate_text(&clause) {
            out.push(MemorySuggestion {
                kind: "recent_work".to_string(),
                topic: "current_work".to_string(),
                content,
                source: "auto_review".to_string(),
            });
        }
    }
    out
}

fn preference_candidate_text(clause: &str) -> Option<String> {
    let has_trigger = [
        "我喜欢",
        "我不喜欢",
        "我偏好",
        "我的习惯",
        "以后回答",
        "以后默认",
        "以后都",
        "每次都",
        "尽量",
        "不要太",
        "别太",
        "默认用",
        "优先用",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    if !has_trigger || looks_sensitive_or_task_like(clause) {
        return None;
    }
    let content = clean_candidate_sentence(clause, 80);
    if content.chars().count() < 6 {
        return None;
    }
    Some(content)
}

fn recent_work_candidate_text(clause: &str) -> Option<String> {
    let has_time_trigger = [
        "最近在",
        "这周在",
        "本周在",
        "目前在",
        "正在",
        "继续",
        "上次那个",
        "最近主要",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    let has_work_noun = [
        "项目", "材料", "报告", "PPT", "方案", "文档", "工作", "任务", "周报", "汇报",
    ]
    .iter()
    .any(|needle| clause.contains(needle));
    if !has_time_trigger || !has_work_noun || looks_sensitive(clause) {
        return None;
    }
    let content = clean_candidate_sentence(clause, 60);
    if content.chars().count() < 6 {
        return None;
    }
    Some(content)
}

fn clean_candidate_sentence(value: &str, max_chars: usize) -> String {
    let cleaned = value
        .trim()
        .trim_start_matches("请记住")
        .trim_start_matches("记住")
        .trim_start_matches("你要记住")
        .trim_matches(|c: char| {
            c.is_ascii_punctuation()
                || matches!(
                    c,
                    '“' | '”' | '‘' | '’' | '《' | '》' | '「' | '」' | '：' | ':'
                )
        });
    clean_text(cleaned, max_chars)
}

pub fn refresh_recent_work_expiry() -> io::Result<usize> {
    let _guard = write_lock().lock();
    let now = Utc::now();
    Ok(refresh_recent_work_expiry_unlocked(now)?
        + refresh_timed_memory_expiry_unlocked("current_focus", now)?
        + refresh_timed_memory_expiry_unlocked("recent_activity", now)?)
}

fn refresh_recent_work_expiry_unlocked(now: DateTime<Utc>) -> io::Result<usize> {
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

fn refresh_timed_memory_expiry_unlocked(kind: &str, now: DateTime<Utc>) -> io::Result<usize> {
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

pub fn render_memory_block() -> io::Result<(String, Vec<InjectedMemoryItem>)> {
    let profile = load_profile()?;
    let preferences = load_preferences().unwrap_or_default();
    let work_context = load_work_context().unwrap_or_default();
    let current_focus = load_current_focus().unwrap_or_default();
    let recent_activity = load_recent_activity().unwrap_or_default();
    let legacy_recent_work = load_recent_work().unwrap_or_default();
    Ok(render_from_parts(
        &profile,
        &preferences,
        &work_context,
        &current_focus,
        &recent_activity,
        &legacy_recent_work,
        Utc::now(),
    ))
}

pub fn memory_enabled() -> bool {
    UserPrefs::load().memory_enabled
}

fn disabled_runtime_snapshot(session_id: &str) -> io::Result<RuntimeMemorySnapshot> {
    let path = runtime_prompt_path(session_id);
    write_text_atomic(&path, "")?;
    Ok(RuntimeMemorySnapshot {
        session_id: session_id.to_string(),
        runtime_path: path.display().to_string(),
        block: String::new(),
        items: Vec::new(),
    })
}

pub fn refresh_runtime_prompt(session_id: &str) -> io::Result<RuntimeMemorySnapshot> {
    let _guard = write_lock().lock();
    if !memory_enabled() {
        return disabled_runtime_snapshot(session_id);
    }
    let now = Utc::now();
    let _ = refresh_recent_work_expiry_unlocked(now)?;
    let _ = refresh_timed_memory_expiry_unlocked("current_focus", now)?;
    let _ = refresh_timed_memory_expiry_unlocked("recent_activity", now)?;
    let (block, items) = render_memory_block()?;
    let path = runtime_prompt_path(session_id);
    write_text_atomic(&path, &block)?;
    Ok(RuntimeMemorySnapshot {
        session_id: session_id.to_string(),
        runtime_path: path.display().to_string(),
        block,
        items,
    })
}

pub fn ensure_runtime_prompt(session_id: &str) -> io::Result<PathBuf> {
    let path = runtime_prompt_path(session_id);
    if !path.exists() {
        let _ = refresh_runtime_prompt(session_id)?;
    }
    Ok(path)
}

pub fn runtime_snapshot(session_id: &str) -> io::Result<RuntimeMemorySnapshot> {
    refresh_runtime_prompt(session_id)
}

fn render_from_parts(
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    legacy_recent_work: &[RecentWorkItem],
    now: DateTime<Utc>,
) -> (String, Vec<InjectedMemoryItem>) {
    let mut items = Vec::new();
    let mut profile_lines = Vec::new();

    if !profile.identity.call_name.is_empty() {
        profile_lines.push(format!("- 称呼：{}", profile.identity.call_name));
        items.push(InjectedMemoryItem {
            id: "profile.call_name".to_string(),
            kind: "profile".to_string(),
            text: format!("称呼：{}", profile.identity.call_name),
        });
    }
    if !profile.identity.assistant_alias.is_empty() {
        profile_lines.push(format!("- 助手昵称：{}", profile.identity.assistant_alias));
        items.push(InjectedMemoryItem {
            id: "profile.assistant_alias".to_string(),
            kind: "profile".to_string(),
            text: format!("助手昵称：{}", profile.identity.assistant_alias),
        });
    }

    let mut habits = Vec::new();
    push_if_present(&mut habits, &profile.conventions.language);
    if !profile.conventions.doc_standard.is_empty() {
        habits.push(format!("公文格式遵 {}", profile.conventions.doc_standard));
    }
    if !profile.conventions.number_usage.is_empty() {
        habits.push(format!("数字用法遵 {}", profile.conventions.number_usage));
    }
    habits.extend(profile.conventions.style_notes.iter().cloned());
    if !habits.is_empty() {
        let text = habits.join("；");
        profile_lines.push(format!("- 输出习惯：{text}"));
        items.push(InjectedMemoryItem {
            id: "profile.conventions".to_string(),
            kind: "profile".to_string(),
            text: format!("输出习惯：{text}"),
        });
    }

    let mut preference_lines = Vec::new();
    for pref in preferences
        .iter()
        .filter(|p| p.scope == "unconditional" && !p.text.is_empty())
        .take(20)
    {
        preference_lines.push(format!("- {}", pref.text));
        items.push(InjectedMemoryItem {
            id: if pref.id.is_empty() {
                format!("preference.{}", pref.topic)
            } else {
                pref.id.clone()
            },
            kind: "preference".to_string(),
            text: pref.text.clone(),
        });
    }

    let mut work_context_lines = Vec::new();
    for ctx in work_context
        .iter()
        .filter(|item| !item.text.is_empty())
        .take(5)
    {
        work_context_lines.push(format!("- {}", ctx.text));
        items.push(InjectedMemoryItem {
            id: if ctx.id.is_empty() {
                format!("work_context.{}", ctx.topic)
            } else {
                ctx.id.clone()
            },
            kind: "work_context".to_string(),
            text: ctx.text.clone(),
        });
    }

    let mut focus_lines = Vec::new();
    for item in active_timed_memory(current_focus, now)
        .into_iter()
        .take(CURRENT_FOCUS_MAX_INJECTED)
    {
        focus_lines.push(format!("- {}", item.text));
        items.push(InjectedMemoryItem {
            id: format!("current_focus.{}", item.id),
            kind: "current_focus".to_string(),
            text: item.text.clone(),
        });
    }

    let mut activity_lines = Vec::new();
    for item in active_timed_memory(recent_activity, now)
        .into_iter()
        .take(RECENT_ACTIVITY_MAX_INJECTED)
    {
        activity_lines.push(format!("- {}", item.text));
        items.push(InjectedMemoryItem {
            id: format!("recent_activity.{}", item.id),
            kind: "recent_activity".to_string(),
            text: item.text.clone(),
        });
    }

    let mut recent_lines = Vec::new();
    for item in active_recent_work(legacy_recent_work, now)
        .into_iter()
        .take(RECENT_WORK_MAX_INJECTED)
    {
        let text = if item.summary.is_empty() {
            format!("正在处理：{}", item.title)
        } else {
            format!("正在处理：{}（{}）", item.title, item.summary)
        };
        recent_lines.push(format!("- {text}"));
        items.push(InjectedMemoryItem {
            id: format!("recent_work.{}", item.id),
            kind: "current_focus".to_string(),
            text,
        });
    }

    if profile_lines.is_empty()
        && preference_lines.is_empty()
        && work_context_lines.is_empty()
        && focus_lines.is_empty()
        && activity_lines.is_empty()
        && recent_lines.is_empty()
    {
        return (String::new(), items);
    }

    let mut block =
        String::from("<pinvou_user_memory>\n权威层级：低于用户当前指令；与本轮冲突以本轮为准。\n");
    if !profile_lines.is_empty() {
        block.push_str("画像：\n");
        block.push_str(&profile_lines.join("\n"));
        block.push('\n');
    }
    if !preference_lines.is_empty() {
        block.push_str("长期偏好：\n");
        block.push_str(&preference_lines.join("\n"));
        block.push('\n');
    }
    if !work_context_lines.is_empty() {
        block.push_str("工作背景：\n");
        block.push_str(&work_context_lines.join("\n"));
        block.push('\n');
    }
    if !focus_lines.is_empty() {
        block.push_str("当前关注（会过期）：\n");
        block.push_str(&focus_lines.join("\n"));
        block.push('\n');
    }
    if !activity_lines.is_empty() {
        block.push_str("近期动态（会过期）：\n");
        block.push_str(&activity_lines.join("\n"));
        block.push('\n');
    }
    if !recent_lines.is_empty() {
        block.push_str("当前关注（兼容旧近期工作，会过期）：\n");
        block.push_str(&recent_lines.join("\n"));
        block.push('\n');
    }
    block.push_str("</pinvou_user_memory>\n");
    (block, items)
}

pub fn write_memory_snapshot_document(
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    recent_work: &[RecentWorkItem],
    pending: &[PendingMemoryItem],
    never: &[NeverMemoryItem],
    runtime: Option<&RuntimeMemorySnapshot>,
) -> io::Result<PathBuf> {
    let path = snapshot_path();
    let generated_at = Utc::now().to_rfc3339();
    let doc = render_memory_snapshot_document(
        &generated_at,
        profile,
        preferences,
        work_context,
        current_focus,
        recent_activity,
        recent_work,
        pending,
        never,
        runtime,
    )?;
    write_text_atomic(&path, &doc)?;
    Ok(path)
}

fn render_memory_snapshot_document(
    generated_at: &str,
    profile: &MemoryProfile,
    preferences: &[PreferenceFile],
    work_context: &[WorkContextFile],
    current_focus: &[TimedMemoryItem],
    recent_activity: &[TimedMemoryItem],
    recent_work: &[RecentWorkItem],
    pending: &[PendingMemoryItem],
    never: &[NeverMemoryItem],
    runtime: Option<&RuntimeMemorySnapshot>,
) -> io::Result<String> {
    let mut doc = String::new();
    let _ = writeln!(&mut doc, "# PINVOU 设备记忆快照");
    let _ = writeln!(&mut doc);
    let _ = writeln!(&mut doc, "- 生成时间：{generated_at}");
    let _ = writeln!(
        &mut doc,
        "- 来源目录：{}",
        paths::user_memory_dir().display()
    );
    let _ = writeln!(
        &mut doc,
        "- 说明：本文件由“同步记忆”生成，仅用于查看、迁移排查和调试；结构化记忆文件仍是事实源。"
    );
    let _ = writeln!(
        &mut doc,
        "- 注意：`_pending`、`_never` 和 `runtime` 不会作为长期记忆直接注入模型。"
    );

    let _ = writeln!(&mut doc, "\n## 运行时注入摘要");
    if let Some(snapshot) = runtime {
        if snapshot.block.trim().is_empty() {
            let _ = writeln!(&mut doc, "当前没有可注入的有效记忆。");
        } else {
            let _ = writeln!(&mut doc, "```text\n{}```", snapshot.block);
        }
        let _ = writeln!(&mut doc, "- runtime 文件：{}", snapshot.runtime_path);
    } else {
        let _ = writeln!(&mut doc, "当前没有绑定 session，未生成运行时注入摘要。");
    }

    let _ = writeln!(&mut doc, "\n## 长期记忆");
    let _ = writeln!(&mut doc, "\n### 用户画像");
    push_snapshot_line(&mut doc, "用户称呼", &profile.identity.call_name);
    push_snapshot_line(&mut doc, "助手昵称", &profile.identity.assistant_alias);
    push_snapshot_line(&mut doc, "默认语言", &profile.conventions.language);
    push_snapshot_line(&mut doc, "文档标准", &profile.conventions.doc_standard);
    push_snapshot_line(&mut doc, "数字用法", &profile.conventions.number_usage);
    if !profile.conventions.style_notes.is_empty() {
        for note in &profile.conventions.style_notes {
            push_snapshot_line(&mut doc, "输出习惯", note);
        }
    }
    if profile.identity.call_name.is_empty()
        && profile.identity.assistant_alias.is_empty()
        && profile.conventions.language.is_empty()
        && profile.conventions.doc_standard.is_empty()
        && profile.conventions.number_usage.is_empty()
        && profile.conventions.style_notes.is_empty()
    {
        let _ = writeln!(&mut doc, "暂无用户画像。");
    }

    let _ = writeln!(&mut doc, "\n### 长期偏好");
    if preferences.is_empty() {
        let _ = writeln!(&mut doc, "暂无长期偏好。");
    } else {
        for item in preferences {
            let _ = writeln!(
                &mut doc,
                "- [{}] {}",
                snapshot_one_line(&item.topic),
                snapshot_one_line(&item.text)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n### 工作背景");
    if work_context.is_empty() {
        let _ = writeln!(&mut doc, "暂无工作背景。");
    } else {
        for item in work_context {
            let _ = writeln!(
                &mut doc,
                "- [{}] {}（置信度：{:.2}，来源：{}）",
                snapshot_one_line(&item.topic),
                snapshot_one_line(&item.text),
                item.confidence,
                snapshot_optional(&item.source)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n## 近期记忆");
    let _ = writeln!(&mut doc, "\n### 当前关注");
    if current_focus.is_empty() {
        let _ = writeln!(&mut doc, "暂无当前关注。");
    } else {
        for item in current_focus {
            push_timed_snapshot_line(&mut doc, item);
        }
    }

    let _ = writeln!(&mut doc, "\n### 近期动态");
    if recent_activity.is_empty() {
        let _ = writeln!(&mut doc, "暂无近期动态。");
    } else {
        for item in recent_activity {
            push_timed_snapshot_line(&mut doc, item);
        }
    }

    let _ = writeln!(&mut doc, "\n### 兼容旧近期工作");
    if recent_work.is_empty() {
        let _ = writeln!(&mut doc, "暂无旧近期工作。");
    } else {
        for item in recent_work {
            let summary = if item.summary.is_empty() {
                String::new()
            } else {
                format!("：{}", snapshot_one_line(&item.summary))
            };
            let _ = writeln!(
                &mut doc,
                "- [{}] {}{}（更新：{}，过期：{}）",
                snapshot_one_line(&item.status),
                snapshot_one_line(&item.title),
                summary,
                snapshot_optional(&item.updated_at),
                snapshot_optional(&item.expires_at)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n## 管理数据");
    let _ = writeln!(&mut doc, "\n### 待确认候选（不注入模型）");
    if pending.is_empty() {
        let _ = writeln!(&mut doc, "暂无待确认候选。");
    } else {
        for item in pending {
            let _ = writeln!(
                &mut doc,
                "- [{} / {}] {}",
                snapshot_one_line(&item.status),
                snapshot_one_line(&item.kind),
                snapshot_one_line(&item.content)
            );
        }
    }

    let _ = writeln!(&mut doc, "\n### 不再提示（不注入模型）");
    if never.is_empty() {
        let _ = writeln!(&mut doc, "暂无不再提示记录。");
    } else {
        for item in never {
            let _ = writeln!(
                &mut doc,
                "- {}（原因：{}）",
                snapshot_one_line(&item.pattern),
                snapshot_optional(&item.reason)
            );
        }
    }

    let raw = json!({
        "schema": "pinvou-memory-snapshot/v1",
        "generated_at": generated_at,
        "source_dir": paths::user_memory_dir().display().to_string(),
        "files": {
            "profile": profile_path().display().to_string(),
            "preferences": paths::user_memory_preferences_dir().display().to_string(),
            "work_context": work_context_dir().display().to_string(),
            "current_focus": current_focus_path().display().to_string(),
            "recent_activity": recent_activity_path().display().to_string(),
            "recent_work": recent_work_path().display().to_string(),
            "pending": pending_memory_path().display().to_string(),
            "never": never_memory_path().display().to_string(),
            "runtime_dir": paths::user_memory_runtime_dir().display().to_string()
        },
        "profile": profile,
        "preferences": preferences,
        "work_context": work_context,
        "current_focus": current_focus,
        "recent_activity": recent_activity,
        "recent_work": recent_work,
        "pending": pending,
        "never": never,
        "runtime": runtime
    });
    let raw = serde_json::to_string_pretty(&raw).map_err(invalid_data)?;
    let _ = writeln!(&mut doc, "\n## 结构化快照");
    let _ = writeln!(&mut doc, "~~~json\n{raw}\n~~~");
    Ok(doc)
}

fn push_snapshot_line(doc: &mut String, label: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    let _ = writeln!(
        doc,
        "- **{}**：{}",
        snapshot_one_line(label),
        snapshot_one_line(value)
    );
}

fn push_timed_snapshot_line(doc: &mut String, item: &TimedMemoryItem) {
    let _ = writeln!(
        doc,
        "- [{} / {} / {}天] {}（更新：{}，来源：{}）",
        snapshot_one_line(&item.status),
        snapshot_one_line(&item.topic),
        item.ttl_days,
        snapshot_one_line(&item.text),
        snapshot_optional(&item.updated_at),
        snapshot_optional(&item.source)
    );
}

fn snapshot_one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn snapshot_optional(value: &str) -> String {
    let value = snapshot_one_line(value);
    if value.is_empty() {
        "无".to_string()
    } else {
        value
    }
}

fn active_recent_work(items: &[RecentWorkItem], now: DateTime<Utc>) -> Vec<&RecentWorkItem> {
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

fn recent_work_is_expired(item: &RecentWorkItem, now: DateTime<Utc>) -> bool {
    if parse_time(&item.expires_at).map_or(false, |expires_at| expires_at <= now) {
        return true;
    }
    parse_time(&item.last_hit)
        .map(|last_hit| {
            now.signed_duration_since(last_hit) > Duration::days(RECENT_WORK_DEFAULT_TTL_DAYS)
        })
        .unwrap_or(false)
}

fn write_recent_work_unlocked(items: &[RecentWorkItem]) -> io::Result<()> {
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

fn normalize_work_context_topic(topic: &str) -> String {
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

fn normalize_timed_memory_kind(kind: &str) -> String {
    match clean_text(kind, 40).as_str() {
        "recent_activity" | "completed_work" => "recent_activity".to_string(),
        "current_focus" | "recent_work" | "current_work" => "current_focus".to_string(),
        _ => "current_focus".to_string(),
    }
}

fn normalize_timed_memory_topic(kind: &str, topic: &str) -> String {
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

fn active_timed_memory(items: &[TimedMemoryItem], now: DateTime<Utc>) -> Vec<&TimedMemoryItem> {
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

fn pending_item_from_suggestion(suggestion: MemorySuggestion) -> io::Result<PendingMemoryItem> {
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

fn write_pending_memory_unlocked(items: &[PendingMemoryItem]) -> io::Result<()> {
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

fn load_never_memory_unlocked() -> io::Result<Vec<NeverMemoryItem>> {
    let path = never_memory_path();
    let raw = match fs::read_to_string(&path) {
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

fn write_never_memory_unlocked(items: &[NeverMemoryItem]) -> io::Result<()> {
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

pub fn list_preferences() -> io::Result<Vec<PreferenceFile>> {
    load_preferences()
}

pub fn update_preference(id: &str, patch: MemoryTextPatch) -> io::Result<Option<PreferenceFile>> {
    let _guard = write_lock().lock();
    let id = clean_id(id);
    if id.is_empty() {
        return Ok(None);
    }
    let prefs = load_preferences()?;
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
    if old_path != new_path && old_path.exists() {
        let _ = fs::remove_file(old_path);
    }
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == new_path || path.extension().and_then(|s| s.to_str()) != Some("json") {
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
    write_json_atomic(&new_path, &updated)?;
    Ok(Some(updated))
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

fn load_preferences() -> io::Result<Vec<PreferenceFile>> {
    let dir = paths::user_memory_preferences_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut by_topic: BTreeMap<String, PreferenceFile> = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut pref) = serde_json::from_str::<PreferenceFile>(&raw) else {
            continue;
        };
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
            by_topic.insert(pref.topic.clone(), pref);
        }
    }
    let mut out = by_topic.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.text.cmp(&b.text)));
    Ok(out)
}

fn looks_like_profile_preference_text(text: &str) -> bool {
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

fn push_if_present(out: &mut Vec<String>, value: &str) {
    if !value.is_empty() {
        out.push(value.to_string());
    }
}

fn clean_scalar(value: &str) -> String {
    clean_text(value, 200)
}

fn clean_text(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

fn clean_id(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(80)
        .collect()
}

fn stable_id_from_text(value: &str) -> String {
    stable_id_with_prefix("rw", value)
}

fn stable_id_with_prefix(prefix: &str, value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{}_{hash:016x}", clean_id(prefix))
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(invalid_data)?;
    write_text_atomic(path, &(text + "\n"))
}

fn write_text_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => return Ok(()),
        Err(err) if path.exists() => {
            let _ = fs::remove_file(path);
            fs::rename(tmp, path).map_err(|rename_err| {
                io::Error::new(
                    rename_err.kind(),
                    format!("replace after rename failed ({err}); {rename_err}"),
                )
            })?;
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

fn invalid_data(err: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct IsolatedPinvouHome {
        root: PathBuf,
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl IsolatedPinvouHome {
        fn new(name: &str) -> Self {
            let guard = crate::bridge::paths::tests::ENV_LOCK
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
        let path = write_memory_snapshot_document(
            &profile,
            &preferences,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            None,
        )
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
        let suggestions = discover_turn_suggestions(
            "以后回答默认先给结论，再给步骤。这周在做营商环境推进会材料。",
        );
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
            content:
                "推进公司人力资源手册更新，重点调整章节结构，计划新增数据合规、灵活用工等章节。"
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
        assert!(
            pending.len() <= PENDING_MEMORY_ACTIVE_MAX_STORED + PENDING_MEMORY_RESOLVED_MAX_STORED
        );

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
}
