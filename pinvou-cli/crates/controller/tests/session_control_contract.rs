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
