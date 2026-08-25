use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use pinvou_controller::{
    SessionStore, StoredSessionMetadata, WorkspacePreferences, WorkspaceStore,
};
use pinvou_runtime_api::{
    ApprovalProfile, LogicalSessionId, ModelId, SessionDescriptor, SessionSnapshot, SessionStatus,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "pinvou-controller-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn descriptor() -> SessionDescriptor {
    SessionDescriptor {
        id: LogicalSessionId::new("logical-1").unwrap(),
        title: "Saved task".into(),
        last_active_at: "2026-08-25T10:00:00Z".into(),
        runtime_id: "codex".into(),
        model_id: Some(ModelId::new("gpt-5.6").unwrap()),
        status: SessionStatus::Completed,
        native_session_id: Some("thread-1".into()),
    }
}

#[test]
fn session_store_reopens_snapshot_and_replays_only_events_after_cursor() {
    let root = temp_root("session-store");
    let session_id = LogicalSessionId::new("logical-1").unwrap();
    {
        let mut store = SessionStore::open(&root).unwrap();
        store
            .create_session(StoredSessionMetadata::new(descriptor(), 4))
            .unwrap();
        let first = store
            .append_event(
                &session_id,
                serde_json::json!({"kind":"message","text":"one"}),
            )
            .unwrap();
        assert_eq!(first, 1);
        store
            .write_snapshot(
                &session_id,
                SessionSnapshot {
                    descriptor: descriptor(),
                    cursor: first,
                    normalized_events: vec![serde_json::json!({"kind":"message","text":"one"})],
                },
            )
            .unwrap();
        assert_eq!(
            store
                .append_event(
                    &session_id,
                    serde_json::json!({"kind":"message","text":"two"})
                )
                .unwrap(),
            2
        );
    }

    let store = SessionStore::open(&root).unwrap();
    let restored = store.restore(&session_id).unwrap();
    assert_eq!(restored.cursor, 2);
    assert_eq!(restored.normalized_events.len(), 2);
    assert_eq!(restored.normalized_events[0]["text"], "one");
    assert_eq!(restored.normalized_events[1]["text"], "two");
    assert_eq!(store.list().len(), 1);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_preferences_round_trip_without_using_raw_path_as_directory_name() {
    let root = temp_root("workspace-store");
    let workspace = root.join("private customer workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let store = WorkspaceStore::open(root.join("data")).unwrap();
    let preferences = WorkspacePreferences {
        runtime: Some("codex".into()),
        model_by_runtime: BTreeMap::from([("codex".into(), "gpt-5.6".into())]),
        approval_profile: ApprovalProfile::Request,
        recent_session: Some(LogicalSessionId::new("logical-1").unwrap()),
    };

    store.save(&workspace, &preferences).unwrap();

    assert_eq!(store.load(&workspace).unwrap(), Some(preferences));
    let entries = std::fs::read_dir(root.join("data/workspaces"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].contains("private customer workspace"));

    std::fs::remove_dir_all(root).unwrap();
}
