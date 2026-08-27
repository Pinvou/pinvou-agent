use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_controller::{
    ControllerError, ControllerSession, SessionStore, StoredSessionMetadata, WorkspaceStore,
};
use pinvou_protocol::IpcMessage;
use pinvou_runtime_api::{LogicalSessionId, ModelId, SessionDescriptor, SessionStatus};

fn temp_root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "pinvou-controller-session-control-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn controller_lists_current_workspace_sessions_and_prepares_bound_resume_token() {
    let root = temp_root();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace_store = WorkspaceStore::open(&root).unwrap();
    let workspace_key = workspace_store.workspace_key(&workspace).unwrap();
    let mut sessions = SessionStore::open(&root).unwrap();
    sessions
        .create_session(StoredSessionMetadata::for_workspace(
            SessionDescriptor {
                id: LogicalSessionId::new("logical-1").unwrap(),
                title: "Saved task".into(),
                last_active_at: "2026-08-25T10:00:00Z".into(),
                runtime_id: "codex".into(),
                model_id: Some(ModelId::new("gpt-5.6").unwrap()),
                status: SessionStatus::Completed,
                native_session_id: Some("thread-1".into()),
            },
            3,
            workspace_key,
        ))
        .unwrap();
    drop(sessions);
    let session = ControllerSession::with_storage("instance-a", &root, &workspace).unwrap();

    let list = IpcMessage::request(
        serde_json::json!(1),
        "session.list",
        serde_json::json!({"instance_id":"instance-a"}),
    )
    .unwrap();
    let response = session.handle_bound(list).unwrap();
    assert_eq!(response.payload()["sessions"][0]["id"], "logical-1");

    let prepare = IpcMessage::request(
        serde_json::json!(2),
        "session.resume.prepare",
        serde_json::json!({"instance_id":"instance-a","session_id":"logical-1"}),
    )
    .unwrap();
    let prepared = session.handle_bound(prepare).unwrap();
    assert_eq!(prepared.payload()["status"], "ready");
    assert_eq!(prepared.payload()["session_id"], "logical-1");
    assert_eq!(prepared.payload()["attachment_epoch"], 3);
    assert!(
        prepared.payload()["resume_token"]
            .as_str()
            .unwrap()
            .starts_with("instance-a:")
    );

    let commit = IpcMessage::request(
        serde_json::json!(3),
        "session.resume.commit",
        serde_json::json!({
            "instance_id":"instance-a",
            "resume_token":"instance-b:1"
        }),
    )
    .unwrap();
    assert!(matches!(
        session.handle_bound(commit),
        Err(ControllerError::InvalidMessage)
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn controller_returns_persisted_sessions_when_native_listing_is_unavailable() {
    let root = temp_root();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace_store = WorkspaceStore::open(&root).unwrap();
    let workspace_key = workspace_store.workspace_key(&workspace).unwrap();
    let mut sessions = SessionStore::open(&root).unwrap();
    sessions
        .create_session(StoredSessionMetadata::for_workspace(
            SessionDescriptor {
                id: LogicalSessionId::new("logical-offline").unwrap(),
                title: "Offline task".into(),
                last_active_at: "2026-08-25T11:00:00Z".into(),
                runtime_id: "codex".into(),
                model_id: None,
                status: SessionStatus::Completed,
                native_session_id: Some("thread-offline".into()),
            },
            1,
            workspace_key,
        ))
        .unwrap();
    drop(sessions);
    let session = ControllerSession::with_local_node_and_storage(
        "instance-a",
        r"\\.\pipe\pinvou-node-does-not-exist",
        "node-instance",
        &root,
        &workspace,
    )
    .unwrap();

    let request = IpcMessage::request(
        serde_json::json!(1),
        "session.list",
        serde_json::json!({"instance_id":"instance-a"}),
    )
    .unwrap();
    let response = session.handle_bound(request).unwrap();

    assert_eq!(response.payload()["sessions"][0]["id"], "logical-offline");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn controller_repairs_interrupted_runtime_switch_mapping_and_hides_native_mirror() {
    let root = temp_root();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace_store = WorkspaceStore::open(&root).unwrap();
    let workspace_key = workspace_store.workspace_key(&workspace).unwrap();
    let logical_id = LogicalSessionId::new("pinvou-logical").unwrap();
    let native_id = LogicalSessionId::new("codex-native").unwrap();
    let mut sessions = SessionStore::open(&root).unwrap();
    sessions
        .create_session(StoredSessionMetadata::for_workspace(
            SessionDescriptor {
                id: logical_id.clone(),
                title: "Complete cross-runtime history".into(),
                last_active_at: "2026-08-25T11:00:00Z".into(),
                runtime_id: "codex".into(),
                model_id: None,
                status: SessionStatus::Active,
                native_session_id: None,
            },
            1,
            workspace_key.clone(),
        ))
        .unwrap();
    sessions
        .append_event(
            &logical_id,
            serde_json::json!({
                "protocol_version":1,
                "schema_version":1,
                "node_id":"node",
                "logical_session_id":"codex-native",
                "attachment_id":"codex-attachment",
                "work_id":null,
                "collaborative_run_id":null,
                "stream_id":"main",
                "turn_id":"turn-a",
                "seq":1,
                "source_span":null,
                "timestamp":"2026-08-25T11:00:00.000Z",
                "rate_class":"R1",
                "kind":"text.delta",
                "payload":{"role":"assistant","content":"working"}
            }),
        )
        .unwrap();
    sessions
        .create_session(StoredSessionMetadata::for_workspace(
            SessionDescriptor {
                id: native_id,
                title: "Native mirror".into(),
                last_active_at: "2026-08-25T11:01:00Z".into(),
                runtime_id: "codex".into(),
                model_id: None,
                status: SessionStatus::Unknown,
                native_session_id: Some("codex-native".into()),
            },
            1,
            workspace_key,
        ))
        .unwrap();
    drop(sessions);

    let session = ControllerSession::with_storage("instance-a", &root, &workspace).unwrap();
    let request = IpcMessage::request(
        serde_json::json!(1),
        "session.list",
        serde_json::json!({"instance_id":"instance-a"}),
    )
    .unwrap();
    let response = session.handle_bound(request).unwrap();
    let listed = response.payload()["sessions"].as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], "pinvou-logical");
    assert_eq!(listed[0]["native_session_id"], "codex-native");

    drop(session);
    let repaired = SessionStore::open(&root)
        .unwrap()
        .metadata(&logical_id)
        .unwrap();
    assert_eq!(
        repaired.descriptor.native_session_id.as_deref(),
        Some("codex-native")
    );
    std::fs::remove_dir_all(root).unwrap();
}
