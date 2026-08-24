use pinvou_agent_adapter_codex::{
    CodexEventProjector, MAX_JSON_LINE_BYTES, ProjectedFrame, redact_diagnostic,
};
use pinvou_protocol::RateClass;
use serde_json::Value;

#[test]
fn captured_fixture_maps_to_versioned_runtime_events() {
    let mut projector = CodexEventProjector::new("node-fixture", "attachment-fixture");
    let mut kinds = Vec::new();
    for line in include_str!("fixtures/replay.jsonl").lines() {
        let frame: Value = serde_json::from_str(line).unwrap();
        if let ProjectedFrame::Event(event) = projector.project(&frame).unwrap() {
            kinds.push((event.kind().to_owned(), event.rate_class()));
        }
    }
    assert_eq!(
        kinds,
        [
            ("attachment.started".into(), RateClass::R0),
            ("turn.started".into(), RateClass::R0),
            ("text.delta".into(), RateClass::R1),
            ("thinking.delta".into(), RateClass::R1),
            ("approval.requested".into(), RateClass::R0),
            ("usage.reported".into(), RateClass::R1),
            ("turn.ended".into(), RateClass::R0),
        ]
    );
}

#[test]
fn unknown_notification_is_preserved_as_conservative_r1_vendor_event() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({"method":"mcpServer/startupStatus/updated","params":{"threadId":"thread","name":"fixture","status":"ready","error":null}});
    let ProjectedFrame::Event(event) = projector.project(&frame).unwrap() else {
        panic!("notification was not projected")
    };
    assert_eq!(event.kind(), "vendor");
    assert_eq!(event.rate_class(), RateClass::R1);
    assert_eq!(
        event.vendor_extension().unwrap()["method"],
        "mcpServer/startupStatus/updated"
    );
}

#[test]
fn remote_control_status_notification_is_benign_startup_telemetry() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({
        "method":"remoteControl/status/changed",
        "params":{
            "status":"disabled",
            "serverName":"local",
            "installationId":"install",
            "environmentId":null
        }
    });
    let ProjectedFrame::Event(event) = projector.project(&frame).unwrap() else {
        panic!("remote control status was not projected")
    };
    assert_eq!(event.kind(), "vendor");
    assert_eq!(event.rate_class(), RateClass::R1);
    assert_eq!(
        event.vendor_extension().unwrap()["method"],
        "remoteControl/status/changed"
    );
}

#[test]
fn canonical_user_message_lifecycle_is_benign_and_never_projected_as_tool_activity() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let started = serde_json::json!({
        "method":"item/started",
        "params":{
            "threadId":"thread",
            "turnId":"turn",
            "item":{"type":"userMessage","id":"user-1","content":[{"type":"text","text":"hello"}]}
        }
    });
    let completed = serde_json::json!({
        "method":"item/completed",
        "params":{
            "threadId":"thread",
            "turnId":"turn",
            "item":{"type":"userMessage","id":"user-1","content":[{"type":"text","text":"hello"}]}
        }
    });

    assert!(matches!(
        projector.project(&started).unwrap(),
        ProjectedFrame::Ignored
    ));
    assert!(matches!(
        projector.project(&completed).unwrap(),
        ProjectedFrame::Ignored
    ));
}

#[test]
fn canonical_reasoning_and_plan_completion_lifecycle_is_benign() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    for item_type in ["reasoning", "plan"] {
        let completed = serde_json::json!({
            "method":"item/completed",
            "params":{
                "threadId":"thread",
                "turnId":"turn",
                "item":{"type":item_type,"id":"item-1","summary":[],"content":[]}
            }
        });
        assert!(matches!(
            projector.project(&completed).unwrap(),
            ProjectedFrame::Ignored
        ));
    }
}

#[test]
fn unknown_server_request_fails_closed() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({"id":44,"method":"future/requestPermission","params":{}});
    let error = projector.project(&frame).unwrap_err();
    assert!(format!("{error}").contains("unsupported_control_event"));
}

#[test]
fn approval_projection_retains_the_exact_rpc_id_for_one_shot_resolution() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({"id":91,"method":"item/fileChange/requestApproval","params":{"threadId":"thread","itemId":"change-1"}});
    assert!(matches!(
        projector.project(&frame).unwrap(),
        ProjectedFrame::Event(_)
    ));
    let control = projector.take_pending_control().unwrap();
    assert_eq!(
        control,
        pinvou_agent_adapter_codex::PendingControl::Approval {
            request_id: serde_json::json!(91),
            approval_id: "change-1".into(),
            thread_id: "thread".into(),
            response: pinvou_agent_adapter_codex::ApprovalResponse::Decision,
        }
    );
    assert!(projector.take_pending_control().is_none());
}

#[test]
fn quota_and_sensitive_diagnostics_are_classified_without_leaking_secrets() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({"method":"error","params":{"error":{
        "message":"usage limit exceeded bearer abc123 api_key=secret",
        "codexErrorInfo":"usageLimitExceeded"
    }}});
    let error = projector.project(&frame).unwrap_err();
    assert_eq!(error, pinvou_runtime_api::AdapterError::QuotaExceeded);
    let redacted =
        redact_diagnostic("Authorization: Bearer abc123; api_key=secret; sk-live-secret");
    assert!(!redacted.contains("abc123"));
    assert!(!redacted.contains("secret"));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn line_limit_is_fixed_to_transport_frame_limit() {
    assert_eq!(MAX_JSON_LINE_BYTES, 16 * 1024 * 1024);
}

#[test]
fn control_and_main_sequences_are_independent() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let control = projector.project(&serde_json::json!({"method":"turn/started","params":{"threadId":"t","turn":{"id":"u"}}})).unwrap();
    let main = projector.project(&serde_json::json!({"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"u","delta":"x"}})).unwrap();
    let ProjectedFrame::Event(control) = control else {
        panic!()
    };
    let ProjectedFrame::Event(main) = main else {
        panic!()
    };
    assert_eq!(control.seq(), 1);
    assert_eq!(main.seq(), 1);
}

#[test]
fn approval_id_precedes_item_and_call_identity() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    projector.project(&serde_json::json!({"id":5,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread","approvalId":"callback","itemId":"item","callId":"call","command":"echo"}})).unwrap();
    let pinvou_agent_adapter_codex::PendingControl::Approval { approval_id, .. } =
        projector.take_pending_control().unwrap()
    else {
        panic!()
    };
    assert_eq!(approval_id, "callback");
}

#[test]
fn input_request_preserves_real_questions_shape() {
    let mut projector = CodexEventProjector::new("node", "attachment");
    let frame = serde_json::json!({"id":7,"method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"u","itemId":"input","questions":[{"id":"q1","header":"Choice","question":"Pick","options":null}]}});
    let ProjectedFrame::Event(event) = projector.project(&frame).unwrap() else {
        panic!()
    };
    let payload: Value = serde_json::from_str(event.payload().get()).unwrap();
    assert_eq!(payload["schema"]["questions"][0]["id"], "q1");
}
