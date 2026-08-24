//! Secret/credential management: MCP tool secrets are neither written in
//! plaintext nor placed in `headers` (the foundation sends that field as a
//! literal); they live in the system credential store plus an in-process
//! registry, and `mcp.json` keeps only `${ENV}` placeholders.
//!
//! Placeholders are resolved on demand by the foundation's MCP secret resolver
//! hook (`install_mcp_secret_resolver`, registered at boot) when MCP
//! subprocess env is expanded / request headers are parsed — the process
//! environment is no longer written at runtime: under edition 2024 a runtime
//! `set_var` racing uncoordinated concurrent readers (the foundation's
//! `vars_os()` child-process env snapshots, WebKit/glib libc `getenv`) is a
//! data race, and the in-process registry is the only design that fully
//! closes that window (with zero writers, concurrent readers have no writer
//! to race against).
//!
//! Pure secret-related helpers and the secret read/write methods on
//! `MarketplaceManager` are collected here.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::platform::credential_store::{
    CredentialError, CredentialReference, CredentialStore, redact_secret,
};

use super::bundle;
use super::types::ToolManifest;

/// In-process MCP secret value registry: env var name (`PINVOU3_MCP_SECRET_*`)
/// → plaintext value. All safe Rust; the foundation's resolver callback reads
/// it through `resolve_registered_secret`.
static MCP_SECRET_VALUES: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn secret_values_read() -> RwLockReadGuard<'static, HashMap<String, String>> {
    MCP_SECRET_VALUES.read().unwrap_or_else(|p| p.into_inner())
}

fn secret_values_write() -> RwLockWriteGuard<'static, HashMap<String, String>> {
    MCP_SECRET_VALUES.write().unwrap_or_else(|p| p.into_inner())
}

/// Foundation resolver callback: look up the in-process registry by env var
/// name; return None on a miss (the foundation then falls back to the process
/// env, preserving the externally-manual-export compatibility path).
pub fn resolve_registered_secret(name: &str) -> Option<String> {
    secret_values_read().get(name).cloned()
}

/// Store a single secret value (install/resolve/migration paths).
pub(super) fn store_secret_value(env_name: String, value: String) {
    secret_values_write().insert(env_name, value);
}

/// Remove a single secret value (uninstall path).
pub(super) fn remove_secret_value(env_name: &str) {
    secret_values_write().remove(env_name);
}

#[cfg(test)]
pub(super) fn clear_secret_values_for_test() {
    secret_values_write().clear();
}

#[cfg(test)]
pub(super) fn snapshot_secret_values() -> HashMap<String, String> {
    secret_values_read().clone()
}

#[cfg(test)]
pub(super) fn restore_secret_values(snapshot: HashMap<String, String>) {
    *secret_values_write() = snapshot;
}

/// Whether a manifest field name looks like a secret (for compatibility with
/// legacy manifest.env fields suffixed `_API_KEY`).
pub(super) fn is_sensitive_key_name(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper == "API_KEY"
        || upper == "TOKEN"
        || upper == "SECRET"
        || upper == "KEY"
}

/// Placeholder env var name for a secret: the shared key between mcp.json
/// `${...}` placeholders and the in-process registry. It is no longer written
/// to the process env — the foundation reads it from the registry through the
/// resolver hook under this name.
pub(super) fn mcp_secret_env_var(secret_name: &str) -> String {
    let suffix = secret_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("PINVOU3_MCP_SECRET_{suffix}")
}

/// Placeholder form of a secret in mcp.json (the foundation expands `${...}`).
pub(super) fn mcp_secret_placeholder(secret_name: &str) -> String {
    format!("${{{}}}", mcp_secret_env_var(secret_name))
}

/// Remote MCP secrets must not go into `headers`: the foundation sends that
/// field as a literal and does not expand `${ENV}` placeholders. Bearer uses
/// the dedicated env-var-name config; custom headers without a scheme use
/// `env_headers`. This keeps secrets only in the in-process registry and the
/// credential store.
pub(super) fn set_remote_secret_header(
    env_headers: &mut serde_json::Map<String, serde_json::Value>,
    bearer_token_env_var: &mut Option<String>,
    header: &str,
    scheme: &str,
    key: &str,
) -> Result<(), String> {
    let env_var = mcp_secret_env_var(key);
    if header.eq_ignore_ascii_case("authorization") && scheme.eq_ignore_ascii_case("bearer") {
        if let Some(existing) = bearer_token_env_var.as_deref() {
            if existing != env_var {
                return Err("a remote MCP server does not support multiple Bearer secrets".to_string());
            }
        }
        *bearer_token_env_var = Some(env_var);
        return Ok(());
    }
    if scheme.trim().is_empty() {
        env_headers.insert(header.to_string(), serde_json::Value::String(env_var));
        return Ok(());
    }
    Err(format!(
        "remote MCP secret header '{header}' with scheme '{scheme}' is not supported yet; use Bearer Authorization or a custom header without a scheme"
    ))
}

pub(super) fn mcp_secret_reference(tool_id: &str, target: &str, key: &str) -> CredentialReference {
    CredentialReference::for_mcp_secret(tool_id, target, key)
}

pub(super) fn mcp_secret_missing_error(tool_id: &str, key: &str) -> String {
    format!("MCP tool '{tool_id}' is missing secret {key}; reconfigure it before enabling the tool")
}

pub(super) fn mcp_secret_store_error(tool_id: &str, key: &str, error: CredentialError) -> String {
    redact_secret(&format!(
        "MCP tool '{tool_id}' secret {key} is inaccessible: {}",
        error.user_message()
    ))
}

/// Extract every secret's (keyring target, key) from the manifest:
/// `secret_env`→("env",key), `secret_headers`→("header",source_key),
/// `config_fields`(secret=true)→(env or header, key). Each (target,key) pair
/// is deduplicated once. Targets stay aligned with `resolve_secret_placeholder`
/// at install time (a config_fields "bearer" lands as reference target
/// "header" there).
pub(super) fn manifest_secret_targets(manifest: &ToolManifest) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |target: &str, key: &str| {
        let pair = (target.to_string(), key.to_string());
        if !out.contains(&pair) {
            out.push(pair);
        }
    };
    for s in &manifest.secret_env {
        push("env", &s.key);
    }
    for s in &manifest.secret_headers {
        push(
            bundle::keyring_target(bundle::CredentialTarget::Bearer),
            &s.source_key,
        );
    }
    for f in &manifest.config_fields {
        if f.secret {
            let target = if f.target == "bearer" {
                bundle::keyring_target(bundle::CredentialTarget::Bearer)
            } else {
                f.target.as_str()
            };
            push(target, &f.key);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Secret read/write methods on MarketplaceManager
// ---------------------------------------------------------------------------

use super::MarketplaceManager;

impl<S: CredentialStore> MarketplaceManager<S> {
    /// After a restart, rehydrate the secrets of **all installed tools** from
    /// the keyring into the in-process registry (the foundation resolver reads
    /// them on demand when expanding `${...}` placeholders in MCP subprocess
    /// env). No longer hardcoded to the three built-ins — custom/uploaded
    /// tools with secrets work after restart too.
    pub(super) fn sync_secret_values(&self) -> Result<(), String> {
        // One-shot rebuild: hold the registry write lock throughout (inside
        // the lock there are only map writes and reads of already-ready data;
        // credential_store.get is a keyring/file short read with no long IO
        // or await).
        let mut values = secret_values_write();
        values.clear();
        for tool_id in self.installed_ids() {
            let Some(manifest) = self.load_manifest(&tool_id) else {
                continue;
            };
            for (target, key) in manifest_secret_targets(&manifest) {
                let reference = mcp_secret_reference(&tool_id, &target, &key);
                match self.credential_store.get(&reference) {
                    Ok(Some(value)) if !value.trim().is_empty() => {
                        values.insert(mcp_secret_env_var(&key), value);
                    }
                    Ok(_) => {}
                    Err(e) => return Err(mcp_secret_store_error(&tool_id, &key, e)),
                }
            }
        }
        Ok(())
    }

    /// Resolve a single secret's `${ENV}` placeholder: prefer the value the
    /// user entered this time (and persist it), otherwise the stored
    /// credential, otherwise fall back to the legacy manifest.env plaintext
    /// (for migration). If none exist → missing-secret error.
    pub(super) fn resolve_secret_placeholder(
        &self,
        tool_id: &str,
        target: &str,
        key: &str,
        user_config: &HashMap<String, String>,
        legacy_env: &HashMap<String, String>,
    ) -> Result<String, String> {
        let reference = mcp_secret_reference(tool_id, target, key);
        if let Some(value) = user_config.get(key).filter(|v| !v.trim().is_empty()) {
            self.credential_store
                .set(&reference, value)
                .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
            store_secret_value(mcp_secret_env_var(key), value.clone());
            return Ok(mcp_secret_placeholder(key));
        }

        match self.credential_store.get(&reference) {
            Ok(Some(value)) if !value.trim().is_empty() => {
                store_secret_value(mcp_secret_env_var(key), value);
                Ok(mcp_secret_placeholder(key))
            }
            Ok(_) => {
                if let Some(value) = legacy_env.get(key).filter(|v| !v.trim().is_empty()) {
                    self.credential_store
                        .set(&reference, value)
                        .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
                    store_secret_value(mcp_secret_env_var(key), value.clone());
                    Ok(mcp_secret_placeholder(key))
                } else {
                    Err(mcp_secret_missing_error(tool_id, key))
                }
            }
            Err(e) => Err(mcp_secret_store_error(tool_id, key, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_secret_targets_dedups_and_maps_bearer_to_header() {
        // The same key declared in secret_env/secret_headers and config_fields
        // → deduplicated once.
        let manifest: ToolManifest = serde_json::from_str(
            r#"{
            "id":"t","name":"T","description":"","version":"1","icon":"","category":"",
            "mcp_tools":[],"command":"","args":[],
            "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}],
            "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
            "config_fields":[
                {"key":"AMAP_KEY","label":"","required":false,"target":"env","secret":true},
                {"key":"QCC_API_KEY","label":"","required":false,"target":"bearer","secret":true}
            ]
        }"#,
        )
        .unwrap();
        let targets = manifest_secret_targets(&manifest);
        assert_eq!(targets.len(), 2, "AMAP/QCC each deduplicated once");
        assert!(targets.contains(&("env".to_string(), "AMAP_KEY".to_string())));
        assert!(targets.contains(&("header".to_string(), "QCC_API_KEY".to_string())));
    }

    #[test]
    fn remote_secret_header_config_uses_environment_backed_fields() {
        let mut env_headers = serde_json::Map::new();
        let mut bearer_token_env_var = None;

        set_remote_secret_header(
            &mut env_headers,
            &mut bearer_token_env_var,
            "Authorization",
            "Bearer",
            "PATSNAP_API_KEY",
        )
        .unwrap();
        set_remote_secret_header(
            &mut env_headers,
            &mut bearer_token_env_var,
            "X-Api-Key",
            "",
            "EXAMPLE_API_KEY",
        )
        .unwrap();

        assert_eq!(
            bearer_token_env_var.as_deref(),
            Some("PINVOU3_MCP_SECRET_PATSNAP_API_KEY")
        );
        assert_eq!(
            env_headers["X-Api-Key"],
            "PINVOU3_MCP_SECRET_EXAMPLE_API_KEY"
        );
    }
}
