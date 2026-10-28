//! Closed diagnostic DATA. No clock, cleanup, installation or native authority.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedTimeState {
    Unsupported,
    SetupRequired,
    MissingPolicy,
    Unknown,
    SourceUnavailable,
    CleanupPending,
    /// Fresh original installation: authenticated zero time-obligation census.
    /// This is not Known time or preparation/installation permission.
    CleanupComplete,
    IntegrityFault,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedTimeRefreshFailure {
    Busy,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedTimeStatus {
    pub protocol_version: u8,
    pub state: TrustedTimeState,
    /// Pending/running activity only, never successful time or upgrade rights.
    pub refreshing: bool,
    /// Last completed attempt's contention, independent of protected time state.
    pub refresh_failure: Option<TrustedTimeRefreshFailure>,
}
impl TrustedTimeStatus {
    pub(crate) fn new(state: TrustedTimeState, refreshing: bool) -> Self {
        Self {
            protocol_version: 1,
            state,
            refreshing,
            refresh_failure: None,
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.protocol_version != 1 {
            return Err(Error::IpcInvalid);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedTimeRefreshState {
    Accepted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedTimeRefreshReply {
    pub protocol_version: u8,
    pub state: TrustedTimeRefreshState,
}
impl TrustedTimeRefreshReply {
    pub(crate) fn accepted() -> Self {
        Self {
            protocol_version: 1,
            state: TrustedTimeRefreshState::Accepted,
        }
    }
    pub(crate) fn validate(&self) -> Result<()> {
        if self.protocol_version != 1 {
            return Err(Error::IpcInvalid);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ipc, json};
    #[test]
    fn time_diagnostics_and_activity_ack_have_only_the_closed_nonsecret_fields() {
        for state in [
            TrustedTimeState::Unsupported,
            TrustedTimeState::SetupRequired,
            TrustedTimeState::MissingPolicy,
            TrustedTimeState::Unknown,
            TrustedTimeState::SourceUnavailable,
            TrustedTimeState::CleanupPending,
            TrustedTimeState::CleanupComplete,
            TrustedTimeState::IntegrityFault,
        ] {
            let status = TrustedTimeStatus::new(state, true);
            let raw = json::encode(&status, true).unwrap();
            let value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 4);
            assert!(
                value.get("protocolVersion").is_some()
                    && value.get("state").is_some()
                    && value.get("refreshing").is_some()
                    && value.get("refreshFailure").is_some()
            );
            let parsed: TrustedTimeStatus = json::decode(&raw, true).unwrap();
            parsed.validate().unwrap();
            assert_eq!(parsed, status);
        }
        let ack = TrustedTimeRefreshReply::accepted();
        let value: serde_json::Value =
            serde_json::from_slice(&json::encode(&ack, true).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"protocolVersion":1,"state":"accepted"})
        );
        for raw in [
            br#"{"protocolVersion":1,"state":"known","refreshing":false}"#.as_slice(),
            br#"{"protocolVersion":1,"state":"unknown","refreshing":false,"utc":1}"#,
            br#"{"protocolVersion":1,"state":"unknown","refreshing":false,"refreshing":true}"#,
        ] {
            assert!(json::decode::<TrustedTimeStatus>(raw, true).is_err());
        }
        let invalid = TrustedTimeStatus {
            protocol_version: 2,
            state: TrustedTimeState::Unknown,
            refreshing: false,
            refresh_failure: None,
        };
        assert!(invalid.validate().is_err());
        assert!(
            json::decode::<TrustedTimeRefreshReply>(
                br#"{"protocolVersion":1,"state":"completed"}"#,
                true
            )
            .is_err()
        );
    }
    #[test]
    fn explicit_refresh_replay_is_the_original_ack_and_never_a_second_dispatch() {
        let session = "1".repeat(64);
        let mut sequence = ipc::Sequencer::new(session.clone()).unwrap();
        let request = ipc::Request::parse(
            &json::encode(
                &serde_json::json!({"hostIpcVersion":1,
            "sessionId":session,"sequence":1,"kind":"request-time-refresh","body":{}}),
                true,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            sequence.check(&request).unwrap(),
            ipc::Dispatch::New
        ));
        let reply = ipc::success(
            &session,
            1,
            "request-time-refresh",
            TrustedTimeRefreshReply::accepted(),
        )
        .unwrap();
        sequence.completed(&request, reply.clone(), false).unwrap();
        match sequence.check(&request).unwrap() {
            ipc::Dispatch::Replay(actual) => assert_eq!(actual, reply.as_slice()),
            _ => panic!("duplicate dispatched"),
        }
        assert!(json::decode::<ipc::Empty>(br#"{"nonce":"supplied"}"#, true).is_err());
        assert!(json::decode::<ipc::Empty>(br#"{"utc":1}"#, true).is_err());
    }
}
