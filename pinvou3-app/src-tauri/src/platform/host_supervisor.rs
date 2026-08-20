//! 受信组合根可注入的 Host Supervisor 窄客户端。
//!
//! 这里没有 Renderer/Tauri command。调用方只能传封闭 `ManagedHostWork` 与由 Governor
//! 生成的幂等 directive id；平台 adapter 再与同 UID Supervisor 通信。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pinvou_host_supervisor_protocol::SupervisorRequest;
#[cfg(test)]
pub use pinvou_host_supervisor_protocol::{CgroupObservation, HostWorkObservation};
pub use pinvou_host_supervisor_protocol::{
    ManagedHostWork, ObservedWorkState, SupervisorAction, SupervisorOutcome, SupervisorReceipt,
};

static STATUS_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSupervisorError {
    Unsupported,
    Unavailable(String),
    InvalidRequest(String),
    Protocol(String),
}

impl std::fmt::Display for HostSupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("当前平台不支持 Pinvou Host Supervisor"),
            Self::Unavailable(detail) => write!(formatter, "Host Supervisor 不可用: {detail}"),
            Self::InvalidRequest(detail) => write!(formatter, "Host Supervisor 请求无效: {detail}"),
            Self::Protocol(detail) => write!(formatter, "Host Supervisor 协议错误: {detail}"),
        }
    }
}

impl std::error::Error for HostSupervisorError {}

#[derive(Debug, Clone, Copy, Default)]
pub struct HostSupervisorClient;

impl HostSupervisorClient {
    pub const fn new() -> Self {
        Self
    }

    pub async fn status(
        &self,
        target: ManagedHostWork,
    ) -> Result<SupervisorReceipt, HostSupervisorError> {
        let sequence = STATUS_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("status:{}:{sequence}", std::process::id());
        self.dispatch(SupervisorRequest::status(request_id, target))
            .await
    }

    pub async fn stop(
        &self,
        target: ManagedHostWork,
        directive_id: &str,
        expected_instance_generation: &str,
    ) -> Result<SupervisorReceipt, HostSupervisorError> {
        if target != ManagedHostWork::PinvouAsr {
            return Err(HostSupervisorError::InvalidRequest(
                "only the fixed ASR descriptor supports Stop".to_string(),
            ));
        }
        self.dispatch(SupervisorRequest::stop_pinvou_asr(
            directive_id,
            expected_instance_generation,
        ))
        .await
    }

    async fn dispatch(
        &self,
        request: SupervisorRequest,
    ) -> Result<SupervisorReceipt, HostSupervisorError> {
        request
            .validate()
            .map_err(|error| HostSupervisorError::InvalidRequest(format!("{error:?}")))?;
        tokio::task::spawn_blocking(move || crate::platform::os::host_supervisor_request(&request))
            .await
            .map_err(|error| {
                HostSupervisorError::Unavailable(format!("client task failed: {error}"))
            })?
    }
}

/// desktop launcher 使用固定 app descriptor；不接受 unit、PID 或命令参数。
pub async fn launch_pinvou_app() -> Result<SupervisorReceipt, HostSupervisorError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let request_id = format!("desktop-launch:{}:{timestamp}", std::process::id());
    HostSupervisorClient::new()
        .dispatch(SupervisorRequest::launch_pinvou_app(request_id))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_target_is_opaque_and_fixed_generation() {
        assert_eq!(ManagedHostWork::PinvouApp.descriptor_id(), "pinvou-app");
        assert_eq!(
            ManagedHostWork::PinvouApp.descriptor_revision(),
            "pinvou-app-descriptor-v1"
        );
        assert_eq!(ManagedHostWork::PinvouAsr.descriptor_id(), "pinvou-asr");
        assert_eq!(
            ManagedHostWork::PinvouAsr.descriptor_revision(),
            "pinvou-asr-descriptor-v1"
        );
    }

    #[tokio::test]
    #[cfg(not(target_os = "linux"))]
    async fn unsupported_platform_is_explicit() {
        let result = HostSupervisorClient::new()
            .status(ManagedHostWork::PinvouApp)
            .await;
        assert_eq!(result, Err(HostSupervisorError::Unsupported));
    }
}
