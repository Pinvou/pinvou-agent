use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MODEL_API_KEY_SERVICE: &str = "pinvou3-model-api-key";
const MCP_SECRET_SERVICE: &str = "pinvou3-mcp-secret";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialReference {
    pub service: String,
    pub account: String,
    pub version: u32,
}

impl CredentialReference {
    pub fn for_model(model_id: &str) -> Self {
        Self {
            service: MODEL_API_KEY_SERVICE.to_string(),
            account: format!("model:{model_id}"),
            version: 1,
        }
    }

    pub fn for_mcp_secret(tool_id: &str, target: &str, secret_name: &str) -> Self {
        Self {
            service: MCP_SECRET_SERVICE.to_string(),
            account: format!("mcp:{tool_id}:{target}:{secret_name}"),
            version: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialState {
    Missing,
    Configured,
    EnvOverride,
    NeedsMigration,
    Unavailable,
}

impl Default for CredentialState {
    fn default() -> Self {
        Self::Missing
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialEditAction {
    KeepExisting,
    Replace,
    Delete,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialMigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub failed_model_ids: Vec<String>,
    pub settings_sanitized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError {
    message: String,
}

impl CredentialError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: redact_secret(&message.into()),
        }
    }

    pub fn user_message(&self) -> String {
        self.message.clone()
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CredentialError {}

pub trait CredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError>;
    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError>;
    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError>;
}

#[derive(Debug, Clone, Default)]
pub struct SystemCredentialStore;

impl SystemCredentialStore {
    pub fn new() -> Self {
        Self
    }
}

impl CredentialStore for SystemCredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        ))]
        {
            let entry = keyring::Entry::new(&reference.service, &reference.account)
                .map_err(|err| keyring_error("open", err))?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(keyring_error("read", err)),
            }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        )))]
        {
            let _ = reference;
            Err(CredentialError::new(
                "system credential store is unavailable on this platform; please reconfigure API Key",
            ))
        }
    }

    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        ))]
        {
            let entry = keyring::Entry::new(&reference.service, &reference.account)
                .map_err(|err| keyring_error("open", err))?;
            entry
                .set_password(value)
                .map_err(|err| keyring_error("write", err))
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        )))]
        {
            let _ = (reference, value);
            Err(CredentialError::new(
                "system credential store is unavailable on this platform; please reconfigure API Key",
            ))
        }
    }

    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        ))]
        {
            let entry = keyring::Entry::new(&reference.service, &reference.account)
                .map_err(|err| keyring_error("open", err))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(keyring_error("delete", err)),
            }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
        )))]
        {
            let _ = reference;
            Err(CredentialError::new(
                "system credential store is unavailable on this platform; please reconfigure API Key",
            ))
        }
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"), not(target_env = "musl"))
))]
fn keyring_error(action: &str, err: keyring::Error) -> CredentialError {
    CredentialError::new(format!(
        "credential store {action} failed; please reconfigure API Key or repair system credential access: {err}"
    ))
}

#[derive(Debug, Clone, Default)]
pub struct MemoryCredentialStore {
    values: Arc<Mutex<HashMap<(String, String), String>>>,
    fail: Arc<Mutex<Option<String>>>,
}

impl MemoryCredentialStore {
    pub fn fail_with(&self, message: impl Into<String>) {
        *self.fail.lock().expect("memory credential fail lock") = Some(message.into());
    }

    fn maybe_fail(&self) -> Result<(), CredentialError> {
        if let Some(message) = self.fail.lock().expect("memory credential fail lock").clone() {
            return Err(CredentialError::new(message));
        }
        Ok(())
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, reference: &CredentialReference) -> Result<Option<String>, CredentialError> {
        self.maybe_fail()?;
        Ok(self
            .values
            .lock()
            .expect("memory credential values lock")
            .get(&(reference.service.clone(), reference.account.clone()))
            .cloned())
    }

    fn set(&self, reference: &CredentialReference, value: &str) -> Result<(), CredentialError> {
        self.maybe_fail()?;
        self.values
            .lock()
            .expect("memory credential values lock")
            .insert(
                (reference.service.clone(), reference.account.clone()),
                value.to_string(),
            );
        Ok(())
    }

    fn delete(&self, reference: &CredentialReference) -> Result<(), CredentialError> {
        self.maybe_fail()?;
        self.values
            .lock()
            .expect("memory credential values lock")
            .remove(&(reference.service.clone(), reference.account.clone()));
        Ok(())
    }
}

pub fn redact_secret(input: &str) -> String {
    let bearer_redacted = redact_bearer_tokens(input);
    bearer_redacted
        .split_whitespace()
        .map(|part| {
            if is_secret_like(part) {
                "[REDACTED]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_bearer_tokens(input: &str) -> String {
    let mut output = Vec::new();
    let mut redact_next = false;
    for part in input.split_whitespace() {
        if redact_next {
            output.push("[REDACTED]");
            redact_next = false;
            continue;
        }
        output.push(part);
        if part.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';')
            .eq_ignore_ascii_case("bearer")
        {
            redact_next = true;
        }
    }
    output.join(" ")
}

pub fn is_secret_like(value: &str) -> bool {
    let trimmed = value.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
    if trimmed.len() < 8 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("ak-")
        || lower.starts_with("bce-v3/")
        || lower.starts_with("tvly-")
        || lower.starts_with("mgp")
        || (trimmed.len() >= 24
            && trimmed.chars().any(|c| c.is_ascii_digit())
            && trimmed.chars().any(|c| c.is_ascii_alphabetic()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip_and_delete() {
        let store = MemoryCredentialStore::default();
        let reference = CredentialReference::for_model("m1");
        assert_eq!(store.get(&reference).unwrap(), None);
        store.set(&reference, "sk-test-secret").unwrap();
        assert_eq!(store.get(&reference).unwrap().as_deref(), Some("sk-test-secret"));
        store.delete(&reference).unwrap();
        assert_eq!(store.get(&reference).unwrap(), None);
    }

    #[test]
    fn credential_error_redacts_secret_like_content() {
        let err = CredentialError::new("write failed for sk-test-secret-1234567890");
        assert!(!err.user_message().contains("sk-test-secret"));
        assert!(err.user_message().contains("[REDACTED]"));
    }

    #[test]
    fn mcp_reference_uses_separate_service() {
        let reference = CredentialReference::for_mcp_secret("iwencai", "env", "IWENCAI_API_KEY");
        assert_eq!(reference.service, "pinvou3-mcp-secret");
        assert_eq!(reference.account, "mcp:iwencai:env:IWENCAI_API_KEY");
        assert_eq!(reference.version, 1);
    }

    #[test]
    fn credential_error_redacts_mcp_bearer_tokens() {
        let err = CredentialError::new("request failed Authorization Bearer qcc-secret-token-1234567890");
        let message = err.user_message();
        assert!(!message.contains("qcc-secret-token"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn failing_memory_store_returns_redacted_errors() {
        let store = MemoryCredentialStore::default();
        store.fail_with("cannot read sk-secret-value-123456789");
        let err = store
            .get(&CredentialReference::for_model("m1"))
            .expect_err("store should fail");
        assert!(!err.user_message().contains("sk-secret-value"));
    }
}
