use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, ApprovalProfile, AuthStatus, ControlStrength,
    LogicalSessionId, ModelCatalog, ModelDescriptor, ModelId, NegotiatedCapabilities,
    PermissionCapability, RuntimeCapabilities, RuntimeCommand, RuntimeEventSubscription,
    RuntimeOperation, RuntimeSession, SessionDescriptor, SessionSnapshot, SessionStatus,
};

struct UnsupportedAdapter {
    probed: bool,
    capabilities: RuntimeCapabilities,
}

impl AgentRuntimeAdapter for UnsupportedAdapter {
    fn probe(&mut self) -> Result<(), AdapterError> {
        self.probed = true;
        Ok(())
    }
    fn capabilities(&self) -> Result<RuntimeCapabilities, AdapterError> {
        self.probed
            .then(|| self.capabilities.clone())
            .ok_or(AdapterError::NotProbed)
    }
    fn auth_status(&mut self) -> Result<AuthStatus, AdapterError> {
        Ok(AuthStatus::NotRequired)
    }
    fn subscribe_events(
        &mut self,
        _: &RuntimeSession,
    ) -> Result<RuntimeEventSubscription, AdapterError> {
        Err(AdapterError::unsupported("subscribe_events"))
    }
    fn close(&mut self, _: &RuntimeSession) -> Result<(), AdapterError> {
        Ok(())
    }
}

#[test]
fn seam_is_versioned_and_unsupported_operations_fail_explicitly() {
    let mut adapter = UnsupportedAdapter {
        probed: false,
        capabilities: RuntimeCapabilities::default(),
    };
    assert_eq!(adapter.interface_version(), 2);
    assert_eq!(adapter.capabilities(), Err(AdapterError::NotProbed));
    adapter.probe().unwrap();
    assert_eq!(
        adapter.capabilities().unwrap(),
        RuntimeCapabilities::default()
    );
    assert!(matches!(
        adapter.send(&RuntimeSession::new("session-1").unwrap(), RuntimeCommand::text("hello").unwrap()),
        Err(AdapterError::Unsupported { operation }) if operation == "send"
    ));
    let session = RuntimeSession::new("session-close").unwrap();
    adapter.close(&session).unwrap();
    adapter.close(&session).unwrap();
}

#[test]
fn structured_errors_have_stable_exit_classification() {
    use pinvou_protocol::StableExitCode;
    assert_eq!(
        AdapterError::NotProbed.exit_code(),
        StableExitCode::Internal
    );
    assert_eq!(
        AdapterError::BlockedAuth.exit_code(),
        StableExitCode::BlockedAuth
    );
    assert_eq!(
        AdapterError::HandshakeTimeout.exit_code(),
        StableExitCode::Cancelled
    );
    assert_eq!(
        AdapterError::QuotaExceeded.exit_code(),
        StableExitCode::RuntimeFailed
    );
    assert_eq!(
        AdapterError::ProcessExit {
            code: Some(9),
            signal: None,
            unexpected_eof: false,
            details: "failed".into()
        }
        .exit_code(),
        StableExitCode::RuntimeFailed
    );
}

#[test]
fn capabilities_round_trip_without_agent_name_inference() {
    let capabilities = RuntimeCapabilities {
        interactive_chat: true,
        native_resume: true,
        tool_approval: true,
        session_modes: vec!["workspace-write".into()],
        auth_flows: vec!["browser".into()],
        ..RuntimeCapabilities::default()
    };
    let json = serde_json::to_vec(&capabilities).unwrap();
    assert_eq!(
        serde_json::from_slice::<RuntimeCapabilities>(&json).unwrap(),
        capabilities
    );
}

#[test]
fn identifiers_and_commands_reject_empty_values() {
    assert!(RuntimeSession::new("").is_err());
    assert!(RuntimeCommand::text("").is_err());
}

#[test]
fn method_not_found_downgrades_the_negotiated_snapshot() {
    let mut state = NegotiatedCapabilities::default();
    assert_eq!(state.snapshot(), Err(AdapterError::NotProbed));
    state.complete(RuntimeCapabilities {
        steering: true,
        ..RuntimeCapabilities::default()
    });
    state.method_not_found("steer").unwrap();
    assert!(!state.snapshot().unwrap().steering);
}

#[test]
fn new_capability_evidence_is_backward_compatible() {
    let capabilities: RuntimeCapabilities = serde_json::from_value(serde_json::json!({
        "interactive_chat": true
    }))
    .unwrap();

    assert!(capabilities.interactive_chat);
    assert!(!capabilities.session_listing);
    assert!(!capabilities.model_catalog);
    assert!(!capabilities.model_switching);
    assert!(!capabilities.permission_profiles);
}

#[test]
fn session_snapshot_round_trips_with_stable_snake_case_fields() {
    let descriptor = SessionDescriptor {
        id: LogicalSessionId::new("session-1").unwrap(),
        title: "First task".into(),
        last_active_at: "2026-08-25T10:00:00Z".into(),
        runtime_id: "codex".into(),
        model_id: Some(ModelId::new("gpt-5.6").unwrap()),
        status: SessionStatus::Completed,
        native_session_id: Some("thread-1".into()),
    };
    let snapshot = SessionSnapshot {
        descriptor,
        cursor: 7,
        normalized_events: vec![serde_json::json!({"kind":"message_completed"})],
    };

    let value = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(value["descriptor"]["status"], "completed");
    assert_eq!(value["descriptor"]["native_session_id"], "thread-1");
    assert_eq!(value["cursor"], 7);
    assert_eq!(
        serde_json::from_value::<SessionSnapshot>(value).unwrap(),
        snapshot
    );
}

#[test]
fn model_catalog_rejects_invalid_default_or_current_model() {
    let models = vec![
        ModelDescriptor::new("gpt-5.6", "GPT-5.6", true, true).unwrap(),
        ModelDescriptor::new("gpt-5.5", "GPT-5.5", true, true).unwrap(),
    ];
    assert!(ModelCatalog::new("codex", None, models).is_err());

    let models = vec![ModelDescriptor::new("gpt-5.6", "GPT-5.6", true, false).unwrap()];
    assert!(ModelCatalog::new("codex", Some(ModelId::new("missing").unwrap()), models).is_err());
}

#[test]
fn permission_capability_uses_stable_product_profiles() {
    let capability = PermissionCapability {
        supported_profiles: vec![ApprovalProfile::Request, ApprovalProfile::Assisted],
        control_strength: ControlStrength::Partial,
        native_mode: Some("on-request".into()),
        sandbox: Some("workspace-write".into()),
        residual_guards: vec!["os-policy".into()],
        evidence_version: "codex-app-server-v2".into(),
    };

    let value = serde_json::to_value(&capability).unwrap();
    assert_eq!(value["supported_profiles"][0], "request");
    assert_eq!(value["control_strength"], "partial");
    assert_eq!(
        serde_json::from_value::<PermissionCapability>(value).unwrap(),
        capability
    );
}

#[test]
fn new_adapter_operations_default_to_explicit_unsupported_errors() {
    let mut adapter = UnsupportedAdapter {
        probed: true,
        capabilities: RuntimeCapabilities::default(),
    };
    let operation = || RuntimeOperation::new("operation-1", serde_json::json!({})).unwrap();

    assert!(
        matches!(adapter.list_sessions(operation()), Err(AdapterError::Unsupported { operation }) if operation == "list_sessions")
    );
    assert!(
        matches!(adapter.read_session(operation()), Err(AdapterError::Unsupported { operation }) if operation == "read_session")
    );
    assert!(
        matches!(adapter.list_models(operation()), Err(AdapterError::Unsupported { operation }) if operation == "list_models")
    );
    assert!(
        matches!(adapter.inspect_permissions(operation()), Err(AdapterError::Unsupported { operation }) if operation == "inspect_permissions")
    );
}

#[test]
fn public_identifiers_reject_empty_values() {
    assert!(LogicalSessionId::new("").is_err());
    assert!(ModelId::new("").is_err());
}
