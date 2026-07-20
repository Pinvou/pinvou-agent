//! Best-effort per-turn timing + usage/cost events.
//!
//! This intentionally stays outside SavedSession/messages so timing telemetry
//! never changes model context, session schema, or artifact behavior.
//!
//! 2026-07 升级:`assistant_done` 事件追加 `usage`(input/output/cache tokens)字段,
//! 让历史面板 / replay / cost 统计有数据源。老 session 的旧事件无 usage 字段,
//! 反序列化按缺失处理(`Option`),Timeline API 对老 session 也安全返回。

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

static TURN_SEQ: AtomicU64 = AtomicU64::new(1);
static ACTIVE_TURNS: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();

fn active_turns() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    ACTIVE_TURNS.get_or_init(|| Mutex::new(HashMap::new()))
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
    let path = crate::bridge::paths::session_timing_events(session_id);
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
/// 与上游 `deepseek_tui::models::Usage` 字段集对齐(forward-compat),让历史面板 /
/// replay / 真实 cost 估算有完整数据源。老 session 的旧事件缺这些字段时按缺失
/// 反序列化为 0(见 `read_timeline`)。
///
/// - `input_tokens` / `output_tokens`:基础输入输出(u32 升 u64)。
/// - `cache_hit_tokens` / `cache_miss_tokens`:Anthropic 风格 prompt cache 命中 /
///   未命中(u32 升 u64)。非 Anthropic provider 全为 0。
/// - `cache_write_tokens`:`cache_creation_input_tokens`,按 cache-write 计费
///   (Anthropic 1.25x)。缺这字段会让 cost 估算系统性少算。
/// - `reasoning_tokens`:推理 token(DeepSeek V4 等思考模型的重要成本来源)。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
}

pub fn start_turn(session_id: &str) -> String {
    let turn_id = new_turn_id(session_id);
    if let Ok(mut map) = active_turns().lock() {
        map.entry(session_id.to_string())
            .or_default()
            .push_back(turn_id.clone());
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

/// 旧调用点(无 usage):落盘不带 usage 字段,reader API 反序列化为 None。
/// 保留是为了不强迫所有调用点同步升级(commands.rs:200 的 send_error 兜底等)。
pub fn finish_turn(session_id: &str, status: &str, error: Option<&str>) {
    finish_turn_with_usage(session_id, status, error, None);
}

/// 收尾本轮并把 usage 落进 `assistant_done` 事件。
///
/// `usage: Option<TurnUsage>` —— None 时(老路径 / 失败兜底)不写 usage 字段,
/// reader API 按缺失处理。Some 时写入 4 个 token 计数,前端可估算 cost。
pub fn finish_turn_with_usage(
    session_id: &str,
    status: &str,
    error: Option<&str>,
    usage: Option<TurnUsage>,
) {
    let turn_id = active_turns()
        .lock()
        .ok()
        .and_then(|mut map| {
            let queue = map.get_mut(session_id)?;
            let id = queue.pop_front();
            if queue.is_empty() {
                map.remove(session_id);
            }
            id
        });

    let Some(turn_id) = turn_id else {
        return;
    };

    let mut entry = json!({
        "event": "assistant_done",
        "session_id": session_id,
        "turn_id": turn_id,
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
        });
    }
    append_event(session_id, entry);
}

/// 解析后的 timeline 事件。user_start 与 assistant_done 按同一 turn_id 配对。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub turn_id: String,
    pub event: String,           // "user_start" | "assistant_done"
    pub timestamp: i64,          // ms epoch
    pub ts: String,              // RFC3339
    pub status: Option<String>,  // assistant_done only
    pub error: Option<String>,   // assistant_done only
    pub usage: Option<TurnUsage>, // assistant_done only(老事件为 None)
}

/// 读取 session 的全部 timeline 事件,按 timestamp 升序。
///
/// 文件不存在 / 解析失败的行被静默跳过(append-only jsonl 的单行损坏不应让整个 timeline 崩)。
/// 空文件 / 文件不存在 → 空向量。
pub fn read_timeline(session_id: &str) -> Vec<TimelineEvent> {
    let path = crate::bridge::paths::session_timing_events(session_id);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut events: Vec<TimelineEvent> = content
        .lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                return None;
            }
            // 反序列化成宽 schema(用 Value 再取字段),避免缺字段时整条事件丢
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let usage = v.get("usage").and_then(|u| {
                if u.is_null() {
                    return None;
                }
                Some(TurnUsage {
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
                })
            });
            Some(TimelineEvent {
                turn_id: v.get("turn_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                event: v.get("event").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                timestamp: v.get("timestamp").and_then(|x| x.as_i64()).unwrap_or(0),
                ts: v.get("ts").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                status: v
                    .get("status")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
                error: v
                    .get("error")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                usage,
            })
        })
        .collect();
    events.sort_by_key(|e| e.timestamp);
    events
}

/// session 级聚合统计(给历史面板卡片用)。
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
}

/// 聚合 session timeline 为单个 stats 对象。
///
/// 算法:遍历 read_timeline(),按 turn_id 配对 (user_start, assistant_done),
/// 每对算一个 turn。token / 状态从 assistant_done 取(失败时 usage 可能为 None)。
pub fn compute_stats(session_id: &str) -> SessionTimelineStats {
    let events = read_timeline(session_id);
    let mut stats = SessionTimelineStats::default();
    // 按 turn_id 索引;一个 turn 由 user_start + assistant_done 组成
    let mut by_turn: HashMap<String, (&TimelineEvent, Option<&TimelineEvent>)> = HashMap::new();
    for e in &events {
        let entry = by_turn
            .entry(e.turn_id.clone())
            .or_insert((e, None));
        if e.event == "assistant_done" {
            entry.1 = Some(e);
        } else if e.event == "user_start" && entry.0.event != "user_start" {
            entry.0 = e;
        }
    }
    stats.turn_count = by_turn.len();
    stats.first_turn_ts = events.first().map(|e| e.ts.clone());
    stats.last_turn_ts = events.last().map(|e| e.ts.clone());
    for (_, done) in by_turn.values() {
        if let Some(d) = done {
            if let Some(u) = &d.usage {
                stats.total_input_tokens += u.input_tokens;
                stats.total_output_tokens += u.output_tokens;
                stats.total_cache_hit_tokens += u.cache_hit_tokens;
                stats.total_cache_miss_tokens += u.cache_miss_tokens;
                stats.total_cache_write_tokens += u.cache_write_tokens;
                stats.total_reasoning_tokens += u.reasoning_tokens;
            }
            // [F2] eq_ignore_ascii_case 统一大小写匹配,识别全部三个终态:
            //   Completed / Interrupted / Failed。
            // 之前用硬展开 `Some("Completed") | Some("completed")` 既漏 Interrupted,
            // 又容易再漏一个变体(engine.rs 上游 `format!("{terminal_status:?}")`
            // 永远是 PascalCase,但脏数据/老格式可能是 lowercase)。
            match d.status.as_deref() {
                Some(s) if s.eq_ignore_ascii_case("Completed") => stats.completed_turns += 1,
                Some(s) if s.eq_ignore_ascii_case("Failed") => stats.failed_turns += 1,
                Some(s) if s.eq_ignore_ascii_case("Interrupted") => stats.interrupted_turns += 1,
                _ => {}
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::paths::tests::ENV_LOCK;

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
        std::env::set_var("PINVOU3_HOME", &tmp);

        let sid = "session-a";
        let turn_id = start_turn(sid);
        finish_turn(sid, "completed", None);

        let content =
            std::fs::read_to_string(crate::bridge::paths::session_timing_events(sid)).unwrap();
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
        std::env::set_var("PINVOU3_HOME", &tmp);

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
            }),
        );

        let timeline = read_timeline(sid);
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
    fn read_timeline_handles_missing_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var(
            "PINVOU3_HOME",
            std::env::temp_dir().join(format!("nonexistent-{}", now_ms())),
        );
        let timeline = read_timeline("never-existed");
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
        std::env::set_var("PINVOU3_HOME", &tmp);

        let sid = "session-corrupt";
        start_turn(sid);
        // 注入一行坏 JSON
        let path = crate::bridge::paths::session_timing_events(sid);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(b"this is not json\n"))
            .unwrap();
        finish_turn(sid, "completed", None);

        // 应该跳过坏行,只拿到 2 条事件
        let timeline = read_timeline(sid);
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
        std::env::set_var("PINVOU3_HOME", &tmp);

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
                }),
            );
        }
        start_turn(sid);
        finish_turn_with_usage(sid, "Failed", Some("oops"), None);

        let stats = compute_stats(sid);
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
        std::env::set_var(
            "PINVOU3_HOME",
            std::env::temp_dir().join(format!("nope-{}", now_ms())),
        );
        let stats = compute_stats("does-not-exist");
        assert_eq!(stats.turn_count, 0);
        assert_eq!(stats.total_input_tokens, 0);
        assert!(stats.first_turn_ts.is_none());
    }
}
