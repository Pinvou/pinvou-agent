//! Best-effort per-turn timing + token usage events.
//!
//! This intentionally stays outside SavedSession/messages so timing telemetry
//! never changes model context, session schema, or artifact behavior.
//!
//! 2026-07 升级:`assistant_done` 事件追加 `usage`(input/output/cache tokens)字段,
//! 作为内部诊断数据源。老 session 的旧事件无 usage 字段,反序列化按缺失处理
//! (`Option`),Timeline API 对老 session 也安全返回。

#[cfg(any(feature = "benchmark-hooks", test))]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(any(feature = "benchmark-hooks", test))]
use sha2::{Digest, Sha256};

static TURN_SEQ: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TURNS: OnceLock<Mutex<HashMap<String, VecDeque<ActiveTurnTiming>>>> = OnceLock::new();
#[cfg(any(feature = "benchmark-hooks", test))]
static EVAL_OBSERVATION_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[derive(Debug)]
struct ActiveTurnTiming {
    turn_id: String,
    #[cfg(any(feature = "benchmark-hooks", test))]
    recorded_first_events: HashSet<&'static str>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    tool_calls: u64,
    #[cfg(any(feature = "benchmark-hooks", test))]
    tool_failures: u64,
}

impl ActiveTurnTiming {
    fn new(turn_id: String) -> Self {
        Self {
            turn_id,
            #[cfg(any(feature = "benchmark-hooks", test))]
            recorded_first_events: HashSet::new(),
            #[cfg(any(feature = "benchmark-hooks", test))]
            tool_calls: 0,
            #[cfg(any(feature = "benchmark-hooks", test))]
            tool_failures: 0,
        }
    }
}

/// 防止诊断命令吞入异常增长的 sidecar。当前每轮最多记录六条生命周期事件，
/// 32 MiB 仍足以覆盖大量对话；超限时明确报错，不返回会被误解为空会话的假结果。
const MAX_TIMING_FILE_BYTES: u64 = 32 * 1024 * 1024;

fn active_turns() -> &'static Mutex<HashMap<String, VecDeque<ActiveTurnTiming>>> {
    ACTIVE_TURNS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(any(feature = "benchmark-hooks", test))]
fn eval_observation_sessions() -> &'static Mutex<HashSet<String>> {
    EVAL_OBSERVATION_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub(crate) fn register_eval_observation(session_id: &str) {
    if let Ok(mut sessions) = eval_observation_sessions().lock() {
        sessions.insert(session_id.to_string());
    }
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub(crate) fn unregister_eval_observation(session_id: &str) {
    if let Ok(mut sessions) = eval_observation_sessions().lock() {
        sessions.remove(session_id);
    }
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub(crate) fn eval_observation_enabled(session_id: &str) -> bool {
    eval_observation_sessions()
        .lock()
        .is_ok_and(|sessions| sessions.contains(session_id))
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn new_turn_id(session_id: &str) -> String {
    let seq = TURN_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("turn_{}_{}", now_ms(), seq)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        + "_"
        + &session_id.chars().take(8).collect::<String>()
}

fn append_event(session_id: &str, entry: serde_json::Value) {
    let path = crate::platform::paths::session_timing_events(session_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[timing] create dir failed ({}): {e}", parent.display());
            return;
        }
    }
    let mut line = entry.to_string();
    line.push('\n');
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = result {
        eprintln!("[timing] append failed ({}): {e}", path.display());
    }
}

/// 单轮 token usage 快照(随 `assistant_done` 落盘)。
///
/// 记录上游 `deepseek_tui::models::Usage` 中当前诊断需要的主要 token 字段。
/// 老 session 的旧事件缺这些字段时按 0 读取(见 `read_timeline`)。这里不做金额
/// 换算,也不宣称覆盖 provider 的全部计费维度。
///
/// - `input_tokens` / `output_tokens`:基础输入输出(u32 升 u64)。
/// - `cache_hit_tokens` / `cache_miss_tokens`:Anthropic 风格 prompt cache 命中 /
///   未命中(u32 升 u64)。非 Anthropic provider 全为 0。
/// - `cache_write_tokens`:`cache_creation_input_tokens`,按 cache-write 计费
///   (具体费率由 provider 决定)。
/// - `reasoning_tokens`:推理 token。
/// - `context_window`: context window for the active route. Hydration uses it as the usage
///   denominator; legacy events default to zero and omit the percentage.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub context_window: u64,
}

pub fn start_turn(session_id: &str) -> String {
    let turn_id = new_turn_id(session_id);
    if let Ok(mut map) = active_turns().lock() {
        map.entry(session_id.to_string())
            .or_default()
            .push_back(ActiveTurnTiming::new(turn_id.clone()));
    }
    append_event(
        session_id,
        json!({
            "event": "user_start",
            "session_id": session_id,
            "turn_id": turn_id.clone(),
            "timestamp": now_ms(),
            "ts": Utc::now().to_rfc3339(),
        }),
    );
    turn_id
}

/// On session deletion, clear its residual unpaired turn queues (map keys
/// accumulate per session id and would grow unbounded over the app lifetime
/// without this).
pub fn clear_session(session_id: &str) {
    if let Ok(mut map) = active_turns().lock() {
        map.remove(session_id);
    }
}

/// Test probe: whether this session still has unpaired turn-queue entries (for deletion-path regression assertions).
#[cfg(test)]
pub(crate) fn has_queued_active_turn(session_id: &str) -> bool {
    active_turns()
        .lock()
        .map(|map| map.contains_key(session_id))
        .unwrap_or(false)
}

/// Test probe: queue length for this session (1 means exactly the single
/// in-flight turn; admission-race tests assert a rejected submit leaves it
/// at 1 rather than enqueueing a ghost second entry).
#[cfg(test)]
pub(crate) fn has_extra_queued_turns(session_id: &str) -> bool {
    active_turns()
        .lock()
        .map(|map| map.get(session_id).is_some_and(|queue| queue.len() > 1))
        .unwrap_or(false)
}

/// 旧调用点(无 usage):落盘不带 usage 字段,reader API 反序列化为 None。
/// 保留是为了不强迫所有调用点同步升级(commands.rs:200 的 send_error 兜底等)。
pub fn finish_turn(session_id: &str, status: &str, error: Option<&str>) {
    finish_turn_with_usage(session_id, status, error, None);
}

/// 是否有未收口的在途回合（start_turn 已登记、finish_turn 未弹出）。
/// 用于跨会话安全守卫（如禁止在同工作区会话运行中切换 Git 分支）。
pub fn has_active_turn(session_id: &str) -> bool {
    active_turns()
        .lock()
        .map(|map| map.get(session_id).is_some_and(|queue| !queue.is_empty()))
        .unwrap_or(false)
}

/// Records post-compaction context usage without creating a turn.
///
/// Manual and automatic compaction do not call `start_turn`, and `TurnComplete` carries zero
/// usage for this path. The standalone snapshot lets hydration restore the compacted estimate
/// without changing turn statistics.
pub fn record_context_snapshot(session_id: &str, input_tokens: u64, context_window: u64) {
    if input_tokens == 0 {
        return;
    }
    append_event(
        session_id,
        json!({
            "event": "context_snapshot",
            "session_id": session_id,
            "turn_id": format!("context-snapshot-{}", now_ms()),
            "timestamp": now_ms(),
            "ts": Utc::now().to_rfc3339(),
            "usage": {
                "input_tokens": input_tokens,
                "context_window": context_window,
            },
        }),
    );
}

/// 收尾本轮并把 usage 落进 `assistant_done` 事件。
///
/// `usage: Option<TurnUsage>` —— None 时(老路径 / 失败兜底)不写 usage 字段,
/// reader API 按缺失处理。Some 时写入 6 个 token 计数。
pub fn finish_turn_with_usage(
    session_id: &str,
    status: &str,
    error: Option<&str>,
    usage: Option<TurnUsage>,
) {
    finish_turn_internal(
        session_id,
        status,
        error,
        usage,
        #[cfg(any(feature = "benchmark-hooks", test))]
        None,
        #[cfg(any(feature = "benchmark-hooks", test))]
        false,
    );
}

/// Engine 为本轮构建的授权工具目录摘要。
///
/// 这是进入每一步动态激活前的目录，不等同于某次模型请求实际携带的工具集合。
/// 只记录数量、序列化字节数和 SHA-256，不持久化完整工具 Schema。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(any(feature = "benchmark-hooks", test))]
pub struct ToolCatalogSummary {
    pub catalog_count: u64,
    pub catalog_bytes: u64,
    pub catalog_sha256: String,
}

#[cfg(any(feature = "benchmark-hooks", test))]
impl ToolCatalogSummary {
    pub fn from_serialized_catalog(catalog_count: usize, catalog_json: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(catalog_json);
        Self {
            catalog_count: catalog_count as u64,
            catalog_bytes: catalog_json.len() as u64,
            catalog_sha256: crate::platform::encoding::hex_lower(&hasher.finalize()),
        }
    }
}

/// 记录带授权工具目录摘要的 turn 终态。
#[cfg(any(feature = "benchmark-hooks", test))]
pub fn finish_turn_with_observation(
    session_id: &str,
    status: &str,
    error: Option<&str>,
    usage: Option<TurnUsage>,
    authorized_tool_catalog: Option<ToolCatalogSummary>,
) {
    finish_turn_internal(
        session_id,
        status,
        error,
        usage,
        authorized_tool_catalog,
        true,
    );
}

fn finish_turn_internal(
    session_id: &str,
    status: &str,
    error: Option<&str>,
    usage: Option<TurnUsage>,
    #[cfg(any(feature = "benchmark-hooks", test))] authorized_tool_catalog: Option<
        ToolCatalogSummary,
    >,
    #[cfg(any(feature = "benchmark-hooks", test))] include_observation: bool,
) {
    // Only one turn per session is in flight at a time (turn-lock
    // serialization), so the finishing turn is always the last one queued;
    // take from the tail. Earlier entries can only be stale ids left by the
    // "canceled before submit" path (start_turn followed by
    // emit_unsubmitted_interrupted_terminal, no assistant_done) — popping
    // FIFO would attribute assistant_done to the stale turn and leave the
    // real id stuck in the queue forever. Clear the whole queue here;
    // stale turns produce no terminal event.
    let active_turn = active_turns().lock().ok().and_then(|mut map| {
        let id = map.get_mut(session_id)?.pop_back();
        map.remove(session_id);
        id
    });

    let Some(active_turn) = active_turn else {
        return;
    };

    let mut entry = json!({
        "event": "assistant_done",
        "session_id": session_id,
        "turn_id": active_turn.turn_id,
        "timestamp": now_ms(),
        "ts": Utc::now().to_rfc3339(),
        "status": status,
        "error": error,
    });
    if let Some(u) = usage {
        entry["usage"] = json!({
            "input_tokens": u.input_tokens,
            "output_tokens": u.output_tokens,
            "cache_hit_tokens": u.cache_hit_tokens,
            "cache_miss_tokens": u.cache_miss_tokens,
            "cache_write_tokens": u.cache_write_tokens,
            "reasoning_tokens": u.reasoning_tokens,
            "context_window": u.context_window,
        });
    }
    #[cfg(any(feature = "benchmark-hooks", test))]
    if include_observation {
        entry["tool_calls"] = json!(active_turn.tool_calls);
        entry["tool_failures"] = json!(active_turn.tool_failures);
        if let Some(catalog) = authorized_tool_catalog {
            entry["authorized_tool_catalog"] = serde_json::to_value(catalog).unwrap_or_default();
        }
    }
    append_event(session_id, entry);
}

#[cfg(any(feature = "benchmark-hooks", test))]
fn record_first_event(session_id: &str, event: &'static str, tool_name: Option<&str>) {
    // Take the tail: consistent with finish_turn_internal's tail-pop
    // semantics — earlier entries are stale turns left by "canceled before
    // submit"; observation events must land on the truly in-flight turn.
    let turn_id = active_turns().lock().ok().and_then(|mut map| {
        let active = map.get_mut(session_id)?.back_mut()?;
        active
            .recorded_first_events
            .insert(event)
            .then(|| active.turn_id.clone())
    });
    let Some(turn_id) = turn_id else {
        return;
    };
    append_event(
        session_id,
        json!({
            "event": event,
            "session_id": session_id,
            "turn_id": turn_id,
            "timestamp": now_ms(),
            "ts": Utc::now().to_rfc3339(),
            "tool_name": tool_name,
        }),
    );
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_engine_turn_started(session_id: &str) {
    record_first_event(session_id, "engine_turn_started", None);
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_first_message_delta(session_id: &str) {
    record_first_event(session_id, "first_message_delta", None);
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_tool_started(session_id: &str, tool_name: &str) {
    if let Ok(mut map) = active_turns().lock() {
        if let Some(active) = map.get_mut(session_id).and_then(VecDeque::back_mut) {
            active.tool_calls = active.tool_calls.saturating_add(1);
        }
    }
    record_first_event(session_id, "first_tool_call_started", Some(tool_name));
}

#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_tool_completed(session_id: &str, tool_name: &str, success: bool) {
    if !success {
        if let Ok(mut map) = active_turns().lock() {
            if let Some(active) = map.get_mut(session_id).and_then(VecDeque::back_mut) {
                active.tool_failures = active.tool_failures.saturating_add(1);
            }
        }
    }
    record_first_event(session_id, "first_tool_call_completed", Some(tool_name));
}

/// Record an additional milestone without consuming the active turn id.
#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_milestone(session_id: &str, milestone: &str) {
    record_milestone_meta(session_id, milestone, serde_json::Value::Null);
}

/// Record an additional milestone with bounded structured metadata.
#[cfg(any(feature = "benchmark-hooks", test))]
pub fn record_milestone_meta(session_id: &str, milestone: &str, meta: serde_json::Value) {
    let turn_id = active_turns().lock().ok().and_then(|map| {
        map.get(session_id)
            .and_then(|queue| queue.back())
            .map(|active| active.turn_id.clone())
    });
    let Some(turn_id) = turn_id else { return };
    let mut entry = json!({
        "event": milestone,
        "session_id": session_id,
        "turn_id": turn_id,
        "timestamp": now_ms(),
        "ts": Utc::now().to_rfc3339(),
    });
    if let (Some(target), Some(source)) = (entry.as_object_mut(), meta.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    append_event(session_id, entry);
}

/// 解析后的 timeline 事件。一个 turn 的生命周期事件按同一 turn_id 关联。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub turn_id: String,
    pub event: String,  // "user_start" | "assistant_done" | "context_snapshot"
    pub timestamp: i64, // ms epoch
    pub ts: String,     // RFC3339
    pub status: Option<String>, // assistant_done only
    pub error: Option<String>, // assistant_done only
    pub usage: Option<TurnUsage>, // assistant_done / context_snapshot(老事件为 None)
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub tool_name: Option<String>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub tool_calls: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub tool_failures: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    pub authorized_tool_catalog: Option<ToolCatalogSummary>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_duration_ms: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[cfg(any(feature = "benchmark-hooks", test))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

/// 读取 session 的全部 timeline 事件,按 timestamp 升序。
///
/// 文件不存在 / 解析失败的行被静默跳过(append-only jsonl 的单行损坏不应让整个 timeline 崩)。
/// 空文件 / 文件不存在 → 空向量。
pub fn read_timeline(session_id: &str) -> std::io::Result<Vec<TimelineEvent>> {
    let path = crate::platform::paths::session_timing_events(session_id);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let file_len = file.metadata()?.len();
    if file_len > MAX_TIMING_FILE_BYTES {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            format!("timing sidecar too large: {file_len} bytes (limit {MAX_TIMING_FILE_BYTES})"),
        ));
    }

    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut line = String::new();
    let mut bytes_read = 0_u64;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(read as u64);
        // 文件可能在 metadata 后继续增长,读取过程中再守一次上限。
        if bytes_read > MAX_TIMING_FILE_BYTES {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("timing sidecar grew beyond {MAX_TIMING_FILE_BYTES} bytes while reading"),
            ));
        }
        if let Some(event) = parse_timeline_line(&line) {
            events.push(event);
        }
    }
    events.sort_by_key(|e| e.timestamp);
    Ok(events)
}

fn parse_timeline_line(line: &str) -> Option<TimelineEvent> {
    if line.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let turn_id = v.get("turn_id")?.as_str()?.trim();
    if turn_id.is_empty() {
        return None;
    }
    let event = v.get("event")?.as_str()?;
    // A context snapshot has no paired user_start. compute_stats only aggregates paired
    // (user_start, assistant_done) records, so snapshots cannot affect turn totals.
    let is_base_event = matches!(event, "user_start" | "assistant_done" | "context_snapshot");
    #[cfg(any(feature = "benchmark-hooks", test))]
    let is_observation_event = matches!(
        event,
        "engine_turn_started"
            | "first_message_delta"
            | "first_tool_call_started"
            | "first_tool_call_completed"
            | "turn_started"
            | "first_delta"
            | "tool_call_started"
            | "tool_call_completed"
            | "model_request_metric"
    );
    #[cfg(not(any(feature = "benchmark-hooks", test)))]
    let is_observation_event = false;
    if !is_base_event && !is_observation_event {
        return None;
    }
    let timestamp = v.get("timestamp")?.as_i64()?;
    let ts = v.get("ts")?.as_str()?.trim();
    if ts.is_empty() {
        return None;
    }
    let usage = v
        .get("usage")
        .and_then(serde_json::Value::as_object)
        .map(|u| TurnUsage {
            input_tokens: u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            output_tokens: u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0),
            cache_hit_tokens: u
                .get("cache_hit_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            cache_miss_tokens: u
                .get("cache_miss_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            cache_write_tokens: u
                .get("cache_write_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            reasoning_tokens: u
                .get("reasoning_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            context_window: u
                .get("context_window")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
        });
    Some(TimelineEvent {
        turn_id: turn_id.to_string(),
        event: event.to_string(),
        timestamp,
        ts: ts.to_string(),
        status: v.get("status").and_then(|x| x.as_str()).map(str::to_string),
        error: v
            .get("error")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        usage,
        #[cfg(any(feature = "benchmark-hooks", test))]
        tool_name: v
            .get("tool_name")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        #[cfg(any(feature = "benchmark-hooks", test))]
        tool_id: v
            .get("tool_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        #[cfg(any(feature = "benchmark-hooks", test))]
        tool_calls: v.get("tool_calls").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        tool_failures: v.get("tool_failures").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        authorized_tool_catalog: v
            .get("authorized_tool_catalog")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        elapsed_ms: v.get("elapsed_ms").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        request_duration_ms: v.get("request_duration_ms").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        ttft_ms: v.get("ttft_ms").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        input_tokens: v.get("input_tokens").and_then(|x| x.as_u64()),
        #[cfg(any(feature = "benchmark-hooks", test))]
        output_tokens: v.get("output_tokens").and_then(|x| x.as_u64()),
    })
}

/// session 级聚合统计,供内部诊断入口使用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionTimelineStats {
    pub turn_count: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_hit_tokens: u64,
    pub total_cache_miss_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub total_reasoning_tokens: u64,
    pub first_turn_ts: Option<String>,
    pub last_turn_ts: Option<String>,
    pub completed_turns: usize,
    pub failed_turns: usize,
    /// 用户主动 Ctrl-C / 超时中断的轮次(engine 上游 TurnOutcomeStatus::Interrupted)。
    /// 之前被 `_ => {}` 静默丢弃,导致 turn_count 与 completed+failed 对不上。
    pub interrupted_turns: usize,
    /// 有 user_start 但没有 assistant_done 的轮次,常见于进程退出或文件尾部截断。
    pub incomplete_turns: usize,
    /// assistant_done 存在,但状态不是当前已知枚举。单独暴露,避免静默制造差值。
    pub unknown_status_turns: usize,
}

/// 聚合 session timeline 为单个 stats 对象。
///
/// 算法:遍历 read_timeline(),按 turn_id 配对 (user_start, assistant_done),
/// 每对算一个 turn。token / 状态从 assistant_done 取(失败时 usage 可能为 None)。
pub fn compute_stats(session_id: &str) -> std::io::Result<SessionTimelineStats> {
    let events = read_timeline(session_id)?;
    let mut stats = SessionTimelineStats::default();
    // 按 turn_id 索引;一个 turn 由 user_start + assistant_done 组成
    let mut by_turn: HashMap<String, (Option<&TimelineEvent>, Option<&TimelineEvent>)> =
        HashMap::new();
    for e in &events {
        let entry = by_turn.entry(e.turn_id.clone()).or_insert((None, None));
        if e.event == "assistant_done" {
            entry.1 = Some(e);
        } else if e.event == "user_start" {
            entry.0 = Some(e);
        }
    }
    stats.turn_count = by_turn
        .values()
        .filter(|(start, _)| start.is_some())
        .count();
    stats.first_turn_ts = events
        .iter()
        .find(|event| {
            by_turn
                .get(&event.turn_id)
                .is_some_and(|pair| pair.0.is_some())
        })
        .map(|event| event.ts.clone());
    stats.last_turn_ts = events
        .iter()
        .rev()
        .find(|event| {
            by_turn
                .get(&event.turn_id)
                .is_some_and(|pair| pair.0.is_some())
        })
        .map(|event| event.ts.clone());
    for (start, done) in by_turn.values() {
        if start.is_none() {
            continue;
        }
        let Some(d) = done else {
            stats.incomplete_turns += 1;
            continue;
        };
        if let Some(u) = &d.usage {
            stats.total_input_tokens = stats.total_input_tokens.saturating_add(u.input_tokens);
            stats.total_output_tokens = stats.total_output_tokens.saturating_add(u.output_tokens);
            stats.total_cache_hit_tokens = stats
                .total_cache_hit_tokens
                .saturating_add(u.cache_hit_tokens);
            stats.total_cache_miss_tokens = stats
                .total_cache_miss_tokens
                .saturating_add(u.cache_miss_tokens);
            stats.total_cache_write_tokens = stats
                .total_cache_write_tokens
                .saturating_add(u.cache_write_tokens);
            stats.total_reasoning_tokens = stats
                .total_reasoning_tokens
                .saturating_add(u.reasoning_tokens);
        }
        match d.status.as_deref() {
            Some(s) if s.eq_ignore_ascii_case("Completed") => stats.completed_turns += 1,
            Some(s) if s.eq_ignore_ascii_case("Failed") || s.eq_ignore_ascii_case("send_error") => {
                stats.failed_turns += 1;
            }
            Some(s) if s.eq_ignore_ascii_case("Interrupted") => stats.interrupted_turns += 1,
            _ => stats.unknown_status_turns += 1,
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::paths::tests::ENV_LOCK;

    #[test]
    fn timing_events_append_and_pair_turn_id() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-a";
        let turn_id = start_turn(sid);
        finish_turn(sid, "completed", None);

        let content =
            std::fs::read_to_string(crate::platform::paths::session_timing_events(sid)).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        let start: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let done: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(start["event"], "user_start");
        assert_eq!(done["event"], "assistant_done");
        assert_eq!(start["turn_id"].as_str(), Some(turn_id.as_str()));
        assert_eq!(done["turn_id"].as_str(), Some(turn_id.as_str()));
        // 老路径(无 usage):事件里不应有 usage 字段
        assert!(done.get("usage").is_none() || done["usage"].is_null());

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn finish_turn_with_usage_records_usage_field() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-usage-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-usage";
        start_turn(sid);
        finish_turn_with_usage(
            sid,
            "Completed",
            None,
            Some(TurnUsage {
                input_tokens: 1200,
                output_tokens: 350,
                cache_hit_tokens: 800,
                cache_miss_tokens: 400,
                ..Default::default()
            }),
        );

        let timeline = read_timeline(sid).unwrap();
        assert_eq!(timeline.len(), 2);
        let done = timeline
            .iter()
            .find(|e| e.event == "assistant_done")
            .expect("has assistant_done");
        let u = done.usage.expect("usage recorded");
        assert_eq!(u.input_tokens, 1200);
        assert_eq!(u.output_tokens, 350);
        assert_eq!(u.cache_hit_tokens, 800);
        assert_eq!(u.cache_miss_tokens, 400);

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn context_snapshot_roundtrips_without_polluting_stats() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-snapshot-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-snapshot";
        start_turn(sid);
        finish_turn_with_usage(
            sid,
            "Completed",
            None,
            Some(TurnUsage {
                input_tokens: 1000,
                context_window: 64_000,
                ..Default::default()
            }),
        );
        record_context_snapshot(sid, 120, 64_000);
        // A zero-valued snapshot carries no useful state and is not persisted.
        record_context_snapshot(sid, 0, 64_000);

        let timeline = read_timeline(sid).unwrap();
        let snapshots: Vec<_> = timeline
            .iter()
            .filter(|e| e.event == "context_snapshot")
            .collect();
        assert_eq!(snapshots.len(), 1);
        let usage = snapshots[0].usage.expect("snapshot usage recorded");
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.context_window, 64_000);

        // The unpaired snapshot must not affect turn or token totals.
        let stats = compute_stats(sid).unwrap();
        assert_eq!(stats.turn_count, 1);
        assert_eq!(stats.total_input_tokens, 1000);

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn read_timeline_handles_missing_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe {
            std::env::set_var(
                "PINVOU3_HOME",
                std::env::temp_dir().join(format!("nonexistent-{}", now_ms())),
            )
        };
        let timeline = read_timeline("never-existed").unwrap();
        assert!(timeline.is_empty());
    }

    #[test]
    fn read_timeline_skips_corrupt_lines() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-corrupt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-corrupt";
        start_turn(sid);
        // 注入一行坏 JSON
        let path = crate::platform::paths::session_timing_events(sid);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(b"this is not json\n"))
            .unwrap();
        finish_turn(sid, "completed", None);

        // 应该跳过坏行,只拿到 2 条事件
        let timeline = read_timeline(sid).unwrap();
        assert_eq!(timeline.len(), 2);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn compute_stats_aggregates_usage_and_status() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-stats-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-stats";
        // 2 个 completed turn + 1 个 failed turn
        for _ in 0..2 {
            start_turn(sid);
            finish_turn_with_usage(
                sid,
                "Completed",
                None,
                Some(TurnUsage {
                    input_tokens: 1000,
                    output_tokens: 200,
                    cache_hit_tokens: 500,
                    cache_miss_tokens: 500,
                    ..Default::default()
                }),
            );
        }
        start_turn(sid);
        finish_turn_with_usage(sid, "Failed", Some("oops"), None);

        let stats = compute_stats(sid).unwrap();
        assert_eq!(stats.turn_count, 3);
        assert_eq!(stats.completed_turns, 2);
        assert_eq!(stats.failed_turns, 1);
        assert_eq!(stats.total_input_tokens, 2000);
        assert_eq!(stats.total_output_tokens, 400);
        assert_eq!(stats.total_cache_hit_tokens, 1000);
        assert_eq!(stats.total_cache_miss_tokens, 1000);
        assert!(stats.first_turn_ts.is_some());
        assert!(stats.last_turn_ts.is_some());

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn compute_stats_on_missing_session_is_default() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe {
            std::env::set_var(
                "PINVOU3_HOME",
                std::env::temp_dir().join(format!("nope-{}", now_ms())),
            )
        };
        let stats = compute_stats("does-not-exist").unwrap();
        assert_eq!(stats.turn_count, 0);
        assert_eq!(stats.total_input_tokens, 0);
        assert!(stats.first_turn_ts.is_none());
    }

    /// [F2] Interrupted 终态之前被 `_ => {}` 静默丢弃,导致 turn_count 与
    /// completed+failed 对不上。验证 compute_stats 现在能正确分类三个终态,
    /// 且大小写不敏感(engine 上游落盘是 PascalCase,但脏数据可能是 lowercase)。
    #[test]
    fn compute_stats_counts_interrupted_terminal_state() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-interrupted-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-interrupted";
        // 1 completed + 1 failed + 2 interrupted(一个 PascalCase 一个 lowercase,
        // 验证 eq_ignore_ascii_case 写法不再漏变体)
        start_turn(sid);
        finish_turn_with_usage(sid, "Completed", None, None);
        start_turn(sid);
        finish_turn_with_usage(sid, "Failed", Some("err"), None);
        start_turn(sid);
        finish_turn_with_usage(sid, "Interrupted", Some("ctrl-c"), None);
        start_turn(sid);
        finish_turn_with_usage(sid, "interrupted", None, None);

        let stats = compute_stats(sid).unwrap();
        assert_eq!(stats.turn_count, 4);
        assert_eq!(stats.completed_turns, 1);
        assert_eq!(stats.failed_turns, 1);
        assert_eq!(stats.interrupted_turns, 2);
        // 三个终态加起来必须等于 turn_count——这是 F2 修复的本质保证
        assert_eq!(
            stats.completed_turns + stats.failed_turns + stats.interrupted_turns,
            stats.turn_count
        );

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn compute_stats_classifies_real_terminal_edges() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-terminal-edges-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-terminal-edges";
        start_turn(sid);
        finish_turn(sid, "send_error", Some("engine unavailable"));
        start_turn(sid);
        finish_turn(sid, "FutureStatus", None);
        start_turn(sid);

        let stats = compute_stats(sid).unwrap();
        assert_eq!(stats.turn_count, 3);
        assert_eq!(stats.failed_turns, 1);
        assert_eq!(stats.incomplete_turns, 1);
        assert_eq!(stats.unknown_status_turns, 1);
        assert_eq!(
            stats.completed_turns
                + stats.failed_turns
                + stats.interrupted_turns
                + stats.incomplete_turns
                + stats.unknown_status_turns,
            stats.turn_count
        );

        // 清掉进程内 ACTIVE_TURNS 的未完成记录,避免影响同进程后续测试。
        finish_turn(sid, "Interrupted", None);
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn read_timeline_skips_events_without_required_identity() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-bad-identity-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-bad-identity";
        start_turn(sid);
        finish_turn(sid, "Completed", None);
        let path = crate::platform::paths::session_timing_events(sid);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut file| {
                file.write_all(
                    br#"{"event":"user_start","timestamp":1,"ts":"1970-01-01T00:00:00Z"}
{"turn_id":"missing-event","timestamp":2,"ts":"1970-01-01T00:00:00Z"}
{"event":"unknown","turn_id":"unknown-event","timestamp":3,"ts":"1970-01-01T00:00:00Z"}
"#,
                )
            })
            .unwrap();

        let timeline = read_timeline(sid).unwrap();
        assert_eq!(timeline.len(), 2);
        assert_eq!(compute_stats(sid).unwrap().turn_count, 1);

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn read_timeline_surfaces_io_errors_instead_of_empty_data() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-io-error-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let path = crate::platform::paths::session_timing_events("session-io-error");
        std::fs::create_dir_all(&path).unwrap();
        assert!(read_timeline("session-io-error").is_err());
        assert!(compute_stats("session-io-error").is_err());

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn read_timeline_rejects_oversized_sidecar() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-too-large-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let path = crate::platform::paths::session_timing_events("session-too-large");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_TIMING_FILE_BYTES + 1).unwrap();
        let error = read_timeline("session-too-large").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// [F4] 缺 timestamp / 类型错的脏事件之前 unwrap_or(0) 变成 1970-01-01,
    /// 污染 first_turn_ts / last_turn_ts。验证现在被整条跳过:
    /// turn_count 不被多算、first/last 不被 1970 污染。
    #[test]
    fn read_timeline_skips_events_with_missing_or_bad_timestamp() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-badts-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-badts";
        // 一条正常 turn
        start_turn(sid);
        finish_turn(sid, "completed", None);

        // 注入两条脏事件:一条完全缺 timestamp,一条 timestamp 类型错(string 而非 int)
        let path = crate::platform::paths::session_timing_events(sid);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                f.write_all(
                    br#"{"event":"user_start","turn_id":"bad1","ts":"2099-01-01T00:00:00Z"}
{"event":"assistant_done","turn_id":"bad2","timestamp":"not-a-number","ts":"2099-01-01T00:00:00Z"}
"#,
                )
            })
            .unwrap();

        let timeline = read_timeline(sid).unwrap();
        // 只剩 2 条正常事件;2 条脏事件被跳过
        assert_eq!(timeline.len(), 2, "dirty-timestamp events must be skipped");
        // 没有任何事件的 timestamp 是 0(1970)
        assert!(
            timeline.iter().all(|e| e.timestamp > 0),
            "no event should fall back to 1970"
        );

        let stats = compute_stats(sid).unwrap();
        // turn_count 不被坏事件污染(还是 1)
        assert_eq!(stats.turn_count, 1);
        // first/last 都不是 1970(脏事件 timestamp=0 排序后会变成 first)
        assert!(stats.first_turn_ts.is_some());
        assert!(stats.last_turn_ts.is_some());

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// Earlier entries in the queue can only be stale ids left by the
    /// "canceled before submit" path (no terminal recorded). Start two
    /// turns in a row to simulate stale residue, then finish: the terminal
    /// must be attributed to the last queued turn, and once the queue is
    /// cleared a further finish records nothing — a FIFO pop would
    /// attribute assistant_done to the stale turn while the real id stays
    /// stuck in the queue, mis-attributing later turns.
    #[test]
    fn finish_turn_attributes_terminal_to_newest_queued_turn() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-tail-pop-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-tail-pop";
        let stale = start_turn(sid);
        let live = start_turn(sid);
        finish_turn(sid, "Completed", None);

        let timeline = read_timeline(sid).unwrap();
        let done: Vec<_> = timeline
            .iter()
            .filter(|e| e.event == "assistant_done")
            .collect();
        assert_eq!(done.len(), 1);
        assert_eq!(
            done[0].turn_id, live,
            "terminal must be attributed to the newest queued turn"
        );
        assert_ne!(done[0].turn_id, stale);

        // The queue has been fully cleared: another finish must not record a terminal event.
        finish_turn(sid, "Completed", None);
        assert_eq!(
            read_timeline(sid)
                .unwrap()
                .iter()
                .filter(|e| e.event == "assistant_done")
                .count(),
            1,
            "finish on an emptied queue must be a no-op"
        );

        clear_session(sid);
        let _ = std::fs::remove_dir_all(tmp);
    }

    /// After clear_session removes a session's residual unpaired queues, a
    /// late finish must not record events to that session's sidecar;
    /// repeated clears are idempotent.
    #[test]
    fn clear_session_empties_queue_and_silences_late_finish() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-clear-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: the caller's test holds platform::paths::tests::ENV_LOCK throughout; env writes are serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-clear";
        start_turn(sid);
        clear_session(sid);
        finish_turn(sid, "Completed", None);

        let timeline = read_timeline(sid).unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].event, "user_start");

        clear_session(sid);
        finish_turn(sid, "Completed", None);
        assert_eq!(read_timeline(sid).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// Full chain for TurnUsage: persisting all fields, reading them back, and
    /// compute_stats accumulation. [F3] verifies the forward-compat field set
    /// (cache_write_tokens / reasoning_tokens) keeps every field, while also
    /// covering the basic fields (input/output/cache_hit/cache_miss).
    #[test]
    fn finish_turn_with_usage_records_all_usage_fields() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-extended-usage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: platform::paths::tests::ENV_LOCK held; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-extended-usage";
        start_turn(sid);
        finish_turn_with_usage(
            sid,
            "Completed",
            None,
            Some(TurnUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_hit_tokens: 30,
                cache_miss_tokens: 20,
                cache_write_tokens: 80,
                reasoning_tokens: 500,
                context_window: 128_000,
            }),
        );

        let timeline = read_timeline(sid).unwrap();
        assert_eq!(timeline.len(), 2);
        let done = timeline
            .iter()
            .find(|e| e.event == "assistant_done")
            .expect("has assistant_done");
        let u = done.usage.expect("usage recorded");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.cache_hit_tokens, 30);
        assert_eq!(u.cache_miss_tokens, 20);
        assert_eq!(u.cache_write_tokens, 80);
        assert_eq!(u.reasoning_tokens, 500);
        assert_eq!(u.context_window, 128_000);

        let stats = compute_stats(sid).unwrap();
        assert_eq!(stats.total_cache_write_tokens, 80);
        assert_eq!(stats.total_reasoning_tokens, 500);

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// Two queued turns: after two consecutive start_turn calls enqueue on
    /// the same session, one observation finish consumes a single turn.
    /// The forwarder previously double-finished (usage variant +
    /// observation variant), popping twice and corrupting the second
    /// turn's assistant_done; that call site has since converged to a
    /// single observation finish, and this test pins the "one finish
    /// consumes one active turn" semantics.
    /// Attribution follows the tail-pop semantics (finish_turn_internal):
    /// under the single-flight guarantee the in-flight turn is the last one
    /// queued, and earlier entries are stale turns left by "canceled before
    /// submit" — the terminal lands on second, the whole queue is cleared,
    /// stale first produces no terminal event, and later finishes are
    /// no-ops.
    #[test]
    fn single_observation_finish_tail_pops_latest_turn_and_clears_queue() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-queued-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-queued";
        let first = start_turn(sid);
        let second = start_turn(sid);
        assert_ne!(first, second);

        // 模拟当前 forwarder 的单次 observation 收尾(带 usage 与目录摘要)。
        finish_turn_with_observation(
            sid,
            "Completed",
            None,
            Some(TurnUsage::default()),
            Some(ToolCatalogSummary::from_serialized_catalog(1, b"[{}]")),
        );

        let timeline = read_timeline(sid).unwrap();
        let done_events: Vec<_> = timeline
            .iter()
            .filter(|event| event.event == "assistant_done")
            .collect();
        assert_eq!(done_events.len(), 1, "exactly one turn must be finished");
        // Tail-pop attribution: the terminal lands on the last-queued
        // second (the in-flight turn under single flight); stale first
        // produces no terminal event.
        assert_eq!(done_events[0].turn_id, second);
        // observation 字段随该次收尾落盘,不因重复收尾丢失。
        assert_eq!(done_events[0].tool_calls, Some(0));
        assert!(done_events[0].authorized_tool_catalog.is_some());

        // The queue has been fully cleared: later finishes are no-ops and must not record terminal events.
        finish_turn_with_observation(sid, "Completed", None, None, None);
        let timeline = read_timeline(sid).unwrap();
        let done_events: Vec<_> = timeline
            .iter()
            .filter(|event| event.event == "assistant_done")
            .collect();
        assert_eq!(
            done_events.len(),
            1,
            "finish on an emptied queue is a no-op"
        );
        assert!(
            !timeline
                .iter()
                .any(|event| event.turn_id == first && event.event == "assistant_done"),
            "stale unsubmitted-cancel residue must not receive a terminal"
        );

        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn records_first_turn_phases_and_terminal_authorized_catalog() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-phases-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // SAFETY: this test holds platform::paths::tests::ENV_LOCK; env writes are serialized.
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-phases";
        start_turn(sid);
        record_engine_turn_started(sid);
        record_engine_turn_started(sid);
        record_first_message_delta(sid);
        record_first_message_delta(sid);
        record_tool_started(sid, "tool_search");
        record_tool_completed(sid, "tool_search", true);
        record_tool_started(sid, "mcp_weather_get_weather");
        record_tool_completed(sid, "mcp_weather_get_weather", false);

        let catalog_json = br#"[{"name":"request_user_input"},{"name":"tool_search"}]"#;
        let catalog = ToolCatalogSummary::from_serialized_catalog(2, catalog_json);
        finish_turn_with_observation(
            sid,
            "Completed",
            None,
            Some(TurnUsage::default()),
            Some(catalog.clone()),
        );

        let timeline = read_timeline(sid).unwrap();
        let events = timeline
            .iter()
            .map(|event| event.event.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            events,
            vec![
                "user_start",
                "engine_turn_started",
                "first_message_delta",
                "first_tool_call_started",
                "first_tool_call_completed",
                "assistant_done",
            ]
        );

        let first_tool = timeline
            .iter()
            .find(|event| event.event == "first_tool_call_started")
            .expect("first tool phase");
        assert_eq!(first_tool.tool_name.as_deref(), Some("tool_search"));

        let done = timeline
            .iter()
            .find(|event| event.event == "assistant_done")
            .expect("assistant_done");
        assert_eq!(done.tool_calls, Some(2));
        assert_eq!(done.tool_failures, Some(1));
        assert_eq!(done.authorized_tool_catalog.as_ref(), Some(&catalog));
        assert_eq!(catalog.catalog_count, 2);
        assert_eq!(catalog.catalog_bytes, catalog_json.len() as u64);
        assert_eq!(catalog.catalog_sha256.len(), 64);

        let _ = std::fs::remove_dir_all(tmp);
    }

    /// 分支切换等破坏性操作的跨会话守卫依据：start_turn 登记后视为在途，
    /// finish_turn 收口后放行。
    #[test]
    fn has_active_turn_reflects_in_flight_sessions() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = std::env::temp_dir().join(format!(
            "pinvou3-timing-active-turn-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // edition 2024 起 set_var 为 unsafe；测试进程单线程持有 ENV_LOCK，安全。
        unsafe { std::env::set_var("PINVOU3_HOME", &tmp) };

        let sid = "session-active-turn-guard";
        assert!(!has_active_turn(sid));
        start_turn(sid);
        assert!(has_active_turn(sid));
        finish_turn(sid, "completed", None);
        assert!(!has_active_turn(sid));

        let _ = std::fs::remove_dir_all(tmp);
    }
}
