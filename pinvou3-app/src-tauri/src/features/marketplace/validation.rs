//! 远程 MCP 连接校验:把原始错误归类成用户可读的中文提示,以及拼装校验用的 McpConfig。

use deepseek_tui::mcp::{McpConfig, McpPool, McpServerConfig, McpTimeouts};

use crate::platform::credential_store::redact_secret;
use crate::platform::paths;

use super::MarketplaceManager;
use super::types::{MarketplaceToolValidation, ToolManifest};

/// 把远程 MCP 校验抛出的原始错误归类成用户可读的中文提示。
/// 先脱敏(redact_secret),再按 auth/network/限流/超时等关键词归类。
pub(super) fn remote_validation_user_error(raw: &str) -> String {
    let redacted = redact_secret(raw);
    let lower = redacted.to_ascii_lowercase();
    let auth_failed = [
        "401",
        "403",
        "unauthorized",
        "forbidden",
        "invalid token",
        "invalid api key",
        "invalid apikey",
        "api key invalid",
        "apikey invalid",
        "expired",
        "authentication failed",
        "auth failed",
        "permission denied",
        "access denied",
        "鉴权",
        "认证",
        "无效",
        "过期",
        "权限",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let network_error = [
        "dns",
        "tls",
        "certificate",
        "connection refused",
        "connection reset",
        "connect error",
        "proxy",
        "network",
        "failed to lookup address",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if auth_failed {
        "API Key 无效或已过期，请更新后重试".to_string()
    } else if lower.contains("429") || lower.contains("too many requests") {
        "远程 MCP 服务当前限流，请稍后重试".to_string()
    } else if lower.contains("timed out") || lower.contains("timeout") || lower.contains("超时") {
        "远程 MCP 服务响应超时，请稍后重试".to_string()
    } else if lower.contains("tools/list") || lower.contains("工具列表") {
        "远程 MCP 工具列表异常，请稍后重试".to_string()
    } else if network_error {
        "无法连接远程 MCP 服务，请检查网络或代理".to_string()
    } else {
        "远程 MCP 连接校验失败，请检查 API Key 或稍后重试".to_string()
    }
}

/// manifest 期望从远程拿到的工具名(去掉 `mcp_{server}_` / `mcp_{id}_` 前缀,与引擎对齐)。
pub(super) fn expected_remote_tool_names(manifest: &ToolManifest) -> Vec<String> {
    if !manifest.mcp_tools.is_empty() {
        return manifest
            .mcp_tools
            .iter()
            .map(|name| normalize_manifest_tool_name(name, manifest))
            .collect();
    }
    Vec::new()
}

fn normalize_manifest_tool_name(name: &str, manifest: &ToolManifest) -> String {
    for server in &manifest.servers {
        let prefix = format!("mcp_{}_", server.name);
        if let Some(rest) = name.strip_prefix(&prefix) {
            return rest.to_string();
        }
    }
    let id_prefix = format!("mcp_{}_", manifest.id);
    if let Some(rest) = name.strip_prefix(&id_prefix) {
        return rest.to_string();
    }
    name.to_string()
}

impl<S: crate::platform::credential_store::CredentialStore> MarketplaceManager<S> {
    /// manifest 显式要求时，把"配置已写入"收紧为"远程 MCP 已握手且工具可发现"。
    pub fn requires_remote_connection_validation(&self, tool_id: &str) -> bool {
        self.load_manifest(tool_id)
            .map(|m| m.validate_on_install && !m.servers.is_empty())
            .unwrap_or(false)
    }

    pub async fn validate_remote_connection(
        &self,
        tool_id: &str,
    ) -> Result<MarketplaceToolValidation, String> {
        let manifest = self
            .load_manifest(tool_id)
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;
        if !self.requires_remote_connection_validation(tool_id) {
            return Ok(MarketplaceToolValidation {
                tool_id: tool_id.to_string(),
                connected: true,
                tools: Vec::new(),
            });
        }

        let mut pool = McpPool::new(self.validation_mcp_config(&manifest)?);
        let errors = pool.connect_all().await;
        if !errors.is_empty() {
            let message = errors
                .into_iter()
                .map(|(server, err)| format!("{server}: {err:#}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(remote_validation_user_error(&message));
        }

        let tools = pool
            .all_tools()
            .into_iter()
            .map(|(_, tool)| tool.name.clone())
            .collect::<Vec<_>>();
        if tools.is_empty() {
            return Err("远程 MCP 工具列表异常，请稍后重试".to_string());
        }

        let expected = expected_remote_tool_names(&manifest);
        if !expected.is_empty() {
            let missing = expected
                .iter()
                .filter(|name| !tools.iter().any(|tool| tool == *name))
                .cloned()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(format!(
                    "远程 MCP 工具列表异常，缺少工具: {}",
                    missing.join(", ")
                ));
            }
        }

        Ok(MarketplaceToolValidation {
            tool_id: tool_id.to_string(),
            connected: true,
            tools,
        })
    }

    fn validation_mcp_config(&self, manifest: &ToolManifest) -> Result<McpConfig, String> {
        let mcp_path = paths::mcp_config_path();
        let content =
            std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 MCP 配置失败: {e}"))?;
        let mcp: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| format!("解析 MCP 配置失败: {e}"))?;
        let servers = mcp
            .get("servers")
            .and_then(|v| v.as_object())
            .ok_or_else(|| "MCP 配置缺少 servers".to_string())?;

        let mut config = McpConfig {
            timeouts: McpTimeouts {
                connect_timeout: 20,
                execute_timeout: 30,
                read_timeout: 30,
            },
            servers: std::collections::HashMap::new(),
        };
        for server in &manifest.servers {
            let entry = servers
                .get(&server.name)
                .ok_or_else(|| format!("MCP 配置缺少 server '{}'", server.name))?;
            let mut server_config: McpServerConfig = serde_json::from_value(entry.clone())
                .map_err(|e| format!("解析 MCP server '{}' 失败: {e}", server.name))?;
            server_config.required = true;
            server_config.connect_timeout = Some(20);
            server_config.execute_timeout = Some(30);
            server_config.read_timeout = Some(30);
            config.servers.insert(server.name.clone(), server_config);
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_validation_error_classifier_prefers_auth_message_for_token_failures() {
        for raw in [
            "401 Unauthorized",
            "403 Forbidden",
            "invalid apikey",
            "invalid api key",
            "invalid token",
            "api key invalid",
            "authentication failed",
            "auth failed",
            "permission denied",
            "access denied",
            "鉴权失败",
            "认证失败",
            "API Key 无效",
            "token expired",
        ] {
            assert_eq!(
                remote_validation_user_error(raw),
                "API Key 无效或已过期，请更新后重试",
                "raw={raw}"
            );
        }
    }

    #[test]
    fn remote_validation_error_classifier_separates_network_and_unknown_errors() {
        for raw in [
            "connection refused",
            "dns lookup failed",
            "proxy connect failed",
            "TLS certificate error",
            "failed to lookup address information",
        ] {
            assert_eq!(
                remote_validation_user_error(raw),
                "无法连接远程 MCP 服务，请检查网络或代理",
                "raw={raw}"
            );
        }

        assert_eq!(
            remote_validation_user_error("upstream rejected request"),
            "远程 MCP 连接校验失败，请检查 API Key 或稍后重试"
        );
        assert_eq!(
            remote_validation_user_error("unexpected json-rpc error wrong-token-20260715"),
            "远程 MCP 连接校验失败，请检查 API Key 或稍后重试"
        );
    }
}
