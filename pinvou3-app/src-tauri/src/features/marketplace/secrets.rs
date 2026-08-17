//! 密钥/凭证管理:MCP 工具的 secret 既不写明文也不进 `headers`(底座当字面量发送),
//! 而是落进系统凭据库 + 进程环境变量,`mcp.json` 里只留 `${ENV}` 占位符。
//!
//! 这里集中放 secret 相关的纯函数助手 + `MarketplaceManager` 上读写 secret 的方法。

use std::collections::HashMap;

use crate::platform::credential_store::{
    redact_secret, CredentialError, CredentialReference, CredentialStore,
};

use super::bundle;
use super::types::ToolManifest;

/// 判断 manifest 字段名是否疑似密钥(用于兼容旧 manifest.env 中以 `_API_KEY` 结尾的字段)。
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

/// secret 在进程环境变量里的名字(进程级、子进程可继承)。
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

/// mcp.json 里 secret 的占位符形式(底座会展开 `${...}`)。
pub(super) fn mcp_secret_placeholder(secret_name: &str) -> String {
    format!("${{{}}}", mcp_secret_env_var(secret_name))
}

/// 远程 MCP 的密钥不能写进 `headers`:底座会把那个字段当作字面量发送,
/// 不会展开 `${ENV}` 占位符。Bearer 走专用的环境变量配置;无 scheme 的
/// 自定义 header 则使用 `env_headers`。这样密钥始终只在进程环境和凭据库中。
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
                return Err("同一个远程 MCP server 不支持多个 Bearer 密钥".to_string());
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
        "远程 MCP 密钥 header '{header}' 的 scheme '{scheme}' 暂不支持；请使用 Bearer Authorization 或无 scheme 的自定义 header"
    ))
}

pub(super) fn mcp_secret_reference(tool_id: &str, target: &str, key: &str) -> CredentialReference {
    CredentialReference::for_mcp_secret(tool_id, target, key)
}

pub(super) fn mcp_secret_missing_error(tool_id: &str, key: &str) -> String {
    format!("MCP 工具 '{tool_id}' 缺少密钥 {key}，请重新配置后再启用该工具")
}

pub(super) fn mcp_secret_store_error(tool_id: &str, key: &str, error: CredentialError) -> String {
    redact_secret(&format!(
        "MCP 工具 '{tool_id}' 的密钥 {key} 无法访问: {}",
        error.user_message()
    ))
}

/// 从 manifest 提取所有 secret 的 (keyring target, key):
/// `secret_env`→("env",key)、`secret_headers`→("header",source_key)、
/// `config_fields`(secret=true)→(env 或 header, key)。同一 (target,key) 去重一次。
/// 与 install 时 `resolve_secret_placeholder` 用的 target 对齐(config_fields 的
/// "bearer" 在 install 里落成 reference target "header")。
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
// MarketplaceManager 上的 secret 读写方法
// ---------------------------------------------------------------------------

use super::MarketplaceManager;

impl<S: CredentialStore> MarketplaceManager<S> {
    /// 重启后把**所有已安装工具**的 secret 从 keyring 重灌进进程 env(MCP 子进程 expand
    /// `${...}` 占位符用)。不再硬编码内置 3 个 —— 自定义/上传的带 secret 工具重启后同样生效。
    pub(super) fn sync_secret_env_vars(&self) -> Result<(), String> {
        for tool_id in self.installed_ids() {
            let Some(manifest) = self.load_manifest(&tool_id) else {
                continue;
            };
            for (target, key) in manifest_secret_targets(&manifest) {
                let reference = mcp_secret_reference(&tool_id, &target, &key);
                match self.credential_store.get(&reference) {
                    Ok(Some(value)) if !value.trim().is_empty() => {
                        std::env::set_var(mcp_secret_env_var(&key), value);
                    }
                    Ok(_) => {}
                    Err(e) => return Err(mcp_secret_store_error(&tool_id, &key, e)),
                }
            }
        }
        Ok(())
    }

    /// 解析单个 secret 的 `${ENV}` 占位符:优先用用户当次填入的值(并落库),
    /// 否则取已存的凭据,再否则回退到旧 manifest.env 明文(迁移用)。三者都无 → 报缺密钥错。
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
            std::env::set_var(mcp_secret_env_var(key), value);
            return Ok(mcp_secret_placeholder(key));
        }

        match self.credential_store.get(&reference) {
            Ok(Some(value)) if !value.trim().is_empty() => {
                std::env::set_var(mcp_secret_env_var(key), value);
                Ok(mcp_secret_placeholder(key))
            }
            Ok(_) => {
                if let Some(value) = legacy_env.get(key).filter(|v| !v.trim().is_empty()) {
                    self.credential_store
                        .set(&reference, value)
                        .map_err(|e| mcp_secret_store_error(tool_id, key, e))?;
                    std::env::set_var(mcp_secret_env_var(key), value);
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
        // 同一 key 在 secret_env/secret_headers 与 config_fields 重复声明 → 去重一次。
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
        assert_eq!(targets.len(), 2, "AMAP/QCC 各去重一次");
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
