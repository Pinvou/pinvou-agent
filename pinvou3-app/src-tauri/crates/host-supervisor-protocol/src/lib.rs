//! PinvouOS Host Supervisor 的有界 wire contract。
//!
//! 协议刻意没有 PID、unit、cgroup path、命令或 property 字段。调用方只能选择编译期
//! 封闭的工作与动作；Linux daemon 再把它们映射到自己内嵌的 descriptor。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 2;
pub const PINVOU_APP_DESCRIPTOR_ID: &str = "pinvou-app";
pub const PINVOU_APP_DESCRIPTOR_REVISION: &str = "pinvou-app-descriptor-v1";
pub const PINVOU_ASR_DESCRIPTOR_ID: &str = "pinvou-asr";
pub const PINVOU_ASR_DESCRIPTOR_REVISION: &str = "pinvou-asr-descriptor-v1";
pub const MAX_REQUEST_BYTES: usize = 4 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const MAX_DETAIL_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedHostWork {
    PinvouApp,
    PinvouAsr,
}

impl ManagedHostWork {
    pub const fn descriptor_id(self) -> &'static str {
        match self {
            Self::PinvouApp => PINVOU_APP_DESCRIPTOR_ID,
            Self::PinvouAsr => PINVOU_ASR_DESCRIPTOR_ID,
        }
    }

    pub const fn descriptor_revision(self) -> &'static str {
        match self {
            Self::PinvouApp => PINVOU_APP_DESCRIPTOR_REVISION,
            Self::PinvouAsr => PINVOU_ASR_DESCRIPTOR_REVISION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorAction {
    Status,
    Launch,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorRequest {
    pub protocol_version: u16,
    pub request_id: String,
    pub target: ManagedHostWork,
    pub descriptor_revision: String,
    pub expected_instance_generation: Option<String>,
    pub action: SupervisorAction,
}

impl SupervisorRequest {
    pub fn status(request_id: impl Into<String>, target: ManagedHostWork) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            target,
            descriptor_revision: target.descriptor_revision().to_string(),
            expected_instance_generation: None,
            action: SupervisorAction::Status,
        }
    }

    pub fn launch_pinvou_app(request_id: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            target: ManagedHostWork::PinvouApp,
            descriptor_revision: ManagedHostWork::PinvouApp.descriptor_revision().to_string(),
            expected_instance_generation: None,
            action: SupervisorAction::Launch,
        }
    }

    pub fn stop_pinvou_asr(
        request_id: impl Into<String>,
        expected_instance_generation: impl Into<String>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            target: ManagedHostWork::PinvouAsr,
            descriptor_revision: ManagedHostWork::PinvouAsr.descriptor_revision().to_string(),
            expected_instance_generation: Some(expected_instance_generation.into()),
            action: SupervisorAction::Stop,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        if self.descriptor_revision != self.target.descriptor_revision() {
            return Err(ProtocolError::DescriptorRevisionMismatch);
        }
        validate_request_id(&self.request_id)?;
        match (self.target, self.action) {
            (ManagedHostWork::PinvouApp, SupervisorAction::Status)
            | (ManagedHostWork::PinvouApp, SupervisorAction::Launch)
            | (ManagedHostWork::PinvouAsr, SupervisorAction::Status) => {
                if self.expected_instance_generation.is_some() {
                    return Err(ProtocolError::UnexpectedInstanceGeneration);
                }
            }
            (ManagedHostWork::PinvouAsr, SupervisorAction::Stop) => {
                validate_instance_generation(
                    self.expected_instance_generation
                        .as_deref()
                        .ok_or(ProtocolError::MissingInstanceGeneration)?,
                )?;
            }
            _ => return Err(ProtocolError::ActionNotAllowed),
        }
        Ok(())
    }
}

pub fn validate_request_id(request_id: &str) -> Result<(), ProtocolError> {
    if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
        return Err(ProtocolError::InvalidRequestId);
    }
    if !request_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ProtocolError::InvalidRequestId);
    }
    Ok(())
}

/// systemd InvocationID is a 128-bit identifier rendered as 32 lowercase hex bytes.
pub fn validate_instance_generation(generation: &str) -> Result<(), ProtocolError> {
    if generation.len() != 32
        || generation.bytes().all(|byte| byte == b'0')
        || !generation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ProtocolError::InvalidInstanceGeneration);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolError {
    UnsupportedVersion,
    DescriptorRevisionMismatch,
    InvalidRequestId,
    InvalidInstanceGeneration,
    MissingInstanceGeneration,
    UnexpectedInstanceGeneration,
    ActionNotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorOutcome {
    Applied,
    AlreadyApplied,
    Reconciled,
    OutcomeUnknown,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedWorkState {
    Active,
    Activating,
    Deactivating,
    Inactive,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PressureLine {
    pub avg10: Option<f64>,
    pub avg60: Option<f64>,
    pub avg300: Option<f64>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryPressure {
    pub some: Option<PressureLine>,
    pub full: Option<PressureLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CgroupObservation {
    pub memory_current_bytes: Option<u64>,
    pub memory_peak_bytes: Option<u64>,
    pub memory_events: BTreeMap<String, u64>,
    pub memory_pressure: Option<MemoryPressure>,
    pub pids_current: Option<u64>,
    pub memory_high_bytes: Option<u64>,
    pub memory_max_bytes: Option<u64>,
    pub memory_swap_max_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostWorkObservation {
    pub instance_generation: Option<String>,
    pub state: ObservedWorkState,
    pub sub_state: String,
    pub unit_result: String,
    pub main_pid: Option<u32>,
    pub restart_count: Option<u64>,
    pub cgroup: CgroupObservation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisorReceipt {
    pub protocol_version: u16,
    pub request_id: String,
    pub target: ManagedHostWork,
    pub descriptor_revision: String,
    pub expected_instance_generation: Option<String>,
    pub action: SupervisorAction,
    pub outcome: SupervisorOutcome,
    pub observation: Option<HostWorkObservation>,
    pub detail: String,
    pub observed_at_unix_ms: u64,
}

impl SupervisorReceipt {
    pub fn validate_for(&self, request: &SupervisorRequest) -> Result<(), &'static str> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err("supervisor response protocol version mismatch");
        }
        if self.request_id != request.request_id
            || self.target != request.target
            || self.descriptor_revision != request.descriptor_revision
            || self.expected_instance_generation != request.expected_instance_generation
            || self.action != request.action
        {
            return Err("supervisor response does not match request");
        }
        if self.detail.len() > MAX_DETAIL_BYTES {
            return Err("supervisor response detail exceeds bound");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_rejects_untrusted_control_fields() {
        let raw = r#"{
          "protocol_version":1,
          "request_id":"directive:1",
          "target":"pinvou_app",
          "descriptor_revision":"pinvou-app-descriptor-v1",
          "expected_instance_generation":"0123456789abcdef0123456789abcdef",
          "action":"stop",
          "pid":123,
          "unit":"ssh.service",
          "command":"systemctl stop ssh"
        }"#;
        assert!(serde_json::from_str::<SupervisorRequest>(raw).is_err());
    }

    #[test]
    fn request_id_is_ascii_bounded_and_path_free() {
        assert!(validate_request_id("governor:directive-1").is_ok());
        assert!(validate_request_id("").is_err());
        assert!(validate_request_id("../other-unit").is_err());
        assert!(validate_request_id("contains space").is_err());
        assert!(validate_request_id(&"x".repeat(MAX_REQUEST_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn descriptor_revision_and_instance_generation_are_separate() {
        let mut request = SupervisorRequest::stop_pinvou_asr(
            "governor:directive-2",
            "0123456789abcdef0123456789abcdef",
        );
        assert!(request.validate().is_ok());
        request.descriptor_revision = "pinvou-asr-descriptor-v0".to_string();
        assert_eq!(
            request.validate(),
            Err(ProtocolError::DescriptorRevisionMismatch)
        );
        assert_eq!(ManagedHostWork::PinvouAsr.descriptor_id(), "pinvou-asr");
        assert_eq!(
            ManagedHostWork::PinvouAsr.descriptor_revision(),
            "pinvou-asr-descriptor-v1"
        );
    }

    #[test]
    fn unknown_target_and_action_are_rejected_by_serde() {
        let unknown_target = r#"{"protocol_version":1,"request_id":"d:1","target":"linux_pid","descriptor_revision":"pinvou-app-descriptor-v1","expected_instance_generation":null,"action":"status"}"#;
        let unknown_action = r#"{"protocol_version":1,"request_id":"d:1","target":"pinvou_app","descriptor_revision":"pinvou-app-descriptor-v1","expected_instance_generation":null,"action":"kill"}"#;
        assert!(serde_json::from_str::<SupervisorRequest>(unknown_target).is_err());
        assert!(serde_json::from_str::<SupervisorRequest>(unknown_action).is_err());
    }

    #[test]
    fn action_matrix_is_closed_and_stop_requires_exact_invocation_id() {
        let mut app_stop = SupervisorRequest::status("d:2", ManagedHostWork::PinvouApp);
        app_stop.action = SupervisorAction::Stop;
        app_stop.expected_instance_generation =
            Some("0123456789abcdef0123456789abcdef".to_string());
        assert_eq!(app_stop.validate(), Err(ProtocolError::ActionNotAllowed));

        let mut missing_generation = SupervisorRequest::status("d:3", ManagedHostWork::PinvouAsr);
        missing_generation.action = SupervisorAction::Stop;
        assert_eq!(
            missing_generation.validate(),
            Err(ProtocolError::MissingInstanceGeneration)
        );

        let invalid_generation = SupervisorRequest::stop_pinvou_asr("d:4", "ASR-v1");
        assert_eq!(
            invalid_generation.validate(),
            Err(ProtocolError::InvalidInstanceGeneration)
        );
        assert_eq!(
            SupervisorRequest::stop_pinvou_asr("d:5", "00000000000000000000000000000000")
                .validate(),
            Err(ProtocolError::InvalidInstanceGeneration)
        );
    }
}
