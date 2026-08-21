use pinvou_runtime_api::{
    AdapterError, AgentRuntimeAdapter, AuthStatus, NegotiatedCapabilities, RuntimeCapabilities,
    RuntimeCommand, RuntimeEventSubscription, RuntimeSession,
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
    assert_eq!(adapter.interface_version(), 1);
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
