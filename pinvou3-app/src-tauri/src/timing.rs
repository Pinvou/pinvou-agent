//! Best-effort per-turn timing events.
//!
//! This intentionally stays outside SavedSession/messages so timing telemetry
//! never changes model context, session schema, or artifact behavior.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use chrono::Utc;
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

pub fn finish_turn(session_id: &str, status: &str, error: Option<&str>) {
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

    append_event(
        session_id,
        json!({
            "event": "assistant_done",
            "session_id": session_id,
            "turn_id": turn_id,
            "timestamp": now_ms(),
            "ts": Utc::now().to_rfc3339(),
            "status": status,
            "error": error,
        }),
    );
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

        let _ = std::fs::remove_dir_all(tmp);
    }
}
