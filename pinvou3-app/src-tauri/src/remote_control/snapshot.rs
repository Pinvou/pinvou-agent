use serde_json::{json, Value};

use crate::bridge::sessions::SessionStore;

pub fn build_session_snapshot(store: &SessionStore, session_id: &str) -> Result<Value, String> {
    let saved = store
        .load(session_id)
        .map_err(|e| format!("load session snapshot({session_id}): {e:?}"))?;
    let mode = store.mode_state(session_id).mode;
    let messages = saved
        .messages
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| project_message(idx, msg))
        .collect::<Vec<_>>();
    let artifacts = saved
        .artifacts
        .iter()
        .map(|a| {
            let path = a.storage_path.to_string_lossy().to_string();
            json!({
                "id": a.id,
                "basename": a.storage_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
                "path_tail": tail_path(&path),
                "kind": format!("{:?}", a.kind),
                "byte_size": a.byte_size,
                "created_at": a.created_at,
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "snapshot_source": "store",
        "session": {
            "id": saved.metadata.id,
            "title": saved.metadata.title,
            "mode": mode,
            "status": "idle",
            "updated_at": saved.metadata.updated_at,
            "message_count": saved.metadata.message_count,
        },
        "messages": messages,
        "pending_user_inputs": [],
        "running_tools": [],
        "artifacts": artifacts,
    }))
}

fn project_message(idx: usize, msg: &deepseek_tui::models::Message) -> Option<Value> {
    let role = serde_json::to_value(&msg.role).ok()?;
    let role = role.as_str().unwrap_or("assistant").to_string();
    let blocks = serde_json::to_value(&msg.content).ok()?;
    let mut text = String::new();
    let mut tools = Vec::new();
    let mut has_tool_result = false;
    if let Some(arr) = blocks.as_array() {
        for block in arr {
            let typ = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match typ {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        text.push_str(t);
                    }
                }
                "tool_use" => {
                    tools.push(json!({
                        "id": block.get("id").cloned().unwrap_or(Value::Null),
                        "name": block.get("name").cloned().unwrap_or(Value::Null),
                        "args": block.get("input").cloned().unwrap_or(Value::Null),
                    }));
                }
                "tool_result" => {
                    has_tool_result = true;
                }
                _ => {}
            }
        }
    }
    if text.trim().is_empty() && tools.is_empty() && !has_tool_result {
        return None;
    }
    Some(json!({
        "id": format!("m_{idx}"),
        "role": role,
        "content": text,
        "tools": tools,
        "blocks": blocks,
        "created_at": null,
    }))
}

fn tail_path(path: &str) -> String {
    let parts = path.split('/').rev().take(3).collect::<Vec<_>>();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}
