//! mcp.json 旧版明文密钥迁移:把早期写进 manifest/mcp.json 的明文 API Key 搬到
//! 系统凭据库,文件里只留 `${ENV}` 占位符。

use crate::platform::paths;

use super::MarketplaceManager;
use super::connectors::write_json_pretty;
use super::secrets::{
    mcp_secret_env_var, mcp_secret_placeholder, mcp_secret_reference, mcp_secret_store_error,
    store_secret_value,
};
use super::types::McpSecretMigrationResult;

/// 内置的"已知有明文密钥的工具"清单(迁移目标)。
#[derive(Debug, Clone, Copy)]
struct LegacyMcpSecretSpec {
    tool_id: &'static str,
    target: &'static str,
    key: &'static str,
}

fn legacy_mcp_secret_specs() -> &'static [LegacyMcpSecretSpec] {
    &[
        LegacyMcpSecretSpec {
            tool_id: "weather",
            target: "env",
            key: "AMAP_KEY",
        },
        LegacyMcpSecretSpec {
            tool_id: "iwencai",
            target: "env",
            key: "IWENCAI_API_KEY",
        },
        LegacyMcpSecretSpec {
            tool_id: "qcc",
            target: "header",
            key: "QCC_API_KEY",
        },
    ]
}

fn legacy_spec_for_tool(tool_id: &str) -> Option<&'static LegacyMcpSecretSpec> {
    legacy_mcp_secret_specs()
        .iter()
        .find(|spec| spec.tool_id == tool_id)
}

fn legacy_spec_for_server_name(server_name: &str) -> Option<&'static LegacyMcpSecretSpec> {
    if server_name == "weather" {
        legacy_spec_for_tool("weather")
    } else if server_name == "iwencai" {
        legacy_spec_for_tool("iwencai")
    } else if server_name.starts_with("qcc-") {
        legacy_spec_for_tool("qcc")
    } else {
        None
    }
}

impl<S: crate::platform::credential_store::CredentialStore> MarketplaceManager<S> {
    pub fn migrate_mcp_plaintext_secrets(&self) -> Result<McpSecretMigrationResult, String> {
        let mut result = McpSecretMigrationResult::default();
        for spec in legacy_mcp_secret_specs() {
            let path = self.servers_dir.join(spec.tool_id).join("manifest.json");
            if path.is_file() {
                self.migrate_manifest_file(&path, spec, &mut result)?;
            }
        }
        let mcp_path = paths::mcp_config_path();
        if mcp_path.is_file() {
            self.migrate_mcp_json_file(&mcp_path, &mut result)?;
        }
        Ok(result)
    }

    fn migrate_manifest_file(
        &self,
        path: &std::path::Path,
        spec: &LegacyMcpSecretSpec,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let Some(value) = json
            .get("env")
            .and_then(|env| env.get(spec.key))
            .and_then(|v| v.as_str())
            .filter(|v| !v.trim().is_empty())
            .map(ToOwned::to_owned)
        else {
            return Ok(());
        };

        self.store_migrated_secret(spec, &value, result)?;
        if let Some(env) = json.get_mut("env").and_then(|env| env.as_object_mut()) {
            env.remove(spec.key);
            if env.is_empty() {
                json.as_object_mut().map(|obj| obj.remove("env"));
            }
        }
        write_json_pretty(path, &json)?;
        Ok(())
    }

    fn migrate_mcp_json_file(
        &self,
        path: &std::path::Path,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("解析 {} 失败: {e}", path.display()))?;
        let mut changed = false;
        let Some(servers) = json
            .get_mut("servers")
            .and_then(|servers| servers.as_object_mut())
        else {
            return Ok(());
        };

        for (server_name, entry) in servers.iter_mut() {
            let Some(spec) = legacy_spec_for_server_name(server_name) else {
                continue;
            };
            if let Some(env) = entry.get_mut("env").and_then(|env| env.as_object_mut()) {
                if let Some(value) = env
                    .get(spec.key)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToOwned::to_owned)
                {
                    self.store_migrated_secret(spec, &value, result)?;
                    env.insert(
                        spec.key.to_string(),
                        serde_json::Value::String(mcp_secret_placeholder(spec.key)),
                    );
                    changed = true;
                }
            }
            if let Some(headers) = entry
                .get_mut("headers")
                .and_then(|headers| headers.as_object_mut())
            {
                if let Some(auth) = headers
                    .get("Authorization")
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.trim().is_empty())
                    .map(ToOwned::to_owned)
                {
                    if let Some(secret) = auth.strip_prefix("Bearer ").filter(|v| !v.is_empty()) {
                        self.store_migrated_secret(spec, secret, result)?;
                        // `headers` 是字面量,不会展开 `${ENV}`。迁移到
                        // 底座的 Bearer 环境变量字段,避免“已迁移但实际鉴权失败”。
                        headers.remove("Authorization");
                        if headers.is_empty() {
                            entry.as_object_mut().map(|object| object.remove("headers"));
                        }
                        entry["bearer_token_env_var"] =
                            serde_json::Value::String(mcp_secret_env_var(spec.key));
                        changed = true;
                    }
                }
            }
        }

        if changed {
            write_json_pretty(path, &json)?;
        }
        Ok(())
    }

    fn store_migrated_secret(
        &self,
        spec: &LegacyMcpSecretSpec,
        value: &str,
        result: &mut McpSecretMigrationResult,
    ) -> Result<(), String> {
        let reference = mcp_secret_reference(spec.tool_id, spec.target, spec.key);
        let env_value = match self.credential_store.get(&reference) {
            Ok(Some(existing)) if !existing.trim().is_empty() => {
                result.skipped_count += 1;
                result.messages.push(format!(
                    "MCP 工具 '{}' 的密钥 {} 已存在，已跳过覆盖并清理旧明文",
                    spec.tool_id, spec.key
                ));
                existing
            }
            Ok(_) => {
                self.credential_store.set(&reference, value).map_err(|e| {
                    result.failed_count += 1;
                    mcp_secret_store_error(spec.tool_id, spec.key, e)
                })?;
                result.migrated_count += 1;
                result.messages.push(format!(
                    "MCP 工具 '{}' 的密钥 {} 已迁移到系统凭据存储",
                    spec.tool_id, spec.key
                ));
                value.to_string()
            }
            Err(e) => {
                result.failed_count += 1;
                return Err(mcp_secret_store_error(spec.tool_id, spec.key, e));
            }
        };
        store_secret_value(mcp_secret_env_var(spec.key), env_value);
        Ok(())
    }
}
