use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::credential_store::{redact_secret, CredentialError, CredentialReference};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProvisioningStatus {
    NotStarted,
    QueryingUser,
    CreatingToken,
    ConfiguringPolicy,
    Ready,
    Failed,
    Disabled,
}

impl Default for ProvisioningStatus {
    fn default() -> Self {
        Self::NotStarted
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceBindingStatus {
    Unknown,
    Bound,
    NotBound,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmApiErrorCode {
    NotLoggedIn,
    UnsupportedPlatform,
    DeviceNotBound,
    DeviceBindingFailed,
    AdminCredentialMissing,
    ProvisioningFailed,
    ServiceUnreachable,
    RateLimited,
    ServiceDisabled,
    Unavailable,
    UserNotFound,
    PermissionDenied,
}

impl LlmApiErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotLoggedIn => "not_logged_in",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::DeviceNotBound => "device_not_bound",
            Self::DeviceBindingFailed => "device_binding_failed",
            Self::AdminCredentialMissing => "admin_credential_missing",
            Self::ProvisioningFailed => "provisioning_failed",
            Self::ServiceUnreachable => "service_unreachable",
            Self::RateLimited => "rate_limited",
            Self::ServiceDisabled => "service_disabled",
            Self::Unavailable => "unavailable",
            Self::UserNotFound => "user_not_found",
            Self::PermissionDenied => "permission_denied",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiError {
    pub code: LlmApiErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl LlmApiError {
    pub fn new(code: LlmApiErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: redact_secret(&message.into()),
            retryable,
        }
    }

    pub fn to_tauri_error(&self) -> String {
        format!("{}: {}", self.code.as_str(), self.message)
    }
}

impl std::fmt::Display for LlmApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_tauri_error())
    }
}

impl std::error::Error for LlmApiError {}

impl From<CredentialError> for LlmApiError {
    fn from(value: CredentialError) -> Self {
        Self::new(LlmApiErrorCode::Unavailable, value.user_message(), true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiIdentity {
    pub pinvou_user_id: String,
    pub device_binding_id: String,
    pub bios_sn_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiPolicy {
    pub quota_limit_tokens: u64,
    pub rpm_limit: u32,
    pub allowed_models: Vec<String>,
}

impl Default for LlmApiPolicy {
    fn default() -> Self {
        Self {
            quota_limit_tokens: 1_000_000,
            rpm_limit: 60,
            allowed_models: vec![crate::llmapi_hub::DEFAULT_MODEL.to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmUsageSnapshot {
    pub period: String,
    pub limit_tokens: u64,
    pub used_tokens: u64,
    pub remaining_tokens: u64,
    pub unmetered_call_count: u64,
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl LlmUsageSnapshot {
    pub fn new(period: String, limit_tokens: u64) -> Self {
        Self {
            period,
            limit_tokens,
            used_tokens: 0,
            remaining_tokens: limit_tokens,
            unmetered_call_count: 0,
            last_synced_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmCallRecord {
    pub pinvou_user_id: String,
    pub device_binding_id: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvisioningTask {
    pub pinvou_user_id: String,
    pub device_binding_id: String,
    pub status: ProvisioningStatus,
    pub last_error_code: Option<LlmApiErrorCode>,
    pub last_error_message: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiBinding {
    pub pinvou_user_id: String,
    pub device_binding_id: String,
    pub newapi_user_id: Option<String>,
    pub newapi_token_id: Option<String>,
    pub token_credential_ref: Option<CredentialReference>,
    pub policy: LlmApiPolicy,
    pub usage: LlmUsageSnapshot,
    pub enabled: bool,
    pub provisioning_status: ProvisioningStatus,
    pub last_error_code: Option<LlmApiErrorCode>,
    pub last_error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LlmApiBinding {
    pub fn new(identity: &LlmApiIdentity, policy: LlmApiPolicy) -> Self {
        let now = Utc::now();
        Self {
            pinvou_user_id: identity.pinvou_user_id.clone(),
            device_binding_id: identity.device_binding_id.clone(),
            newapi_user_id: None,
            newapi_token_id: None,
            token_credential_ref: None,
            usage: LlmUsageSnapshot::new(current_period(), policy.quota_limit_tokens),
            policy,
            enabled: true,
            provisioning_status: ProvisioningStatus::NotStarted,
            last_error_code: None,
            last_error_message: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn mark_status(&mut self, status: ProvisioningStatus) {
        self.provisioning_status = status;
        self.updated_at = Utc::now();
    }

    pub fn mark_error(&mut self, err: &LlmApiError) {
        self.provisioning_status = ProvisioningStatus::Failed;
        self.last_error_code = Some(err.code);
        self.last_error_message = Some(err.message.clone());
        self.updated_at = Utc::now();
    }

    pub fn clear_error(&mut self) {
        self.last_error_code = None;
        self.last_error_message = None;
        self.updated_at = Utc::now();
    }
}

pub fn current_period() -> String {
    Utc::now().format("%Y-%m").to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaStatus {
    pub period: String,
    pub limit_tokens: u64,
    pub used_tokens: u64,
    pub remaining_tokens: u64,
    pub unmetered_call_count: u64,
    pub last_synced_at: Option<DateTime<Utc>>,
}

impl From<&LlmUsageSnapshot> for QuotaStatus {
    fn from(value: &LlmUsageSnapshot) -> Self {
        Self {
            period: value.period.clone(),
            limit_tokens: value.limit_tokens,
            used_tokens: value.used_tokens,
            remaining_tokens: value.remaining_tokens,
            unmetered_call_count: value.unmetered_call_count,
            last_synced_at: value.last_synced_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiStatusResponse {
    pub pinvou_user_id: Option<String>,
    pub device_binding_status: DeviceBindingStatus,
    pub enabled: bool,
    pub provisioning_status: ProvisioningStatus,
    pub quota: Option<QuotaStatus>,
    pub last_call_status: Option<String>,
    pub last_error_code: Option<LlmApiErrorCode>,
    pub last_error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnsureLlmApiBindingResponse {
    pub status: ProvisioningStatus,
    pub created: bool,
    pub retryable: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuiltinLlmApiModelsResponse {
    pub available_models: Vec<String>,
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiAdminOverviewItem {
    pub pinvou_user_id: String,
    pub device_binding_status: DeviceBindingStatus,
    pub enabled: bool,
    pub provisioning_status: ProvisioningStatus,
    pub newapi_user_id: Option<String>,
    pub newapi_token_id: Option<String>,
    pub quota_used_tokens: u64,
    pub quota_limit_tokens: u64,
    pub last_error_code: Option<LlmApiErrorCode>,
    pub last_error_message: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmApiAdminOverviewResponse {
    pub items: Vec<LlmApiAdminOverviewItem>,
    pub total: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_serialization_omits_sensitive_plaintext() {
        let identity = LlmApiIdentity {
            pinvou_user_id: "u_1".to_string(),
            device_binding_id: "dev_abc".to_string(),
            bios_sn_hash: "secret-bios-hash".to_string(),
        };
        let mut binding = LlmApiBinding::new(&identity, LlmApiPolicy::default());
        binding.token_credential_ref = Some(CredentialReference::for_llmapi_token(
            &identity.pinvou_user_id,
            &identity.device_binding_id,
        ));

        let json = serde_json::to_string(&binding).unwrap();
        assert!(!json.contains("sk-test-token"));
        assert!(!json.contains("secret-bios-hash"));
        assert!(json.contains("pinvou3-llmapi-token"));
    }

    #[test]
    fn llmapi_error_redacts_secret_like_text() {
        let err = LlmApiError::new(
            LlmApiErrorCode::ProvisioningFailed,
            "failed with sk-secret-token-123456789",
            true,
        );
        assert!(!err.message.contains("sk-secret"));
        assert!(err.message.contains("[REDACTED]"));
    }
}
