//! PinvouOS ASR Context Agent.
//!
//! 这个 Agent 不在语音关键路径调用模型。它把连续运行时、最近用户输入和本机私有
//! 词表编译成一个有界快照；Qwen3-ASR 每次识别只读取该快照。新的 Memory 稳定只读
//! Context Projection 尚未接入，旧兼容 MemoryAgent 不能作为来源。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{CapabilityContract, Interruptibility, ResourceClass, RuntimeSnapshot};
use super::runtime::PinvouOsRuntime;

pub const ASR_CONTEXT_AGENT_ID: &str = "agent:asr-context";
pub const ASR_CONTEXT_CAPABILITY_ID: &str = "voice.context.compile";
pub const ASR_CONTEXT_MAX_TERMS: usize = 100;
pub const ASR_CONTEXT_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

const STATE_SCHEMA_VERSION: u32 = 1;
const MAX_OBSERVED_TERMS: usize = 512;
const MAX_TERM_CHARS: usize = 64;
const MAX_CONTEXT_CHARS: usize = 8_192;
const MAX_PRIVATE_LEXICON_BYTES: u64 = 64 * 1024;

// 产品与本地开发环境的公共基础词。个人姓名、联系人和客户词不得写入代码；它们
// 来自 Memory Agent 或 ~/.pinvou3/pinvou-os/asr-lexicon.txt。
const BASE_TERMS: &[&str] = &[
    "Pinvou",
    "PinvouOS",
    "MegaBook",
    "MegaCube",
    "Qwen",
    "Qwen3-ASR",
    "OpenVINO",
    "OpenVINO GenAI",
    "GLM",
    "Agent",
    "Runtime",
    "Context",
    "Context Compiler",
    "ASR",
    "LLM",
    "AI",
    "Front Agent",
    "Orchestrator Agent",
    "Surface Agent",
    "Resource Agent",
    "Device Agent",
    "Capability Agent",
    "Memory Agent",
    "Policy Agent",
    "Attention Agent",
    "ASR Context Agent",
    "World State",
    "Mission",
    "Run",
    "Claim",
    "Directive",
    "GPU",
    "CPU",
    "iGPU",
    "NPU",
    "Intel Arc",
    "Ubuntu",
    "Linux",
    "Windows",
    "Rust",
    "Tauri",
    "React",
    "JavaScript",
    "TypeScript",
    "Python",
    "Cargo",
    "Node.js",
    "Vite",
    "WebView",
    "WebKitGTK",
    "systemd",
    "SSH",
    "Git",
    "GitHub",
    "API",
    "MCP",
    "Obsidian",
    "Token",
    "Prompt",
    "JSON",
    "JSONL",
    "REST",
    "HTTP",
    "WebSocket",
    "Wi-Fi",
    "Bluetooth",
    "microphone",
    "speech recognition",
    "transcription",
    "hotword",
    "keyterm",
    "INT8",
    "INT4",
    "FP16",
];

const ENGLISH_STOPWORDS: &[&str] = &[
    "about", "after", "again", "also", "and", "are", "because", "been", "before", "being", "but",
    "can", "could", "did", "does", "doing", "done", "for", "from", "had", "has", "have", "here",
    "how", "into", "its", "just", "like", "more", "most", "not", "now", "of", "on", "only", "or",
    "our", "please", "should", "some", "than", "that", "the", "their", "them", "then", "there",
    "these", "they", "this", "those", "through", "to", "under", "use", "using", "very", "was",
    "we", "were", "what", "when", "where", "which", "will", "with", "would", "you", "your",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AsrContextTerm {
    pub text: String,
    pub sources: Vec<String>,
    pub score: i64,
    pub english: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AsrContextSnapshot {
    pub revision: u64,
    pub refreshed_at_ms: i64,
    pub next_refresh_at_ms: i64,
    pub max_terms: usize,
    pub term_count: usize,
    pub english_term_count: usize,
    pub context_text: String,
    pub terms: Vec<AsrContextTerm>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ObservedTerm {
    text: String,
    count: u32,
    last_observed_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    schema_version: u32,
    revision: u64,
    #[serde(default)]
    observed_terms: BTreeMap<String, ObservedTerm>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<AsrContextSnapshot>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            revision: 0,
            observed_terms: BTreeMap::new(),
            snapshot: None,
        }
    }
}

struct AsrContextInner {
    state_path: PathBuf,
    private_lexicon_path: PathBuf,
    state: RwLock<PersistedState>,
}

#[derive(Clone)]
pub struct AsrContextAgent {
    inner: Arc<AsrContextInner>,
}

impl AsrContextAgent {
    pub fn boot(state_path: PathBuf, private_lexicon_path: PathBuf) -> Result<Self> {
        let parent = state_path
            .parent()
            .context("ASR context state path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("create ASR context directory {}", parent.display()))?;
        super::platform::harden_private_runtime_dir(parent)
            .with_context(|| format!("protect ASR context directory {}", parent.display()))?;

        let state = match fs::read(&state_path) {
            Ok(raw) => match serde_json::from_slice::<PersistedState>(&raw) {
                Ok(state) if state.schema_version == STATE_SCHEMA_VERSION => state,
                Ok(_) => {
                    log::warn!("ignoring ASR context state with unsupported schema");
                    PersistedState::default()
                }
                Err(error) => {
                    log::warn!("ignoring invalid ASR context state: {error}");
                    PersistedState::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read ASR context state {}", state_path.display()))
            }
        };

        Ok(Self {
            inner: Arc::new(AsrContextInner {
                state_path,
                private_lexicon_path,
                state: RwLock::new(state),
            }),
        })
    }

    pub fn current_snapshot(&self) -> Option<AsrContextSnapshot> {
        self.inner.state.read().snapshot.clone()
    }

    pub fn current_context(&self) -> String {
        self.current_snapshot()
            .map(|snapshot| snapshot.context_text)
            .unwrap_or_default()
    }

    /// 只提取候选术语，不保存用户整句，避免在 Session 兼容账本之外再复制对话。
    /// 新候选在下一次半小时编译时进入 ASR context。
    pub fn observe_user_text(&self, text: &str, observed_at_ms: i64) -> Result<usize> {
        let private_terms = read_private_lexicon(&self.inner.private_lexicon_path)?;
        let known_terms = BASE_TERMS
            .iter()
            .map(|term| (*term).to_string())
            .chain(private_terms)
            .collect::<Vec<_>>();
        let extracted = extract_terms(text, &known_terms);
        if extracted.is_empty() {
            return Ok(0);
        }

        let mut state = self.inner.state.write();
        for term in &extracted {
            let key = normalize_term(term);
            let entry = state
                .observed_terms
                .entry(key)
                .or_insert_with(|| ObservedTerm {
                    text: term.clone(),
                    count: 0,
                    last_observed_at_ms: observed_at_ms,
                });
            entry.text = preferred_spelling(&entry.text, term);
            entry.count = entry.count.saturating_add(1);
            entry.last_observed_at_ms = entry.last_observed_at_ms.max(observed_at_ms);
        }
        prune_observed_terms(&mut state.observed_terms);
        persist_state(&self.inner.state_path, &state)?;
        Ok(extracted.len())
    }

    pub fn refresh(
        &self,
        runtime: &RuntimeSnapshot,
        refreshed_at_ms: i64,
    ) -> Result<AsrContextSnapshot> {
        let private_terms = read_private_lexicon(&self.inner.private_lexicon_path)?;
        let mut state = self.inner.state.write();
        let next_revision = state.revision.saturating_add(1);
        let snapshot = compile_context(
            next_revision,
            refreshed_at_ms,
            runtime,
            &private_terms,
            &state.observed_terms,
        );
        state.revision = next_revision;
        state.snapshot = Some(snapshot.clone());
        persist_state(&self.inner.state_path, &state)?;
        Ok(snapshot)
    }
}

pub fn asr_context_compile_contract() -> CapabilityContract {
    CapabilityContract {
        capability_id: ASR_CONTEXT_CAPABILITY_ID.to_string(),
        version: 1,
        summary: "每30分钟把连续上下文编译为Qwen3-ASR术语快照".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "runtimeSnapshot": {"type": "object"},
                "recentTerms": {"type": "array", "items": {"type": "string"}},
                "maxTerms": {"type": "integer", "maximum": ASR_CONTEXT_MAX_TERMS}
            }
        }),
        output_schema: json!({
            "type": "object",
            "required": ["contextText", "terms", "refreshedAtMs"],
            "properties": {
                "contextText": {"type": "string"},
                "terms": {"type": "array", "items": {"type": "string"}},
                "refreshedAtMs": {"type": "integer"}
            }
        }),
        // 这是 PinvouOS 进程内的派生投影，只读取 Runtime 和本机私有词表。
        // 未来 Memory 只能经批准的稳定只读 Context Projection 接入。
        preconditions: Vec::new(),
        permissions: Vec::new(),
        side_effects: vec!["updates_local_asr_context_snapshot".to_string()],
        resource_class: ResourceClass::Light,
        interruptibility: Interruptibility::Immediate,
        idempotent: false,
    }
}

pub fn spawn_asr_context_agent(
    agent: AsrContextAgent,
    runtime: PinvouOsRuntime,
    cadence: Duration,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let cadence = cadence.max(Duration::from_secs(60));
        let mut ticker = tokio::time::interval(cadence);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let agent = agent.clone();
            let runtime = runtime.clone();
            let result = tokio::task::spawn_blocking(move || {
                agent.refresh(&runtime.snapshot(), chrono::Utc::now().timestamp_millis())
            })
            .await;
            match result {
                Ok(Ok(snapshot)) => log::info!(
                    "PinvouOS ASR Context Agent refreshed revision={} terms={} english={}",
                    snapshot.revision,
                    snapshot.term_count,
                    snapshot.english_term_count
                ),
                Ok(Err(error)) => {
                    log::warn!("PinvouOS ASR Context Agent refresh failed: {error:#}")
                }
                Err(error) => log::warn!("PinvouOS ASR Context Agent task failed: {error}"),
            }
        }
    })
}

#[derive(Debug, Clone)]
struct Candidate {
    text: String,
    score: i64,
    sources: BTreeSet<String>,
    english: bool,
}

fn compile_context(
    revision: u64,
    refreshed_at_ms: i64,
    runtime: &RuntimeSnapshot,
    private_terms: &[String],
    observed_terms: &BTreeMap<String, ObservedTerm>,
) -> AsrContextSnapshot {
    let mut candidates = BTreeMap::<String, Candidate>::new();
    for (index, term) in BASE_TERMS.iter().enumerate() {
        // 核心产品/模型名始终保留；其余公共词只在动态上下文没有更相关候选时补位。
        let score = if index < 16 { 9_000 } else { 3_000 };
        add_candidate(&mut candidates, term, "base", score);
    }
    for term in private_terms {
        add_candidate(&mut candidates, term, "private_lexicon", 10_000);
    }

    let known_terms = BASE_TERMS
        .iter()
        .map(|term| (*term).to_string())
        .chain(private_terms.iter().cloned())
        .collect::<Vec<_>>();

    for agent in runtime.agents.values() {
        add_candidate(&mut candidates, &agent.display_name, "runtime_agent", 6_500);
        for capability in &agent.capabilities {
            add_candidate(
                &mut candidates,
                &capability.capability_id,
                "runtime_capability",
                6_200,
            );
        }
        for term in extract_terms(&agent.role, &known_terms) {
            add_candidate(&mut candidates, &term, "runtime_agent", 5_500);
        }
    }
    for mission in runtime.missions.values() {
        for term in extract_terms(&mission.objective, &known_terms) {
            add_candidate(&mut candidates, &term, "active_mission", 7_000);
        }
    }
    for claim in runtime.claims.values().filter(|claim| claim.active) {
        add_candidate(&mut candidates, &claim.subject, "world_claim", 5_800);
        add_candidate(&mut candidates, &claim.predicate, "world_claim", 5_800);
        add_value_terms(
            &mut candidates,
            &claim.value,
            "world_claim",
            5_800,
            &known_terms,
            0,
        );
    }
    for observed in observed_terms.values() {
        let age_minutes = refreshed_at_ms
            .saturating_sub(observed.last_observed_at_ms)
            .max(0)
            / 60_000;
        let recency_bonus = (1_500_i64 - age_minutes.min(1_500)).max(0);
        let frequency_bonus = i64::from(observed.count.min(20)) * 100;
        add_candidate(
            &mut candidates,
            &observed.text,
            "recent_user_context",
            5_000 + recency_bonus + frequency_bonus,
        );
    }

    let mut ranked = candidates.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.english.cmp(&left.english))
            .then_with(|| left.text.to_lowercase().cmp(&right.text.to_lowercase()))
    });
    ranked.truncate(ASR_CONTEXT_MAX_TERMS);

    let terms = ranked
        .into_iter()
        .map(|candidate| AsrContextTerm {
            text: candidate.text,
            sources: candidate.sources.into_iter().collect(),
            score: candidate.score,
            english: candidate.english,
        })
        .collect::<Vec<_>>();
    let english_term_count = terms.iter().filter(|term| term.english).count();
    let joined = terms
        .iter()
        .map(|term| term.text.as_str())
        .collect::<Vec<_>>()
        .join("、");
    let mut context_text = format!(
        "场景：PinvouOS正在与用户持续交互。优先正确识别人名、产品名、英文缩写和技术术语。相关词：{joined}"
    );
    if context_text.chars().count() > MAX_CONTEXT_CHARS {
        context_text = context_text.chars().take(MAX_CONTEXT_CHARS).collect();
    }

    AsrContextSnapshot {
        revision,
        refreshed_at_ms,
        next_refresh_at_ms: refreshed_at_ms
            .saturating_add(ASR_CONTEXT_REFRESH_INTERVAL.as_millis() as i64),
        max_terms: ASR_CONTEXT_MAX_TERMS,
        term_count: terms.len(),
        english_term_count,
        context_text,
        terms,
    }
}

fn add_value_terms(
    candidates: &mut BTreeMap<String, Candidate>,
    value: &Value,
    source: &str,
    score: i64,
    known_terms: &[String],
    depth: usize,
) {
    if depth > 2 {
        return;
    }
    match value {
        Value::String(value) => {
            if value.chars().count() <= MAX_TERM_CHARS && looks_like_explicit_term(value) {
                add_candidate(candidates, value, source, score);
            }
            for term in extract_terms(value, known_terms) {
                add_candidate(candidates, &term, source, score);
            }
        }
        Value::Array(values) => {
            for value in values.iter().take(64) {
                add_value_terms(candidates, value, source, score, known_terms, depth + 1);
            }
        }
        Value::Object(values) => {
            for (key, value) in values.iter().take(64) {
                add_candidate(candidates, key, source, score.saturating_sub(100));
                add_value_terms(candidates, value, source, score, known_terms, depth + 1);
            }
        }
        _ => {}
    }
}

fn add_candidate(
    candidates: &mut BTreeMap<String, Candidate>,
    raw: &str,
    source: &str,
    base_score: i64,
) {
    let Some(term) = sanitize_term(raw) else {
        return;
    };
    let key = normalize_term(&term);
    let english = contains_ascii_letter(&term);
    let score = base_score + if english { 500 } else { 0 };
    let entry = candidates.entry(key).or_insert_with(|| Candidate {
        text: term.clone(),
        score,
        sources: BTreeSet::new(),
        english,
    });
    entry.text = preferred_spelling(&entry.text, &term);
    entry.score = entry.score.max(score);
    entry.english |= english;
    entry.sources.insert(source.to_string());
}

fn extract_terms(text: &str, known_terms: &[String]) -> Vec<String> {
    let mut terms = BTreeMap::<String, String>::new();
    for known in known_terms {
        if known.chars().any(is_cjk) && text.contains(known) {
            if let Some(term) = sanitize_term(known) {
                terms.insert(normalize_term(&term), term);
            }
        }
    }
    // 含凭据提示的整句不做自由英文抽取，避免把“password 后面的值”当成一个
    // 高频英文术语。已知的人名/产品名仍可保留，但任意未知 token 全部放弃。
    if contains_sensitive_marker(text) {
        return terms.into_values().collect();
    }

    let mut ascii_tokens = Vec::<String>::new();
    let mut current = String::new();
    for character in text.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_alphanumeric()
            || matches!(character, '.' | '-' | '_' | '+' | '#' | '/' | '=')
        {
            current.push(character);
        } else if !current.is_empty() {
            let token = std::mem::take(&mut current);
            if let Some(term) = sanitize_term(&token) {
                if contains_ascii_letter(&term)
                    && !ENGLISH_STOPWORDS.contains(&normalize_term(&term).as_str())
                {
                    ascii_tokens.push(term.clone());
                    terms.insert(normalize_term(&term), term);
                }
            }
        }
    }
    for pair in ascii_tokens.windows(2) {
        if pair.iter().any(|term| looks_distinctive_english(term)) {
            let phrase = format!("{} {}", pair[0], pair[1]);
            if let Some(term) = sanitize_term(&phrase) {
                terms.insert(normalize_term(&term), term);
            }
        }
    }

    for (open, close) in [('`', '`'), ('“', '”'), ('「', '」'), ('《', '》')] {
        let mut rest = text;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len_utf8()..];
            let Some(end) = after.find(close) else {
                break;
            };
            if let Some(term) = sanitize_term(&after[..end]) {
                terms.insert(normalize_term(&term), term);
            }
            rest = &after[end + close.len_utf8()..];
        }
    }
    terms.into_values().collect()
}

fn looks_distinctive_english(term: &str) -> bool {
    term.chars().any(|character| character.is_ascii_uppercase())
        || term.chars().any(|character| character.is_ascii_digit())
        || term.contains(['.', '-', '_', '+', '#'])
}

fn looks_like_explicit_term(value: &str) -> bool {
    let words = value.split_whitespace().count();
    words <= 6
        && (contains_ascii_letter(value)
            || (value.chars().any(is_cjk) && value.chars().count() <= 16))
}

fn sanitize_term(raw: &str) -> Option<String> {
    let term = raw
        .trim()
        .trim_matches(|character: char| {
            character.is_ascii_punctuation() && !matches!(character, '.' | '-' | '_' | '+' | '#')
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let length = term.chars().count();
    if length < 2 || length > MAX_TERM_CHARS || is_sensitive_or_unhelpful(&term) {
        return None;
    }
    if !term
        .chars()
        .any(|character| character.is_alphabetic() || is_cjk(character))
    {
        return None;
    }
    Some(term)
}

fn is_sensitive_or_unhelpful(term: &str) -> bool {
    let lower = term.to_ascii_lowercase();
    if term.contains('@')
        || lower.contains("://")
        || lower.starts_with("/home/")
        || lower.starts_with("c:\\")
    {
        return true;
    }
    if [
        "password",
        "passwd",
        "api_key",
        "apikey",
        "secret",
        "bearer ",
        "authorization",
        "private key",
        "access_key",
        "accesskey",
        "cookie",
        "密码",
        "密钥",
        "口令",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    let compact = term
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    compact.len() >= 24
        && compact
            .chars()
            .any(|character| character.is_ascii_lowercase())
        && compact
            .chars()
            .any(|character| character.is_ascii_uppercase())
        && compact.chars().any(|character| character.is_ascii_digit())
}

fn contains_sensitive_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "password",
        "passwd",
        "api key",
        "api_key",
        "apikey",
        "secret",
        "bearer",
        "authorization",
        "private key",
        "access key",
        "access_key",
        "accesskey",
        "cookie",
        "密码",
        "密钥",
        "口令",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn contains_ascii_letter(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_ascii_alphabetic())
}

fn is_cjk(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}')
}

fn normalize_term(value: &str) -> String {
    value.trim().to_lowercase()
}

fn preferred_spelling(existing: &str, candidate: &str) -> String {
    let existing_distinctive = looks_distinctive_english(existing);
    let candidate_distinctive = looks_distinctive_english(candidate);
    if candidate_distinctive && !existing_distinctive {
        candidate.to_string()
    } else {
        existing.to_string()
    }
}

fn prune_observed_terms(terms: &mut BTreeMap<String, ObservedTerm>) {
    if terms.len() <= MAX_OBSERVED_TERMS {
        return;
    }
    let mut ranked = terms.values().cloned().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .last_observed_at_ms
            .cmp(&left.last_observed_at_ms)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.text.cmp(&right.text))
    });
    *terms = ranked
        .into_iter()
        .take(MAX_OBSERVED_TERMS)
        .map(|term| (normalize_term(&term.text), term))
        .collect();
}

fn read_private_lexicon(path: &Path) -> Result<Vec<String>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read ASR lexicon {}", path.display()))
        }
    };
    if metadata.len() > MAX_PRIVATE_LEXICON_BYTES {
        anyhow::bail!("ASR private lexicon exceeds {MAX_PRIVATE_LEXICON_BYTES} bytes");
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("read ASR lexicon {}", path.display()))?;
    let mut terms = BTreeMap::<String, String>::new();
    for line in raw.lines().take(1_000) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for raw_term in line.split([',', '，', ';', '；']) {
            if let Some(term) = sanitize_term(raw_term) {
                terms.insert(normalize_term(&term), term);
            }
        }
    }
    Ok(terms.into_values().collect())
}

fn persist_state(path: &Path, state: &PersistedState) -> Result<()> {
    let payload = serde_json::to_vec_pretty(state).context("serialize ASR context state")?;
    crate::platform::filesystem::atomic_write(path, &payload)
        .with_context(|| format!("write ASR context state {}", path.display()))?;
    let file = fs::OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("reopen ASR context state {}", path.display()))?;
    super::platform::harden_private_ledger(&file)
        .with_context(|| format!("protect ASR context state {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_paths() -> (PathBuf, PathBuf, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "pinvou-asr-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let state = root.join("asr-context.v1.json");
        let lexicon = root.join("asr-lexicon.txt");
        (root, state, lexicon)
    }

    #[test]
    fn context_is_capped_and_english_terms_are_prioritized() {
        let mut runtime = RuntimeSnapshot::default();
        for index in 0..140 {
            runtime.missions.insert(
                format!("mission-{index}"),
                super::super::model::Mission {
                    mission_id: format!("mission-{index}"),
                    objective: format!("EnglishTerm{index} 技术讨论"),
                    priority: 50,
                    status: super::super::model::MissionStatus::Active,
                    created_at_ms: 1,
                    deadline_at_ms: None,
                },
            );
        }
        let snapshot = compile_context(1, 100_000, &runtime, &[], &BTreeMap::new());
        assert_eq!(snapshot.term_count, ASR_CONTEXT_MAX_TERMS);
        assert!(snapshot.english_term_count >= 90);
        assert!(snapshot.context_text.contains("Qwen3-ASR"));
    }

    #[test]
    fn private_name_and_recent_technical_terms_enter_next_snapshot() {
        let (root, state_path, lexicon_path) = temp_paths();
        fs::write(&lexicon_path, "白浪\n").unwrap();
        let agent = AsrContextAgent::boot(state_path.clone(), lexicon_path).unwrap();
        agent
            .observe_user_text("白浪正在调试Qwen3-ASR和OpenVINO Runtime", 90_000)
            .unwrap();
        let snapshot = agent.refresh(&RuntimeSnapshot::default(), 100_000).unwrap();

        assert!(snapshot.terms.iter().any(|term| term.text == "白浪"));
        assert!(snapshot
            .terms
            .iter()
            .any(|term| term.text.eq_ignore_ascii_case("Qwen3-ASR")));
        assert!(state_path.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn secrets_and_long_credentials_are_not_context_terms() {
        let known = vec!["白浪".to_string()];
        let terms = extract_terms(
            "白浪 密码是wolfman607714，API_KEY=do-not-store，OpenVINO",
            &known,
        );
        assert!(terms.contains(&"白浪".to_string()));
        assert!(!terms.contains(&"OpenVINO".to_string()));
        assert!(!terms.iter().any(|term| term.contains("do-not-store")));
        assert!(!terms.iter().any(|term| term.contains("wolfman")));
    }

    #[test]
    fn observed_state_keeps_terms_but_not_original_utterance() {
        let (root, state_path, lexicon_path) = temp_paths();
        let agent = AsrContextAgent::boot(state_path.clone(), lexicon_path).unwrap();
        agent
            .observe_user_text("请研究NewInferenceEngine的具体延迟表现", 100_000)
            .unwrap();
        let persisted = fs::read_to_string(state_path).unwrap();
        assert!(persisted.contains("NewInferenceEngine"));
        assert!(!persisted.contains("请研究"));
        fs::remove_dir_all(root).unwrap();
    }
}
