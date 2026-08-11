use serde_json::{json, Value};

use crate::features::sessions::SessionStore;

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
    let normalized = path.replace('\\', "/");
    let parts = normalized.split('/').rev().take(3).collect::<Vec<_>>();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mode_state::SerializableMode;
    use crate::platform::paths;
    use crate::platform::paths::tests::ENV_LOCK;
    use deepseek_tui::models::{ContentBlock, Message};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct PinvouHomeOverride {
        previous: Option<OsString>,
        root: PathBuf,
    }

    impl PinvouHomeOverride {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "pinvou3-remote-snapshot-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_nanos())
                    .unwrap_or(0)
            ));
            let previous = std::env::var_os("PINVOU3_HOME");
            std::env::set_var("PINVOU3_HOME", &root);
            Self { previous, root }
        }
    }

    impl Drop for PinvouHomeOverride {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(previous) => std::env::set_var("PINVOU3_HOME", previous),
                None => std::env::remove_var("PINVOU3_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
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

    #[test]
    fn snapshot_projects_messages_tools_mode_and_artifacts() {
        let _env_lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = PinvouHomeOverride::new();
        let store = SessionStore::boot().expect("boot isolated session store");
        let saved = store
            .create_new("test-model".into(), None, paths::workspace_dir())
            .expect("create session");
        let session_id = saved.metadata.id;
        let messages = vec![
            text_message("user", "生成远程报告"),
            Message {
                role: "assistant".into(),
                content: vec![
                    ContentBlock::Text {
                        text: "报告已生成".into(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tool-1".into(),
                        name: "write_file".into(),
                        input: json!({"path": "output.md"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".into(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-1".into(),
                    content: "ok".into(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        store
            .update_messages(&session_id, messages)
            .expect("persist messages");
        store
            .set_mode(&session_id, SerializableMode::Plan)
            .expect("set plan mode");
        // 产物落在账本根下：经 SessionStore::session_roots 取根，不直接拼 paths。
        let artifact = store
            .session_roots(&session_id)
            .expect("resolve session roots")
            .ledger
            .join("output.md");
        std::fs::create_dir_all(artifact.parent().expect("artifact parent"))
            .expect("create artifact parent");
        std::fs::write(&artifact, "# report\n").expect("write artifact");
        store
            .update_artifacts(&session_id, vec![artifact.to_string_lossy().to_string()])
            .expect("persist artifact");

        let snapshot = build_session_snapshot(&store, &session_id).expect("build snapshot");
        assert_eq!(snapshot["snapshot_source"], "store");
        assert_eq!(snapshot["session"]["id"], session_id);
        assert_eq!(snapshot["session"]["mode"], "plan");
        assert_eq!(snapshot["session"]["message_count"], 3);
        assert_eq!(snapshot["messages"].as_array().expect("messages").len(), 3);
        assert_eq!(snapshot["messages"][1]["content"], "报告已生成");
        assert_eq!(snapshot["messages"][1]["tools"][0]["name"], "write_file");
        assert_eq!(snapshot["messages"][2]["content"], "");
        assert_eq!(
            snapshot["artifacts"].as_array().expect("artifacts").len(),
            1
        );
        assert_eq!(snapshot["artifacts"][0]["basename"], "output.md");
        assert!(snapshot["artifacts"][0]["path_tail"]
            .as_str()
            .expect("path tail")
            .ends_with("workspace/output.md"));
    }

    #[test]
    fn snapshot_path_tail_is_cross_platform_and_empty_messages_are_dropped() {
        assert_eq!(
            tail_path(r"C:\pinvou3\sessions\session-1\workspace\report.md"),
            "session-1/workspace/report.md"
        );
        let empty = Message {
            role: "assistant".into(),
            content: Vec::new(),
        };
        assert!(project_message(0, &empty).is_none());
    }
}
