//! 工具市场管理器 — 管理 MCP 工具的安装/卸载/状态查询。
//!
//! 每个工具是一个 MCP server，元数据定义在 `manifest.json`。
//! 安装状态持久化在 `~/.pinvou3/marketplace/installed.json`。
//! 安装/卸载时同步修改 `~/.pinvou3/bundle/mcp.json`。

pub mod actions;
pub mod bundle;
pub mod mcp_catalog;
pub mod plugin_import;
pub mod scope;
pub mod skill_marketplace;
pub mod skill_scope;
pub mod store;

use std::path::PathBuf;

use deepseek_tui::mcp::{McpConfig, McpPool, McpServerConfig, McpTimeouts};
use serde::{Deserialize, Serialize};

use crate::platform::credential_store::{
    redact_secret, CredentialError, CredentialReference, CredentialStore, SystemCredentialStore,
};
use crate::platform::paths;

/// 按会话类型 scope 持久化连接器禁用列表。
///
/// Refreshing live engines (组合目录重写 + 工具热刷) is an application
/// orchestration concern and is deliberately left to the caller, keeping
/// marketplace independent from the assistant runtime. 连接器禁用影响
/// companion skills 的可见性:组合目录计算时按 scope 排除被禁用连接器的
/// companion skills(`skill_materialization::disabled_skill_names_for`),
/// 因此落盘后需由调用方重写在线会话的组合目录。
pub async fn apply_disabled_connectors_for(
    scope: ConnectorScope,
    connector_ids: Vec<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        save_disabled_connectors_for(scope, &connector_ids);
    })
    .await
    .map_err(|error| format!("apply_disabled_connectors_for join: {error}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest — 每个 MCP 工具的元数据
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub icon: String,
    pub category: String,
    pub mcp_tools: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub secret_env: Vec<SecretEnv>,
    #[serde(default)]
    pub secret_headers: Vec<SecretHeader>,
    #[serde(default)]
    pub validate_on_install: bool,
    #[serde(default)]
    pub config_fields: Vec<ConfigField>,
    #[serde(default)]
    pub routing_rules: Vec<String>,
    #[serde(default)]
    pub tool_table_entries: Vec<String>,
    #[serde(default)]
    pub pip_dependencies: Vec<String>,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
    /// 配套技能 id:装该 MCP 时一并装、卸时一并删(让"一个能力"=引擎+引导整体装卸)。
    #[serde(default)]
    pub companion_skills: Vec<String>,
    /// spanner 扳手插件入口（相对 `bundles/<id>/spanner/`）。Some 时走 spanner_runner 包装
    /// （plugin-protocol §15），mcp.json 的 args 指向内置 spanner_runner + plugin.json。
    #[serde(default)]
    pub spanner_entry: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteServer {
    pub name: String,
    pub url: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth: Option<RemoteOAuthConfig>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_resource: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteOAuthConfig {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEnv {
    pub key: String,
    pub provider: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretHeader {
    pub header: String,
    #[serde(default = "default_bearer_scheme")]
    pub scheme: String,
    pub source_key: String,
    pub provider: String,
    #[serde(default = "default_required")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub label: String,
    pub required: bool,
    /// "env" = 写入 mcp.json env 字段, "bearer" = 写入 headers Authorization
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default)]
    pub secret: bool,
}

fn default_target() -> String {
    "env".to_string()
}

fn default_required() -> bool {
    true
}

fn default_bearer_scheme() -> String {
    "Bearer".to_string()
}

fn is_sensitive_key_name(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper == "API_KEY"
        || upper == "TOKEN"
        || upper == "SECRET"
        || upper == "KEY"
}

fn mcp_secret_env_var(secret_name: &str) -> String {
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

fn mcp_secret_placeholder(secret_name: &str) -> String {
    format!("${{{}}}", mcp_secret_env_var(secret_name))
}

/// 远程 MCP 的密钥不能写进 `headers`:底座会把那个字段当作字面量发送,
/// 不会展开 `${ENV}` 占位符。Bearer 走专用的环境变量配置;无 scheme 的
/// 自定义 header 则使用 `env_headers`。这样密钥始终只在进程环境和凭据库中。
fn set_remote_secret_header(
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

fn write_json_pretty(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 存量 mcp.json 条目的路径迁移：指向旧布局（`bundle/mcp-servers/<id>/`）的
/// command/args 重写为新包目录（`bundles/<id>/mcp/`）。幂等；只改本 app 写的
/// 文件（mcp.json）。按 mcp.json 的 server 键（= 工具 id）逐条重写，覆盖内嵌预设
/// 与自定义/手放 MCP（不再只遍历内嵌清单）。返回是否有改动（供启动标记观测）。
pub fn migrate_mcp_json_paths() -> Result<bool, String> {
    let mcp_path = paths::mcp_config_path();
    if !mcp_path.is_file() {
        return Ok(false);
    }
    let content =
        std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json 失败: {e}"))?;
    let mut mcp: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("解析 mcp.json 失败: {e}"))?;
    let mut changed = false;
    let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) else {
        return Ok(false);
    };
    let ids: Vec<String> = servers.keys().cloned().collect();
    for id in ids {
        let old_dir = paths::bundle_mcp_servers_dir().join(&id);
        let new_dir = mcp_catalog::package_mcp_dir(&id);
        // 兼容两种分隔符的历史写法（Windows 原生 `\` 与 `/`）
        let old_spellings = [
            old_dir.to_string_lossy().replace('\\', "/"),
            old_dir.to_string_lossy().replace('/', "\\"),
        ];
        let new_text = new_dir.to_string_lossy().to_string();
        let Some(entry) = servers.get_mut(&id) else {
            continue;
        };
        for key in ["command", "args"] {
            if let Some(value) = entry.get_mut(key) {
                if rewrite_path_strings(value, &old_spellings, &new_text) {
                    changed = true;
                }
            }
        }
    }
    if changed {
        write_json_pretty(&mcp_path, &mcp)?;
    }
    Ok(changed)
}

/// 强制迁移自定义 MCP（不在内嵌 `mcp_catalog` 里的）到新布局：把
/// `bundle/mcp-servers/<id>/` 整个搬到 `bundles/<id>/mcp/`。幂等（目标已存在则
/// 跳过）；内嵌预置由 `ensure_package_released` 从快照重释放，不在此搬。返回
/// (moved, kept)。
pub fn migrate_custom_mcp_layout() -> std::io::Result<(usize, usize)> {
    let old_root = paths::bundle_mcp_servers_dir();
    let Ok(rd) = std::fs::read_dir(&old_root) else {
        return Ok((0, 0));
    };
    let mut moved = 0;
    let mut kept = 0;
    for entry in rd.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue; // 跳过文件（如 present_artifact_server.py）
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if mcp_catalog::spec_for(&id).is_some() {
            continue; // 内嵌预置由快照重释放
        }
        let dst = mcp_catalog::package_mcp_dir(&id);
        if dst.is_dir() {
            kept += 1;
            continue; // 目标已存在，幂等跳过
        }
        if let Some(parent) = dst.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::rename(entry.path(), &dst) {
            Ok(_) => {
                log::info!("[marketplace] 迁移自定义 MCP {id} → {}", dst.display());
                moved += 1;
            }
            Err(e) => {
                log::warn!("[marketplace] 迁移自定义 MCP {id} 失败，保留旧位置: {e}");
                kept += 1;
            }
        }
    }
    Ok((moved, kept))
}

/// 递归重写字符串/字符串数组里的旧路径前缀。返回是否有改动。
fn rewrite_path_strings(value: &mut serde_json::Value, old: &[String], new: &str) -> bool {
    match value {
        serde_json::Value::String(s) => {
            let mut out = s.clone();
            for spelling in old {
                out = out.replace(spelling.as_str(), new);
            }
            if out != *s {
                *s = out;
                true
            } else {
                false
            }
        }
        serde_json::Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                if rewrite_path_strings(item, old, new) {
                    changed = true;
                }
            }
            changed
        }
        _ => false,
    }
}

fn mcp_secret_reference(tool_id: &str, target: &str, key: &str) -> CredentialReference {
    CredentialReference::for_mcp_secret(tool_id, target, key)
}

fn mcp_secret_missing_error(tool_id: &str, key: &str) -> String {
    format!("MCP 工具 '{tool_id}' 缺少密钥 {key}，请重新配置后再启用该工具")
}

fn mcp_secret_store_error(tool_id: &str, key: &str, error: CredentialError) -> String {
    redact_secret(&format!(
        "MCP 工具 '{tool_id}' 的密钥 {key} 无法访问: {}",
        error.user_message()
    ))
}

fn expected_remote_tool_names(manifest: &ToolManifest) -> Vec<String> {
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

fn remote_validation_user_error(raw: &str) -> String {
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

#[derive(Debug, Clone, Copy)]
struct LegacyMcpSecretSpec {
    tool_id: &'static str,
    target: &'static str,
    key: &'static str,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpSecretMigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub messages: Vec<String>,
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

// ---------------------------------------------------------------------------
// MarketplaceToolInfo — 前端展示用
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceToolInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub icon: String,
    pub category: String,
    pub installed: bool,
    /// 配套技能 id(来自 manifest `companion_skills`)。前端据此把「有配套 MCP 的技能卡」的
    /// 状态/装卸联动到本 MCP,单一真源在 manifest,避免命名不一致(gongwen↔government-writing)
    /// 时前端漏建映射导致两卡状态分叉。
    #[serde(default)]
    pub companion_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceToolValidation {
    pub tool_id: String,
    pub connected: bool,
    pub tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// 会话工具开关:按会话模式 scope 各自持久化的 "被禁用包"列表（scope 收敛后）。
// 用户在某类会话里关一次,该类型所有新对话/窗口都继承,直到手动开回 —— 见「工具
// 开关」方案,持久语义。落盘到 ~/.pinvou3/disabled_bundles.json（包 id × 模式）。
//
// 落盘格式:`{"scopes": {"<mode>": [...]}, "initialized": ["<mode>"],
// "project_skills_enabled"}`；持久化实现与旧双文件迁移见 `scope.rs`。
// ---------------------------------------------------------------------------

use crate::core::session_mode::SessionMode;

/// 连接器禁用集 scope：按会话模式键控的命名空间（键即模式 kebab-case 名）。
/// 历史上是独立的二元枚举；泛化后降为 `SessionMode` 的别名——serde 同为
/// kebab-case，落盘键与前端协议（`"plain"/"code"`）不变，下游引用零改动。
pub type ConnectorScope = SessionMode;

// 包 id × SessionMode 的单一禁用集已收敛到 `scope` 模块（`disabled_bundles.json`，
// 取代 `disabled_connectors.json` / `disabled_skills.json` 双文件）。这里 re-export
// 保留调用路径；连接器/技能/CLI 开关统一按包 id 落盘，`skill:` 前缀跨文件借道清除。
pub use crate::features::marketplace::scope::{
    load_disabled_bundles, load_disabled_bundles_for, load_hidden_bundles_for,
    remove_bundle_from_disabled_scopes, save_disabled_bundles, save_disabled_bundles_for,
    save_hidden_bundles_for, sync_deny_all_scopes_after_install, unavailable_bundles_for,
};
// 兼容旧名（原「连接器开关」调用方）：语义已收敛为包 id，旧名仅作别名过渡。
pub use crate::features::marketplace::scope::{
    load_disabled_bundles as load_disabled_connectors,
    load_disabled_bundles_for as load_disabled_connectors_for,
    remove_bundle_from_disabled_scopes as remove_connector_from_disabled_scopes,
    save_disabled_bundles as save_disabled_connectors,
    save_disabled_bundles_for as save_disabled_connectors_for,
};
/// 从 manifest 提取所有 secret 的 (keyring target, key):
/// `secret_env`→("env",key)、`secret_headers`→("header",source_key)、
/// `config_fields`(secret=true)→(env 或 header, key)。同一 (target,key) 去重一次。
/// 与 install 时 `resolve_secret_placeholder` 用的 target 对齐(config_fields 的
/// "bearer" 在 install 里落成 reference target "header")。
fn manifest_secret_targets(manifest: &ToolManifest) -> Vec<(String, String)> {
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

/// 重启后把**所有已安装工具**的 secret 从 keyring 重灌进进程 env(MCP 子进程 expand
/// `${...}` 占位符用)。不再硬编码内置 3 个 —— 自定义/上传的带 secret 工具重启后同样生效。
pub fn sync_mcp_secret_env_vars() -> Result<(), String> {
    MarketplaceManager::new().sync_secret_env_vars()
}

/// 当前(plain)被禁用连接器 → 模型可见工具全名(喂给引擎 disallowed_tools 的)。
pub fn disabled_tool_names() -> Vec<String> {
    disabled_tool_names_for(ConnectorScope::Plain)
}

/// 按会话类型 scope:被禁用连接器 → 模型可见工具全名(喂给引擎 disallowed_tools 的)。
/// 不可用集 = 开关关(disabled) + 不可见(hidden)。
pub fn disabled_tool_names_for(scope: ConnectorScope) -> Vec<String> {
    MarketplaceManager::new().model_tool_names(&unavailable_bundles_for(scope))
}

// ---------------------------------------------------------------------------
// MarketplaceManager
// ---------------------------------------------------------------------------

pub struct MarketplaceManager<S: CredentialStore = SystemCredentialStore> {
    /// bundle 解包后的 MCP servers 目录 (~/.pinvou3/bundle/mcp-servers/)
    servers_dir: PathBuf,
    /// 已安装工具列表文件 (~/.pinvou3/marketplace/installed.json)
    installed_file: PathBuf,
    credential_store: S,
}

impl Default for MarketplaceManager<SystemCredentialStore> {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketplaceManager<SystemCredentialStore> {
    pub fn new() -> Self {
        Self::with_store(SystemCredentialStore::new())
    }
}

impl<S: CredentialStore> MarketplaceManager<S> {
    pub fn with_store(credential_store: S) -> Self {
        let servers_dir = paths::bundle_mcp_servers_dir();
        let installed_file = paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json");
        Self {
            servers_dir,
            installed_file,
            credential_store,
        }
    }

    fn sync_secret_env_vars(&self) -> Result<(), String> {
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

    /// 全部可用 MCP 工具的 manifest。来源并集（后者覆盖前者同 id 条目）：
    /// 1. 内嵌目录（`mcp_catalog`，未安装包清单的唯一来源——刀十一起启动不再
    ///    全量释放到磁盘）；
    /// 2. 新布局 `bundles/<id>/mcp/`（已装包的磁盘副本优先，内容可能带迁移清理）。
    /// 旧布局 `bundle/mcp-servers/` 已退役，不再回退读取（强制迁移后删除）。
    pub fn available_tools(&self) -> Vec<ToolManifest> {
        let mut by_id: std::collections::HashMap<String, ToolManifest> =
            std::collections::HashMap::new();
        for spec in mcp_catalog::MCP_PACKAGES {
            match serde_json::from_str::<ToolManifest>(spec.manifest_json) {
                Ok(manifest) => {
                    by_id.insert(spec.id.to_string(), manifest);
                }
                Err(e) => eprintln!("[marketplace] 内嵌 manifest 解析失败（{}）: {e}", spec.id),
            }
        }
        if let Ok(rd) = std::fs::read_dir(paths::bundles_root()) {
            for entry in rd.flatten() {
                let manifest_path = entry.path().join("mcp").join("manifest.json");
                if !manifest_path.is_file() {
                    continue;
                }
                match std::fs::read_to_string(&manifest_path) {
                    Ok(content) => match serde_json::from_str::<ToolManifest>(&content) {
                        Ok(manifest) => {
                            by_id.insert(manifest.id.clone(), manifest);
                        }
                        Err(e) => eprintln!("[marketplace] parse error: {e}"),
                    },
                    Err(e) => eprintln!("[marketplace] read error: {e}"),
                }
            }
        }
        // HashMap 迭代顺序不确定 → 按 id 排序返回，保证前端列表/开关菜单每次刷新顺序稳定
        // （否则每次 toggle 触发刷新时工具行会随机换位）。
        let mut tools: Vec<ToolManifest> = by_id.into_values().collect();
        tools.sort_by(|a, b| a.id.cmp(&b.id));
        tools
    }

    /// 已安装的工具 ID 列表
    pub fn installed_ids(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(&self.installed_file) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        match serde_json::from_str::<Vec<String>>(&content) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!(
                    "[marketplace] installed.json is invalid: {e}; backing up and rebuilding from mcp.json"
                );
                self.backup_corrupt_installed(&content);
                let recovered = self.recover_installed_ids_from_mcp();
                if let Err(write_err) = self.save_installed(&recovered) {
                    eprintln!("[marketplace] failed to rewrite installed.json: {write_err}");
                }
                recovered
            }
        }
    }

    /// 前端列表：所有可用工具 + 安装状态
    pub fn list_tools(&self) -> Vec<MarketplaceToolInfo> {
        let installed = self.installed_ids();
        self.available_tools()
            .into_iter()
            .map(|m| MarketplaceToolInfo {
                installed: installed.contains(&m.id),
                id: m.id,
                name: m.name,
                description: m.description,
                version: m.version,
                icon: m.icon,
                category: m.category,
                companion_skills: m.companion_skills,
            })
            .collect()
    }

    /// 安装工具：写 installed.json + 更新 mcp.json
    /// `user_config` 是前端传入的用户配置（如 API Key），对应 config_fields
    pub fn install(
        &self,
        tool_id: &str,
        user_config: &std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        self.migrate_mcp_plaintext_secrets()?;
        let manifest = self
            .load_manifest(tool_id)
            .ok_or_else(|| format!("工具 '{tool_id}' 不存在"))?;

        // 先装 Python 依赖（跨平台 pip）；失败就不注册，让用户可重试。零依赖工具会直接跳过。
        self.pip_install_deps(&manifest)?;

        // 按需释放包资源到 bundles/<id>/mcp/（§4：安装时释放，非启动全量）。
        // 自定义工具（无内嵌 spec）已由强制迁移 `migrate_custom_mcp_layout` 搬到
        // 新布局；这里统一按新包目录写 mcp.json，不再回退旧布局。
        let fingerprint = mcp_catalog::release_package(tool_id)?;
        let server_dir = mcp_catalog::package_mcp_dir(tool_id);

        // 先写 mcp.json(含 resolve_secret_placeholder,缺密钥会失败);成功后才落
        // installed.json —— 避免「installed 已写、mcp 没注册」的半安装状态。
        self.add_to_mcp_json(&manifest, user_config, &server_dir)?;

        let mut installed = self.installed_ids();
        if !installed.contains(&tool_id.to_string()) {
            installed.push(tool_id.to_string());
        }
        self.save_installed(&installed)?;

        // 镜像写入统一真相源 bundles.json（Phase 2 过渡期：installed.json / mcp.json
        // 仍是权威，bundles.json 只镜像安装态；镜像写失败不翻盘主操作，fail loud 到日志）。
        let mut record = store::BundleRecord::installed_now(tool_id, store::BundleSource::Preset);
        record.credential_keys = bundle::tool_credentials(&manifest)
            .into_iter()
            .map(|c| c.key)
            .collect();
        record.content_fingerprint = fingerprint;
        if let Err(e) = store::BundleStore::new().upsert_preserving(record) {
            log::warn!("[marketplace] bundles.json 镜像写入失败（install {tool_id}）: {e}");
        }

        Ok(())
    }

    /// manifest 显式要求时，把“配置已写入”收紧为“远程 MCP 已握手且工具可发现”。
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

    /// 装 `manifest.pip_dependencies` 里的 Python 依赖（跨平台）。
    /// 用 `python -m pip install`（保证装进跑 MCP server 的同一个 python，不裸 `pip`）。
    /// ① 先预检依赖是否已可用（系统已装/此前装过）→ 命中即跳过，不跑 pip；
    /// ② 否则按序兜底：`--user` → `--user --break-system-packages`（PEP 668）→ `--break-system-packages`，任一成功即 Ok。
    /// 零依赖工具（pip_dependencies 为空）直接返回 Ok，不影响 weather/obsidian 等。
    fn pip_install_deps(&self, manifest: &ToolManifest) -> Result<(), String> {
        if manifest.pip_dependencies.is_empty() {
            return Ok(());
        }
        // Windows:python-pptx 等依赖已随内置 python(python-win)预装,不在用户机器
        // 跑 pip —— 用户也就不需要自己装 python。仅 Linux/macOS 走系统 python3 联网 pip。
        if crate::platform::capabilities::is_windows() {
            return Ok(());
        }
        let python_cmd = "python3";
        let deps = &manifest.pip_dependencies;

        // ① 预检:依赖已可用就直接 Ok,不跑 pip。用 importlib.metadata 按 PyPI 包名查
        //    (python-pptx 等),全部命中即满足。修「明明已装(系统包/此前装过)却仍判失败」。
        let satisfied = std::process::Command::new(python_cmd)
            .arg("-c")
            .arg("import importlib.metadata as m, sys; [m.version(p) for p in sys.argv[1:]]")
            .args(deps)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if satisfied {
            return Ok(());
        }

        // ② pip 安装,按序兜底,任一成功即 Ok:
        //    --user(常规)→ --user --break-system-packages(PEP 668:现代 Debian/Ubuntu 拦 --user,
        //    装进 ~/.local 用户目录、不动系统/发行版包)→ --break-system-packages(某些环境 --user 不可用)。
        let run = |extra: &[&str]| -> std::io::Result<std::process::Output> {
            let mut cmd = std::process::Command::new(python_cmd);
            cmd.args([
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-input",
            ]);
            cmd.args(extra);
            cmd.args(deps);
            cmd.output()
        };
        let attempts: [&[&str]; 3] = [
            &["--user"],
            &["--user", "--break-system-packages"],
            &["--break-system-packages"],
        ];
        let mut last_err = String::new();
        for extra in attempts {
            match run(extra) {
                Ok(o) if o.status.success() => return Ok(()),
                Ok(o) => {
                    last_err = String::from_utf8_lossy(&o.stderr)
                        .trim()
                        .lines()
                        .last()
                        .unwrap_or("")
                        .to_string();
                }
                Err(e) => {
                    return Err(format!(
                        "无法运行 {python_cmd}（请确认已安装 Python 且在 PATH 中）：{e}"
                    ));
                }
            }
        }
        Err(format!(
            "依赖安装失败（pip）：{last_err}（已尝试 --user 与 --break-system-packages;请确认网络可达且 python3 自带 pip）"
        ))
    }

    fn resolve_secret_placeholder(
        &self,
        tool_id: &str,
        target: &str,
        key: &str,
        user_config: &std::collections::HashMap<String, String>,
        legacy_env: &std::collections::HashMap<String, String>,
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
        std::env::set_var(mcp_secret_env_var(spec.key), env_value);
        Ok(())
    }

    /// 卸载工具：从 installed.json + mcp.json 中移除
    pub fn uninstall(&self, tool_id: &str) -> Result<(), String> {
        // 删该工具在 keyring 的 secret(防孤儿;此时 manifest 未删、仍可读声明)。
        // 删不掉不阻断卸载；若用户重新安装并重新填 key，会写入新的系统凭据。
        if let Some(manifest) = self.load_manifest(tool_id) {
            for (target, key) in manifest_secret_targets(&manifest) {
                let reference = mcp_secret_reference(tool_id, &target, &key);
                let _ = self.credential_store.delete(&reference);
                std::env::remove_var(mcp_secret_env_var(&key));
            }
        }
        // 更新 installed.json
        let mut installed = self.installed_ids();
        installed.retain(|id| id != tool_id);
        self.save_installed(&installed)?;

        // 更新 mcp.json
        self.remove_from_mcp_json(tool_id)?;

        // 镜像删除（与 install 对称：失败只记日志。命令层 OAuth token 前置删除的
        // 中止语义在 uninstall_marketplace_tool_sync，先于本函数，不受影响）。
        if let Err(e) = store::BundleStore::new().remove(tool_id) {
            log::warn!("[marketplace] bundles.json 镜像删除失败（uninstall {tool_id}）: {e}");
        }

        // 包目录资源清理（§4：卸载 = 删登记 + 删目录）。companion 技能由命令层
        // 联动删除（其 uninstall 会清理腾空的 skills/ 与包目录）；manifest 卸载后
        // 仍可读——load_manifest 回退内嵌目录。
        let pkg_dir = paths::bundles_root().join(tool_id);
        if pkg_dir.exists() {
            let _ = std::fs::remove_dir_all(pkg_dir.join("mcp"));
            let _ = std::fs::remove_dir(pkg_dir.join("skills")); // 仅空目录能删掉
            let _ = std::fs::remove_dir(&pkg_dir);
        }

        Ok(())
    }

    /// manifest 声明的配套技能 id(装该 MCP 时一并装、卸时一并删)。
    /// uninstall 不删 manifest 文件,故卸载后仍可读到。
    /// CLI 连接器（feishu/wecom/dingtalk/tmeet）无 manifest，配套技能目录查
    /// 能力包注册表的内置常量表（`bundle::cli_bundle_skill_dirs`）——companion
    /// 联动排除（`skill_materialization::disabled_skill_names_for`）由此覆盖 CLI 包。
    pub fn companion_skills(&self, tool_id: &str) -> Vec<String> {
        let cli_dirs = bundle::cli_bundle_skill_dirs(tool_id);
        if !cli_dirs.is_empty() {
            return cli_dirs.iter().map(|s| (*s).to_string()).collect();
        }
        self.load_manifest(tool_id)
            .map(|m| m.companion_skills)
            .unwrap_or_default()
    }

    pub fn oauth_remote_server_name(&self, tool_id: &str) -> Option<String> {
        self.load_manifest(tool_id)?
            .servers
            .into_iter()
            .find(|server| {
                !server.scopes.is_empty()
                    || server.oauth.is_some()
                    || server
                        .oauth_resource
                        .as_deref()
                        .is_some_and(|s| !s.trim().is_empty())
            })
            .map(|server| server.name)
    }

    /// 根据已安装工具生成 instructions 路由规则段 + 工具表条目
    pub fn build_instructions_fragment(&self) -> String {
        let installed = self.installed_ids();
        if installed.is_empty() {
            return String::new();
        }

        let mut tool_table_lines = Vec::new();
        let mut routing_lines = Vec::new();

        for tool_id in &installed {
            if let Some(manifest) = self.load_manifest(tool_id) {
                for entry in &manifest.tool_table_entries {
                    tool_table_lines.push(entry.clone());
                }
                for rule in &manifest.routing_rules {
                    routing_lines.push(format!("- {rule}"));
                }
            }
        }

        let mut fragment = String::new();

        // 工具表条目
        if !tool_table_lines.is_empty() {
            for line in &tool_table_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        // 路由规则
        if !routing_lines.is_empty() {
            fragment.push_str(
                "\n### 工具路由(优先用专用工具,不要用 `Web(action=\"search\")` 替代)\n\n",
            );
            for line in &routing_lines {
                fragment.push_str(line);
                fragment.push('\n');
            }
        }

        fragment
    }

    // --- internal ---

    /// manifest 读取链：新布局（bundles/<id>/mcp/，已装包磁盘副本）→ 内嵌目录
    /// （未安装包；卸载后 manifest 仍可读——companion 联动卸载与 secret 清理
    /// 依赖这一点）。旧布局 bundle/mcp-servers/ 已退役，不再回退读取。
    fn load_manifest(&self, tool_id: &str) -> Option<ToolManifest> {
        let new_path = mcp_catalog::package_mcp_dir(tool_id).join("manifest.json");
        if let Ok(content) = std::fs::read_to_string(&new_path) {
            if let Ok(manifest) = serde_json::from_str(&content) {
                return Some(manifest);
            }
        }
        let spec = mcp_catalog::spec_for(tool_id)?;
        serde_json::from_str(spec.manifest_json).ok()
    }

    /// pinvou3 工具开关:把"连接器 id 列表"映射成"模型可见工具全名"
    /// (`mcp_{server}_{tool}`,小写 —— 引擎 `command_denies_tool` 按小写精确匹配)。
    /// 关一个连接器要把它名下所有工具都列出来。
    pub fn model_tool_names(&self, connector_ids: &[String]) -> Vec<String> {
        let mut names = Vec::new();
        for cid in connector_ids {
            if let Some(m) = self.load_manifest(cid) {
                for server in &m.servers {
                    names.push(format!("mcp_{}_*", server.name).to_ascii_lowercase());
                }
                for t in &m.mcp_tools {
                    // manifest 的 mcp_tools 不统一:部分已是全名(mcp_xxx_yyy),部分是裸工具名。
                    // 已带 `mcp_` 前缀的原样用,否则补 `mcp_{id}_` —— 与引擎 mcp_{server}_{tool} 对齐。
                    let name = if t.starts_with("mcp_") {
                        t.clone()
                    } else {
                        format!("mcp_{}_{}", m.id, t)
                    };
                    names.push(name.to_ascii_lowercase());
                }
            }
        }
        names
    }

    fn save_installed(&self, ids: &[String]) -> Result<(), String> {
        let dir = self.installed_file.parent().unwrap();
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(ids).map_err(|e| e.to_string())?;
        std::fs::write(&self.installed_file, json).map_err(|e| format!("写入失败: {e}"))
    }

    fn backup_corrupt_installed(&self, content: &str) {
        let Some(parent) = self.installed_file.parent() else {
            return;
        };
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = parent.join(format!("installed.json.corrupt.{ts}"));
        if let Err(e) = std::fs::write(&backup, content) {
            eprintln!(
                "[marketplace] failed to backup corrupt installed.json to {}: {e}",
                backup.display()
            );
        }
    }

    fn recover_installed_ids_from_mcp(&self) -> Vec<String> {
        let content = match std::fs::read_to_string(paths::mcp_config_path()) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let Ok(mcp) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Vec::new();
        };
        let Some(servers) = mcp.get("servers").and_then(|s| s.as_object()) else {
            return Vec::new();
        };
        let mut recovered = Vec::new();
        for manifest in self.available_tools() {
            let registered = if manifest.servers.is_empty() {
                servers.contains_key(&manifest.id)
            } else {
                manifest
                    .servers
                    .iter()
                    .any(|server| servers.contains_key(&server.name))
            };
            if registered && !recovered.contains(&manifest.id) {
                recovered.push(manifest.id);
            }
        }
        recovered
    }

    fn add_to_mcp_json(
        &self,
        manifest: &ToolManifest,
        user_config: &std::collections::HashMap<String, String>,
        server_dir: &std::path::Path,
    ) -> Result<(), String> {
        let mcp_path = paths::mcp_config_path();
        let mut mcp: serde_json::Value = if mcp_path.is_file() {
            let content =
                std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json: {e}"))?;
            serde_json::from_str(&content).unwrap_or_else(|_| default_mcp_json())
        } else {
            default_mcp_json()
        };

        let servers = mcp
            .get_mut("servers")
            .and_then(|s| s.as_object_mut())
            .ok_or("mcp.json 格式错误")?;

        if !manifest.servers.is_empty() {
            // ── 远程工具：遍历 manifest.servers[]，写 url/headers ──
            for server in &manifest.servers {
                let mut headers = serde_json::Map::new();
                let mut env_headers = serde_json::Map::new();
                let mut bearer_token_env_var = None;

                // 1. config_fields 中 target="bearer" 的字段（用户填入）
                for field in &manifest.config_fields {
                    if field.target == "bearer" {
                        if let Some(val) = user_config.get(&field.key) {
                            if field.secret {
                                self.resolve_secret_placeholder(
                                    &manifest.id,
                                    bundle::keyring_target(bundle::CredentialTarget::Bearer),
                                    &field.key,
                                    user_config,
                                    &manifest.env,
                                )?;
                                set_remote_secret_header(
                                    &mut env_headers,
                                    &mut bearer_token_env_var,
                                    "Authorization",
                                    "Bearer",
                                    &field.key,
                                )?;
                            } else {
                                headers.insert(
                                    "Authorization".to_string(),
                                    serde_json::Value::String(format!("Bearer {}", val)),
                                );
                            }
                        }
                    }
                }

                // 2. manifest.secret_headers 声明的敏感 header（不落明文）
                for secret in &manifest.secret_headers {
                    self.resolve_secret_placeholder(
                        &manifest.id,
                        bundle::keyring_target(bundle::CredentialTarget::Bearer),
                        &secret.source_key,
                        user_config,
                        &manifest.env,
                    )?;
                    set_remote_secret_header(
                        &mut env_headers,
                        &mut bearer_token_env_var,
                        &secret.header,
                        &secret.scheme,
                        &secret.source_key,
                    )?;
                }

                // 3. 兼容旧 manifest.env 中以 _API_KEY 结尾的字段，迁移后只写占位。
                if headers.is_empty() {
                    for k in manifest.env.keys() {
                        if is_sensitive_key_name(k) {
                            let placeholder = self.resolve_secret_placeholder(
                                &manifest.id,
                                bundle::keyring_target(bundle::CredentialTarget::Bearer),
                                k,
                                user_config,
                                &manifest.env,
                            )?;
                            headers.insert(
                                "Authorization".to_string(),
                                serde_json::Value::String(format!("Bearer {}", placeholder)),
                            );
                            break;
                        }
                    }
                }

                let mut entry = serde_json::json!({ "url": server.url });
                if !server.scopes.is_empty() {
                    entry["scopes"] = serde_json::to_value(&server.scopes).unwrap_or_default();
                }
                if let Some(oauth) = &server.oauth {
                    entry["oauth"] = serde_json::to_value(oauth).unwrap_or_default();
                }
                if let Some(resource) = &server.oauth_resource {
                    if !resource.trim().is_empty() {
                        entry["oauth_resource"] = serde_json::Value::String(resource.clone());
                    }
                }
                if !headers.is_empty() {
                    entry["headers"] = serde_json::Value::Object(headers);
                }
                if !env_headers.is_empty() {
                    entry["env_headers"] = serde_json::Value::Object(env_headers);
                }
                if let Some(env_var) = bearer_token_env_var {
                    entry["bearer_token_env_var"] = serde_json::Value::String(env_var);
                }
                servers.insert(server.name.clone(), entry);
            }
        } else {
            // ── 本地工具：command/args/env（脚本路径指向释放后的包目录）──
            let args: Vec<String> = if manifest.spanner_entry.is_some() {
                // spanner 扳手插件（plugin-protocol §15）：args = [spanner_runner, plugin.json]。
                vec![
                    paths::bundle_spanner_runner().to_string_lossy().to_string(),
                    crate::platform::paths::bundles_root()
                        .join(&manifest.id)
                        .join("plugin.json")
                        .to_string_lossy()
                        .to_string(),
                ]
            } else {
                manifest
                    .args
                    .iter()
                    .map(|a| {
                        if a == "server.py" || a.ends_with("/server.py") {
                            server_dir.join("server.py").to_string_lossy().to_string()
                        } else {
                            a.clone()
                        }
                    })
                    .collect()
            };

            let mut env = manifest
                .env
                .iter()
                .filter(|(k, _)| !is_sensitive_key_name(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<std::collections::HashMap<_, _>>();
            for field in &manifest.config_fields {
                if field.target == "env" {
                    if let Some(val) = user_config.get(&field.key) {
                        if field.secret || is_sensitive_key_name(&field.key) {
                            let placeholder = self.resolve_secret_placeholder(
                                &manifest.id,
                                "env",
                                &field.key,
                                user_config,
                                &manifest.env,
                            )?;
                            env.insert(field.key.clone(), placeholder);
                        } else {
                            env.insert(field.key.clone(), val.clone());
                        }
                    }
                }
            }
            for secret in &manifest.secret_env {
                let placeholder = self.resolve_secret_placeholder(
                    &manifest.id,
                    "env",
                    &secret.key,
                    user_config,
                    &manifest.env,
                )?;
                env.insert(secret.key.clone(), placeholder);
            }
            for key in manifest.env.keys().filter(|k| is_sensitive_key_name(k)) {
                if !env.contains_key(key) {
                    let placeholder = self.resolve_secret_placeholder(
                        &manifest.id,
                        "env",
                        key,
                        user_config,
                        &manifest.env,
                    )?;
                    env.insert(key.clone(), placeholder);
                }
            }

            // python 工具:Windows 用内置 pythonw(无窗口 + 自带依赖),其他平台系统 python3。
            let command = if manifest.command == "python" || manifest.command == "python3" {
                paths::python_command()
            } else {
                manifest.command.clone()
            };
            let mut entry = serde_json::json!({
                "command": command,
                "args": args,
            });
            if !env.is_empty() {
                entry["env"] = serde_json::to_value(&env).unwrap_or_default();
            }

            servers.insert(manifest.id.clone(), entry);
        }

        write_json_pretty(&mcp_path, &mcp)
    }

    fn remove_from_mcp_json(&self, tool_id: &str) -> Result<(), String> {
        let mcp_path = paths::mcp_config_path();
        if !mcp_path.is_file() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(&mcp_path).map_err(|e| format!("读取 mcp.json: {e}"))?;
        let mut mcp: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|_| default_mcp_json());

        if let Some(servers) = mcp.get_mut("servers").and_then(|s| s.as_object_mut()) {
            // 先尝试加载 manifest 看是否有 servers 字段（远程工具有多条目）
            if let Some(manifest) = self.load_manifest(tool_id) {
                if !manifest.servers.is_empty() {
                    for server in &manifest.servers {
                        servers.remove(&server.name);
                    }
                } else {
                    servers.remove(tool_id);
                }
            } else {
                servers.remove(tool_id);
            }
        }

        let json = serde_json::to_string_pretty(&mcp).map_err(|e| e.to_string())?;
        std::fs::write(&mcp_path, json).map_err(|e| format!("写入 mcp.json: {e}"))
    }
}

fn default_mcp_json() -> serde_json::Value {
    serde_json::json!({"servers": {}})
}

#[cfg(test)]
// 测试借 platform::paths::tests::ENV_LOCK(std Mutex)串行化全局 env;单线程测试内跨 await 持有无竞争者,不会死锁。
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::platform::credential_store::{CredentialStore, MemoryCredentialStore};
    use crate::platform::paths::tests::ENV_LOCK;
    use std::future::Future;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

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

    /// 把 PINVOU3_HOME 指到一个干净临时目录跑闭包,跑完恢复并清理。
    /// 借 paths 的 ENV_LOCK 跟其它 mutate PINVOU3_HOME 的测试串行,避免互相覆盖。
    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let prev_amap = std::env::var("PINVOU3_MCP_SECRET_AMAP_KEY").ok();
        let prev_iwencai = std::env::var("PINVOU3_MCP_SECRET_IWENCAI_API_KEY").ok();
        let prev_qcc = std::env::var("PINVOU3_MCP_SECRET_QCC_API_KEY").ok();
        let prev_patsnap = std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").ok();
        let dir = std::env::temp_dir().join(format!("pinvou3-mkt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PINVOU3_HOME", &dir);
        f();
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        for (key, value) in [
            ("PINVOU3_MCP_SECRET_AMAP_KEY", prev_amap),
            ("PINVOU3_MCP_SECRET_IWENCAI_API_KEY", prev_iwencai),
            ("PINVOU3_MCP_SECRET_QCC_API_KEY", prev_qcc),
            ("PINVOU3_MCP_SECRET_PATSNAP_API_KEY", prev_patsnap),
        ] {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn with_temp_home_async<F, Fut>(f: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let prev_amap = std::env::var("PINVOU3_MCP_SECRET_AMAP_KEY").ok();
        let prev_iwencai = std::env::var("PINVOU3_MCP_SECRET_IWENCAI_API_KEY").ok();
        let prev_qcc = std::env::var("PINVOU3_MCP_SECRET_QCC_API_KEY").ok();
        let prev_patsnap = std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").ok();
        let dir =
            std::env::temp_dir().join(format!("pinvou3-mkt-test-async-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("PINVOU3_HOME", &dir);
        f().await;
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
        for (key, value) in [
            ("PINVOU3_MCP_SECRET_AMAP_KEY", prev_amap),
            ("PINVOU3_MCP_SECRET_IWENCAI_API_KEY", prev_iwencai),
            ("PINVOU3_MCP_SECRET_QCC_API_KEY", prev_qcc),
            ("PINVOU3_MCP_SECRET_PATSNAP_API_KEY", prev_patsnap),
        ] {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct MockMcpServer {
        url: String,
        seen_methods: Arc<StdMutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl Drop for MockMcpServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn spawn_mock_mcp_server(valid_key: &'static str) -> MockMcpServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen_methods = Arc::new(StdMutex::new(Vec::new()));
        let seen_for_task = Arc::clone(&seen_methods);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let seen = Arc::clone(&seen_for_task);
                tokio::spawn(async move {
                    let _ = handle_mock_mcp_request(stream, valid_key, seen).await;
                });
            }
        });
        MockMcpServer {
            url: format!("http://{addr}/mcp"),
            seen_methods,
            task,
        }
    }

    async fn handle_mock_mcp_request(
        mut stream: tokio::net::TcpStream,
        valid_key: &str,
        seen_methods: Arc<StdMutex<Vec<String>>>,
    ) -> std::io::Result<()> {
        let mut buffer = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buffer.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_header_end(&buffer) {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]).to_string();
        let first = headers.lines().next().unwrap_or_default();
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let _path = parts.next().unwrap_or_default();

        if method == "GET" {
            return write_http_response(&mut stream, 404, "text/plain", "not found").await;
        }

        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buffer.len() < body_start + content_length {
            let mut chunk = [0u8; 1024];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        let body = &buffer[body_start..buffer.len().min(body_start + content_length)];
        let expected_authorization = format!("Bearer {valid_key}");
        let authorized = headers.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.trim().eq_ignore_ascii_case("authorization")
                    && value.trim() == expected_authorization
            })
        });
        if !authorized {
            return write_http_response(&mut stream, 401, "text/plain", "unauthorized").await;
        }

        let request: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let rpc_method = request
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        seen_methods.lock().unwrap().push(rpc_method.clone());

        if rpc_method == "notifications/initialized" {
            return write_http_response(&mut stream, 202, "application/json", "").await;
        }

        let id = request.get("id").cloned().unwrap_or(serde_json::json!(1));
        let result = match rpc_method.as_str() {
            "initialize" => serde_json::json!({
            "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-patsnap", "version": "1.0.0"}
            }),
            "tools/list" => serde_json::json!({
                "tools": [
                    {"name": "patsnap_search", "description": "search", "inputSchema": {"type": "object"}},
                    {"name": "patsnap_fetch", "description": "fetch", "inputSchema": {"type": "object"}}
                ]
            }),
            "resources/list" => serde_json::json!({"resources": []}),
            "resources/templates/list" => serde_json::json!({"resourceTemplates": []}),
            "prompts/list" => serde_json::json!({"prompts": []}),
            _ => serde_json::json!({}),
        };
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        });
        write_http_response(&mut stream, 200, "application/json", &response.to_string()).await
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|w| w == b"\r\n\r\n")
    }

    async fn write_http_response(
        stream: &mut tokio::net::TcpStream,
        status: u16,
        content_type: &str,
        body: &str,
    ) -> std::io::Result<()> {
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            401 => "Unauthorized",
            404 => "Not Found",
            _ => "Error",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await
    }

    fn write_tool_manifest(tool_id: &str, manifest: &str) {
        let dir = crate::features::marketplace::mcp_catalog::package_mcp_dir(tool_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    }

    /// 直接写 installed.json 模拟已安装连接器(避免走完整 install 的远程校验)。
    fn write_installed_ids(ids: &[String]) {
        let path = crate::platform::paths::pinvou3_home()
            .join("marketplace")
            .join("installed.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(ids).unwrap()).unwrap();
    }

    fn read_mcp_json() -> serde_json::Value {
        let content = std::fs::read_to_string(crate::platform::paths::mcp_config_path()).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    fn secret_value(name: &str) -> String {
        format!("test-secret-{name}-value-123456")
    }

    /// 连接器 → 模型可见工具全名:裸名补 `mcp_{id}_` 前缀,已带 `mcp_` 的原样(不双前缀),
    /// 一律小写;不存在的连接器跳过。这正是当初打印映射时抓到"双前缀 bug"的那段逻辑。
    #[test]
    fn installed_ids_recovers_corrupt_file_from_mcp_json() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["get_weather"],"command":"python","args":["server.py"]
                }"#,
            );
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                r#"{"servers":{"weather":{"command":"python3","args":["server.py"]}}}"#,
            )
            .unwrap();
            let installed_path = crate::platform::paths::pinvou3_home()
                .join("marketplace")
                .join("installed.json");
            std::fs::create_dir_all(installed_path.parent().unwrap()).unwrap();
            std::fs::write(&installed_path, "[\"weather\"").unwrap();

            let ids = MarketplaceManager::new().installed_ids();

            assert_eq!(ids, vec!["weather".to_string()]);
            let repaired = std::fs::read_to_string(&installed_path).unwrap();
            assert_eq!(
                serde_json::from_str::<Vec<String>>(&repaired).unwrap(),
                vec!["weather".to_string()]
            );
            let backups: Vec<_> = std::fs::read_dir(installed_path.parent().unwrap())
                .unwrap()
                .flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("installed.json.corrupt.")
                })
                .collect();
            assert_eq!(backups.len(), 1);
        });
    }

    #[test]
    fn model_tool_names_prefix_dedup_and_lowercase() {
        with_temp_home(|| {
            let dir = crate::features::marketplace::mcp_catalog::package_mcp_dir("demo");
            std::fs::create_dir_all(&dir).unwrap();
            let manifest = r#"{
                "id":"demo","name":"Demo","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["bare_tool","mcp_demo_already","UPPER_Tool"],
                "command":"python","args":[]
            }"#;
            std::fs::write(dir.join("manifest.json"), manifest).unwrap();

            let mgr = MarketplaceManager::new();
            let names = mgr.model_tool_names(&["demo".to_string()]);
            assert_eq!(
                names,
                vec![
                    "mcp_demo_bare_tool".to_string(),  // 裸名 → 补前缀
                    "mcp_demo_already".to_string(), // 已带 mcp_ → 原样,不变成 mcp_demo_mcp_demo_already
                    "mcp_demo_upper_tool".to_string(), // 小写化
                ]
            );
            // 没装/不存在的连接器 → 跳过(不报错,空)
            assert!(mgr.model_tool_names(&["nope".to_string()]).is_empty());
        });
    }

    /// 远程 server 连接器可能没有静态 mcp_tools 列表(qcc 即如此)。禁用时必须按
    /// server 名生成前缀规则,否则底座仍会暴露该连接器动态发现出来的全部工具。
    #[test]
    fn model_tool_names_generates_prefix_rules_for_remote_servers() {
        with_temp_home(|| {
            let dir = crate::platform::paths::bundle_mcp_servers_dir().join("qcc");
            std::fs::create_dir_all(&dir).unwrap();
            let manifest = r#"{
                "id":"qcc","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],
                "command":"python","args":[],
                "servers":[
                    {
                        "name":"qcc-company",
                        "url":"https://agent.qcc.com/mcp/company/stream",
                        "scopes":["mcp:tools"],
                        "oauth_resource":"https://agent.qcc.com/mcp/company/stream"
                    }
                ]
            }"#;
            std::fs::write(dir.join("manifest.json"), manifest).unwrap();

            let mgr = MarketplaceManager::new();
            // 对齐真实 qcc manifest 的 qcc-company OAuth 远程 server。
            assert_eq!(
                mgr.model_tool_names(&["qcc".to_string()]),
                vec!["mcp_qcc-company_*".to_string()]
            );
        });
    }

    #[test]
    fn model_tool_names_generates_prefix_rules_for_remote_oauth_server() {
        with_temp_home(|| {
            write_tool_manifest(
                "yuandian-mcp",
                r#"{
                    "id":"yuandian-mcp","name":"华宇元典","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "servers":[
                        {
                            "name":"yuandian_mcp",
                            "url":"https://open.chineselaw.com/mcp",
                            "scopes":["mcp"],
                            "oauth_resource":"https://open.chineselaw.com/mcp"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            assert_eq!(
                mgr.model_tool_names(&["yuandian-mcp".to_string()]),
                vec!["mcp_yuandian_mcp_*".to_string()]
            );
        });
    }

    #[test]
    fn install_remote_oauth_server_writes_deepseek_oauth_config() {
        with_temp_home(|| {
            write_tool_manifest(
                "yuandian-mcp",
                r#"{
                    "id":"yuandian-mcp","name":"华宇元典","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "servers":[
                        {
                            "name":"yuandian_mcp",
                            "url":"https://open.chineselaw.com/mcp",
                            "scopes":["mcp"],
                            "oauth_resource":"https://open.chineselaw.com/mcp"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            mgr.install("yuandian-mcp", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let server = &mcp["servers"]["yuandian_mcp"];
            assert_eq!(server["url"], "https://open.chineselaw.com/mcp");
            assert_eq!(server["scopes"], serde_json::json!(["mcp"]));
            assert_eq!(server["oauth_resource"], "https://open.chineselaw.com/mcp");
            assert!(server.get("headers").is_none());
            assert_eq!(
                mgr.oauth_remote_server_name("yuandian-mcp").as_deref(),
                Some("yuandian_mcp")
            );
        });
    }

    #[test]
    fn canva_oauth_server_writes_config_and_model_prefix() {
        with_temp_home(|| {
            write_tool_manifest(
                "canva-mcp",
                r#"{
                    "id":"canva-mcp","name":"Canva 可画","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "servers":[
                        {
                            "name":"canva_mcp",
                            "url":"https://mcp.canva.cn/mcp",
                            "scopes":[
                                "profile:read",
                                "design:meta:read",
                                "design:content:write",
                                "design:content:read",
                                "folder:read",
                                "folder:write",
                                "brandtemplate:content:read",
                                "brandtemplate:meta:read",
                                "brandtemplate:content:write",
                                "comment:write",
                                "comment:read",
                                "asset:read",
                                "asset:write",
                                "brandkit:read",
                                "help:answers:read",
                                "help:answers:write"
                            ],
                            "oauth_resource":"https://mcp.canva.cn/mcp"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            mgr.install("canva-mcp", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let server = &mcp["servers"]["canva_mcp"];
            assert_eq!(server["url"], "https://mcp.canva.cn/mcp");
            assert_eq!(server["oauth_resource"], "https://mcp.canva.cn/mcp");
            assert!(server["scopes"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("profile:read")));
            assert!(server["scopes"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("design:content:write")));
            assert!(server.get("headers").is_none());
            assert!(server.get("env_headers").is_none());
            assert!(server.get("bearer_token_env_var").is_none());
            assert_eq!(
                mgr.oauth_remote_server_name("canva-mcp").as_deref(),
                Some("canva_mcp")
            );
            assert_eq!(
                mgr.model_tool_names(&["canva-mcp".to_string()]),
                vec!["mcp_canva_mcp_*".to_string()]
            );
        });
    }

    #[test]
    fn install_qcc_oauth_server_writes_deepseek_oauth_config() {
        with_temp_home(|| {
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"企查查","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "config_fields":[],
                    "servers":[
                        {
                            "name":"qcc-company",
                            "url":"https://agent.qcc.com/mcp/company/stream",
                            "scopes":["mcp:tools"],
                            "oauth_resource":"https://agent.qcc.com/mcp/company/stream"
                        }
                    ]
                }"#,
            );

            let mgr = MarketplaceManager::new();
            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let server = &mcp["servers"]["qcc-company"];
            assert_eq!(server["url"], "https://agent.qcc.com/mcp/company/stream");
            assert_eq!(server["scopes"], serde_json::json!(["mcp:tools"]));
            assert_eq!(
                server["oauth_resource"],
                "https://agent.qcc.com/mcp/company/stream"
            );
            assert!(server.get("headers").is_none());
            assert!(server.get("bearer_token_env_var").is_none());
            assert_eq!(
                mgr.oauth_remote_server_name("qcc").as_deref(),
                Some("qcc-company")
            );
        });
    }

    /// 全局禁用列表落盘往返:存→读一致;清空→读空;没文件→读空。
    #[test]
    fn disabled_connectors_persist_roundtrip() {
        with_temp_home(|| {
            assert!(load_disabled_connectors().is_empty()); // 无文件 → 空
            save_disabled_connectors(&["weather".to_string(), "pptx".to_string()]);
            assert_eq!(
                load_disabled_connectors(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            save_disabled_connectors(&[]); // 全开回去
            assert!(load_disabled_connectors().is_empty());
        });
    }

    /// 双 scope(plain/code)独立持久化:互不影响,code 首次写会标记已初始化。
    #[test]
    fn disabled_connectors_scope_isolation() {
        with_temp_home(|| {
            // 模拟已装 2 个连接器。
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            // 未初始化:code 默认全禁已装连接器 ∪ 内置 CLI 连接器;plain 仍按空处理。
            assert!(load_disabled_connectors_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ]
            );
            // plain 写 weather → code 不受影响(仍默认全禁)。
            save_disabled_connectors_for(ConnectorScope::Plain, &["weather".to_string()]);
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            // code 显式写 → 标记初始化,此后以落盘为准。
            save_disabled_connectors_for(ConnectorScope::Code, &["pptx".to_string()]);
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
            // plain 再写空,不影响 code。
            save_disabled_connectors_for(ConnectorScope::Plain, &[]);
            assert!(load_disabled_connectors_for(ConnectorScope::Plain).is_empty());
            assert_eq!(
                load_disabled_connectors_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
        });
    }

    /// companion_skills：CLI 连接器（无 manifest）查能力包注册表静态表返回配套
    /// 技能目录；MCP 连接器仍走 manifest 声明；未知 id 返回空。
    #[test]
    fn companion_skills_covers_cli_bundles_and_manifests() {
        with_temp_home(|| {
            let market = MarketplaceManager::new();
            // CLI 连接器：静态表（feishu 9 个 lark-* 目录）。
            assert_eq!(
                market.companion_skills("feishu"),
                bundle::LARK_SKILL_DIRS
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(market.companion_skills("dingtalk"), vec!["dws".to_string()]);
            // MCP 连接器：manifest 声明。
            write_tool_manifest(
                "gongwen",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"file-text","category":"办公","mcp_tools":["mcp_gongwen_make_gongwen"],"command":"python","args":["server.py"],"companion_skills":["government-writing"]}"#,
            );
            assert_eq!(
                market.companion_skills("gongwen"),
                vec!["government-writing".to_string()]
            );
            // 未知 id → 空。
            assert!(market.companion_skills("nonexistent").is_empty());
        });
    }

    /// 旧版裸数组格式 `["a","b"]` 迁移到 bundles 文件 plain scope，code 保持未初始化
    /// 默认（DenyAll → 全部已装包 + 内置 CLI 包）。
    #[test]
    fn bundles_legacy_connector_array_migrates_to_bundles_file() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let conn = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(&conn, r#"["weather","pptx"]"#).unwrap();
            assert_eq!(
                load_disabled_bundles(),
                vec!["weather".to_string(), "pptx".to_string()]
            );
            // 旧格式不初始化 code scope → 仍默认全禁（含内置 CLI 包）。
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ]
            );
            // 读到即迁移：落盘已是 bundles 新 map 格式。
            let content = std::fs::read_to_string(
                crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
            )
            .unwrap();
            assert!(
                content.contains("\"scopes\""),
                "迁移后应为新格式: {content}"
            );
        });
    }

    /// 旧双 scope 对象 `{plain, code, code_initialized}` 迁移为 bundles scopes map。
    #[test]
    fn bundles_legacy_connector_object_migrates_to_scopes_map() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let conn = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(
                &conn,
                r#"{"plain":["weather"],"code":["pptx"],"code_initialized":true}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["pptx".to_string()]
            );
            let file = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert!(file.initialized.contains("code"));
            let content = std::fs::read_to_string(
                crate::platform::paths::pinvou3_home().join("disabled_bundles.json"),
            )
            .unwrap();
            assert!(
                content.contains("\"scopes\""),
                "迁移后应为新格式: {content}"
            );
        });
    }

    /// 旧对象 `code_initialized=false` 时，code 数组被忽略、按 DenyAll 默认全禁
    /// （含内置 CLI 包）；plain 列表即使无 initialized 标记也必须生效。
    #[test]
    fn bundles_legacy_object_uninitialized_code_keeps_deny_all_default() {
        with_temp_home(|| {
            write_installed_ids(&["weather".to_string(), "pptx".to_string()]);
            let conn = crate::platform::paths::pinvou3_home().join("disabled_connectors.json");
            std::fs::create_dir_all(conn.parent().unwrap()).unwrap();
            std::fs::write(
                &conn,
                r#"{"plain":["weather"],"code":[],"code_initialized":false}"#,
            )
            .unwrap();
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["weather".to_string()]
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec![
                    "weather".to_string(),
                    "pptx".to_string(),
                    "feishu".to_string(),
                    "wecom".to_string(),
                    "dingtalk".to_string(),
                    "tmeet".to_string(),
                ],
                "code 未初始化应按 DenyAll 默认全禁已装包 + 内置 CLI 包"
            );
        });
    }

    /// 旧技能文件 `skill:` 前缀条目迁移为包 id：剥前缀 + companion 映射到所属包。
    #[test]
    fn bundles_legacy_skill_prefix_maps_to_owner_package() {
        with_temp_home(|| {
            // government-writing 是 gongwen MCP 的 companion 技能 → 映射到 gongwen 包。
            write_tool_manifest(
                "gongwen",
                r#"{"id":"gongwen","name":"公文写作","description":"d","version":"1.0.0","icon":"file-text","category":"办公","mcp_tools":["mcp_gongwen_make_gongwen"],"command":"python","args":["server.py"],"companion_skills":["government-writing"]}"#,
            );
            let skills = crate::platform::paths::pinvou3_home().join("disabled_skills.json");
            std::fs::create_dir_all(skills.parent().unwrap()).unwrap();
            std::fs::write(
                &skills,
                r#"{"scopes":{"plain":["skill:visualizer","government-writing"]},"initialized":["plain"]}"#,
            )
            .unwrap();
            let mut got = load_disabled_bundles();
            got.sort();
            assert_eq!(
                got,
                vec!["gongwen".to_string(), "visualizer".to_string()],
                "skill: 前缀剥除 + companion 技能映射到其 MCP 包"
            );
        });
    }

    /// 新格式未知键经读-改-写后保留（前向兼容）。
    #[test]
    fn bundles_unknown_keys_survive_roundtrip() {
        with_temp_home(|| {
            let path = crate::platform::paths::pinvou3_home().join("disabled_bundles.json");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"{"scopes":{"plain":["weather"]},"initialized":["plain"],"future_field":{"v":1}}"#,
            )
            .unwrap();
            save_disabled_bundles_for(ConnectorScope::Plain, &["pptx".to_string()]);
            let content = std::fs::read_to_string(&path).unwrap();
            assert!(
                content.contains("future_field"),
                "未知键应在读写后保留: {content}"
            );
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
        });
    }

    /// 新装包：code 未初始化时无需落盘；已初始化时自动加入禁用集（默认仍关）。
    #[test]
    fn bundles_sync_deny_all_after_install_keeps_new_bundle_disabled() {
        with_temp_home(|| {
            write_installed_ids(&["pptx".to_string()]);
            sync_deny_all_scopes_after_install("weather");
            assert!(
                crate::features::marketplace::scope::load_disabled_bundles_file()
                    .scopes
                    .get("code")
                    .map(|ids| ids.is_empty())
                    .unwrap_or(true)
            );
            save_disabled_bundles_for(ConnectorScope::Code, &[]);
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
            sync_deny_all_scopes_after_install("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Code),
                vec!["weather".to_string()]
            );
        });
    }

    /// 卸载包：从 plain/code 两个 scope 禁用集移除。
    #[test]
    fn bundles_remove_bundle_cleans_both_scopes() {
        with_temp_home(|| {
            save_disabled_bundles_for(
                ConnectorScope::Plain,
                &["weather".to_string(), "pptx".to_string()],
            );
            save_disabled_bundles_for(ConnectorScope::Code, &["weather".to_string()]);
            remove_bundle_from_disabled_scopes("weather");
            assert_eq!(
                load_disabled_bundles_for(ConnectorScope::Plain),
                vec!["pptx".to_string()]
            );
            assert!(load_disabled_bundles_for(ConnectorScope::Code).is_empty());
        });
    }

    /// 两个 scope 并发写同一文件：进程内串行化 + 原子写保证不丢更新。
    #[test]
    fn bundles_concurrent_scope_writes_do_not_lose_updates() {
        with_temp_home(|| {
            let plain_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_bundles_for(ConnectorScope::Plain, &["weather".to_string()]);
                }
            });
            let code_writer = std::thread::spawn(|| {
                for _ in 0..50 {
                    save_disabled_bundles_for(ConnectorScope::Code, &["pptx".to_string()]);
                }
            });
            plain_writer.join().unwrap();
            code_writer.join().unwrap();

            let file = crate::features::marketplace::scope::load_disabled_bundles_file();
            assert_eq!(file.scopes.get("plain"), Some(&vec!["weather".to_string()]));
            assert!(file.initialized.contains("code"));
            assert_eq!(file.scopes.get("code"), Some(&vec!["pptx".to_string()]));
        });
    }
    /// 连接器禁用联动技能、独立 skill 开关、同名统一三个场景的组合目录断言
    /// 已随 `enabled_skills_for` 移入 `assistant::skill_materialization` 的测试
    /// （marketplace → assistant 会构成 feature 依赖环，架构守卫拒绝）。
    /// 覆盖位置：`skill_materialization.rs` tests 中
    /// `companion_skill_excluded_when_connector_disabled` /
    /// `enabled_skills_respect_first_wins_and_scope_disabled` /
    /// `disabling_package_hides_same_named_skill_unified`。

    #[test]
    fn secret_manifest_parses_declarations_without_plain_secret_values() {
        let weather: ToolManifest = serde_json::from_str(
            r#"{
                "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
            }"#,
        )
        .unwrap();
        assert!(weather.env.is_empty());
        assert_eq!(weather.secret_env[0].key, "AMAP_KEY");

        let qcc: ToolManifest = serde_json::from_str(
            r#"{
                "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                "mcp_tools":[],"command":"","args":[],
                "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
            }"#,
        )
        .unwrap();
        assert!(qcc.env.is_empty());
        assert_eq!(qcc.secret_headers[0].source_key, "QCC_API_KEY");
    }

    #[test]
    fn install_local_secret_env_writes_placeholder_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("amap");
            store
                .set(&mcp_secret_reference("weather", "env", "AMAP_KEY"), &secret)
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("weather", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let amap = mcp["servers"]["weather"]["env"]["AMAP_KEY"]
                .as_str()
                .unwrap();
            assert_eq!(amap, "${PINVOU3_MCP_SECRET_AMAP_KEY}");
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_qcc_secret_header_uses_bearer_env_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                    "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("qcc");
            store
                .set(
                    &mcp_secret_reference("qcc", "header", "QCC_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            assert!(mcp["servers"]["qcc-company"].get("headers").is_none());
            assert_eq!(
                mcp["servers"]["qcc-company"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_QCC_API_KEY"
            );
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn install_patsnap_secret_header_uses_bearer_env_without_plain_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap");
            store
                .set(
                    &mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("patsnap-search", &std::collections::HashMap::new())
                .unwrap();

            let mcp = read_mcp_json();
            let url = mcp["servers"]["patsnap-search"]["url"].as_str().unwrap();
            assert_eq!(url, "https://connect.zhihuiya.com/2b0355/logic-mcp");
            assert!(mcp["servers"]["patsnap-search"].get("headers").is_none());
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert!(!mcp.to_string().contains(&secret));
        });
    }

    #[test]
    fn sync_secret_env_vars_restores_header_secret() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap-sync");
            store
                .set(
                    &mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY"),
                    &secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);
            mgr.save_installed(&["patsnap-search".to_string()]).unwrap();
            std::env::remove_var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY");

            mgr.sync_secret_env_vars().unwrap();

            assert_eq!(
                std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY")
                    .ok()
                    .as_deref(),
                Some(secret.as_str())
            );
        });
    }

    #[test]
    fn uninstall_remote_secret_header_removes_credential_and_env() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let secret = secret_value("patsnap-uninstall");
            let reference = mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY");
            store.set(&reference, &secret).unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            mgr.install("patsnap-search", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY")
                    .ok()
                    .as_deref(),
                Some(secret.as_str())
            );

            mgr.uninstall("patsnap-search").unwrap();

            assert!(!mgr.installed_ids().contains(&"patsnap-search".to_string()));
            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert_eq!(store.get(&reference).unwrap(), None);
            assert!(std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_err());
        });
    }

    #[test]
    fn uninstall_patsnap_does_not_remove_other_connector_secrets() {
        with_temp_home(|| {
            write_tool_manifest(
                "patsnap-search",
                r#"{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}],
                    "servers":[{"name":"patsnap-search","url":"https://connect.zhihuiya.com/2b0355/logic-mcp"}]
                }"#,
            );
            write_tool_manifest(
                "qcc",
                r#"{
                    "id":"qcc","name":"QCC","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":[],"command":"","args":[],
                    "secret_headers":[{"header":"Authorization","scheme":"Bearer","source_key":"QCC_API_KEY","provider":"qcc","required":true}],
                    "servers":[{"name":"qcc-company","url":"https://example.invalid/mcp"}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            let patsnap_secret = secret_value("patsnap-isolated");
            let qcc_secret = secret_value("qcc-isolated");
            let patsnap_ref = mcp_secret_reference("patsnap-search", "header", "PATSNAP_API_KEY");
            let qcc_ref = mcp_secret_reference("qcc", "header", "QCC_API_KEY");
            store.set(&patsnap_ref, &patsnap_secret).unwrap();
            store.set(&qcc_ref, &qcc_secret).unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            mgr.install("patsnap-search", &std::collections::HashMap::new())
                .unwrap();
            mgr.install("qcc", &std::collections::HashMap::new())
                .unwrap();
            assert_eq!(
                std::env::var("PINVOU3_MCP_SECRET_QCC_API_KEY")
                    .ok()
                    .as_deref(),
                Some(qcc_secret.as_str())
            );

            mgr.uninstall("patsnap-search").unwrap();

            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert!(mcp["servers"].get("qcc-company").is_some());
            assert_eq!(store.get(&patsnap_ref).unwrap(), None);
            assert!(std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_err());
            assert_eq!(store.get(&qcc_ref).unwrap(), Some(qcc_secret.clone()));
            assert_eq!(
                std::env::var("PINVOU3_MCP_SECRET_QCC_API_KEY")
                    .ok()
                    .as_deref(),
                Some(qcc_secret.as_str())
            );
        });
    }

    #[tokio::test(flavor = "current_thread")]
    async fn validate_patsnap_with_invalid_token_can_be_rolled_back() {
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-search",
                &format!(
                    r#"{{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["patsnap_search","patsnap_fetch"],"command":"","args":[],
                    "validate_on_install":true,
                    "secret_headers":[{{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}}],
                    "servers":[{{"name":"patsnap-search","url":"{}"}}]
                }}"#,
                    mock.url
                ),
            );
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());
            let mut config = std::collections::HashMap::new();
            config.insert("PATSNAP_API_KEY".to_string(), "wrong-token".to_string());

            mgr.install("patsnap-search", &config).unwrap();
            let err = mgr
                .validate_remote_connection("patsnap-search")
                .await
                .unwrap_err();
            mgr.uninstall("patsnap-search").unwrap();

            assert!(err.contains("API Key 无效"), "unexpected error: {err}");
            assert!(!err.contains("无法连接远程 MCP 服务"));
            assert!(!mgr.installed_ids().contains(&"patsnap-search".to_string()));
            let mcp = read_mcp_json();
            assert!(mcp["servers"].get("patsnap-search").is_none());
            assert_eq!(store
                .get(&mcp_secret_reference(
                    "patsnap-search",
                    "header",
                    "PATSNAP_API_KEY"
                ))
                .unwrap(), None);
            assert!(std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY").is_err());
            assert!(!err.contains("wrong-token"));
        })
        .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn validate_patsnap_with_valid_token_discovers_expected_tools() {
        with_temp_home_async(|| async {
            let mock = spawn_mock_mcp_server("valid-token").await;
            write_tool_manifest(
                "patsnap-search",
                &format!(
                    r#"{{
                    "id":"patsnap-search","name":"Patsnap","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["patsnap_search","patsnap_fetch"],"command":"","args":[],
                    "validate_on_install":true,
                    "secret_headers":[{{"header":"Authorization","scheme":"Bearer","source_key":"PATSNAP_API_KEY","provider":"patsnap","required":true}}],
                    "servers":[{{"name":"patsnap-search","url":"{}"}}]
                }}"#,
                    mock.url
                ),
            );
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());
            let mut config = std::collections::HashMap::new();
            config.insert("PATSNAP_API_KEY".to_string(), "valid-token".to_string());

            mgr.install("patsnap-search", &config).unwrap();
            let validation = mgr
                .validate_remote_connection("patsnap-search")
                .await
                .unwrap();

            assert!(mgr.installed_ids().contains(&"patsnap-search".to_string()));
            let mcp = read_mcp_json();
            assert_eq!(
                mcp["servers"]["patsnap-search"]["bearer_token_env_var"],
                "PINVOU3_MCP_SECRET_PATSNAP_API_KEY"
            );
            assert!(!mcp.to_string().contains("valid-token"));
            assert_eq!(
                store
                    .get(&mcp_secret_reference(
                        "patsnap-search",
                        "header",
                        "PATSNAP_API_KEY"
                    ))
                    .unwrap()
                    .as_deref(),
                Some("valid-token")
            );
            assert_eq!(
                std::env::var("PINVOU3_MCP_SECRET_PATSNAP_API_KEY")
                    .ok()
                    .as_deref(),
                Some("valid-token")
            );
            assert!(validation.tools.contains(&"patsnap_search".to_string()));
            assert!(validation.tools.contains(&"patsnap_fetch".to_string()));
            let seen = mock.seen_methods.lock().unwrap().clone();
            assert!(seen.contains(&"initialize".to_string()));
            assert!(seen.contains(&"tools/list".to_string()));
        })
        .await;
    }

    #[test]
    fn install_failure_does_not_leave_half_installed_state() {
        // #2 半安装回归:缺密钥导致 add_to_mcp_json 失败时,installed.json 不该记录该工具
        // (顺序修复=先写 mcp 成功、再 save_installed)。
        with_temp_home(|| {
            write_tool_manifest(
                "weather-custom",
                r#"{
                    "id":"weather-custom","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"WEATHER_TEST_KEY","provider":"weather-test","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());
            // 不提供 key + keyring 空 → install 必失败
            assert!(
                mgr.install("weather-custom", &std::collections::HashMap::new())
                    .is_err(),
                "缺密钥应安装失败"
            );
            assert!(
                !mgr.installed_ids().contains(&"weather-custom".to_string()),
                "失败时 installed.json 不该记录该工具(否则半安装)"
            );
        });
    }

    #[test]
    fn weather_missing_user_key_fails_without_fallback() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("weather", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("AMAP_KEY"), "错误应提示缺少 AMAP_KEY: {err}");
            assert!(
                !mgr.installed_ids().contains(&"weather".to_string()),
                "缺用户 key 时不应写 installed.json"
            );
            assert!(
                !crate::bridge::paths::mcp_config_path().is_file(),
                "缺用户 key 时不应写入 mcp.json"
            );
        });
    }

    #[test]
    fn migrate_legacy_manifest_env_moves_secret_to_store_and_removes_plaintext() {
        with_temp_home(|| {
            let secret = secret_value("legacy-amap");
            // 明文密钥迁移针对旧布局存量 manifest（`bundle/mcp-servers/<id>/`），
            // 直接铺旧路径模拟历史数据，不走新布局 helper。
            let manifest = format!(
                r#"{{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "env":{{"AMAP_KEY":"{secret}","SAFE_VALUE":"kept"}}
                }}"#
            );
            let legacy_dir = crate::platform::paths::bundle_mcp_servers_dir().join("weather");
            std::fs::create_dir_all(&legacy_dir).unwrap();
            std::fs::write(legacy_dir.join("manifest.json"), &manifest).unwrap();
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("weather", "env", "AMAP_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(
                crate::platform::paths::bundle_mcp_servers_dir()
                    .join("weather")
                    .join("manifest.json"),
            )
            .unwrap();
            assert!(!content.contains(&secret));
            assert!(content.contains("SAFE_VALUE"));
        });
    }

    #[test]
    fn migrate_legacy_qcc_bearer_header_uses_env_and_stores_secret() {
        with_temp_home(|| {
            let secret = secret_value("legacy-qcc");
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.migrated_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&secret));
            assert!(
                content.contains("\"bearer_token_env_var\": \"PINVOU3_MCP_SECRET_QCC_API_KEY\"")
            );
        });
    }

    #[test]
    fn migration_does_not_overwrite_existing_credential_but_cleans_file() {
        with_temp_home(|| {
            let old_secret = secret_value("old-qcc");
            let kept_secret = secret_value("kept-qcc");
            let mcp_path = crate::platform::paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                format!(
                    r#"{{
                        "servers": {{
                            "qcc-company": {{
                                "url": "https://example.invalid/mcp",
                                "headers": {{"Authorization": "Bearer {old_secret}"}}
                            }}
                        }}
                    }}"#
                ),
            )
            .unwrap();
            let store = MemoryCredentialStore::default();
            store
                .set(
                    &mcp_secret_reference("qcc", "header", "QCC_API_KEY"),
                    &kept_secret,
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store.clone());

            let result = mgr.migrate_mcp_plaintext_secrets().unwrap();

            assert_eq!(result.skipped_count, 1);
            let stored = store
                .get(&mcp_secret_reference("qcc", "header", "QCC_API_KEY"))
                .unwrap();
            assert_eq!(stored.as_deref(), Some(kept_secret.as_str()));
            let content = std::fs::read_to_string(&mcp_path).unwrap();
            assert!(!content.contains(&old_secret));
            assert!(!content.contains(&kept_secret));
            assert!(
                content.contains("\"bearer_token_env_var\": \"PINVOU3_MCP_SECRET_QCC_API_KEY\"")
            );
        });
    }

    #[test]
    fn install_missing_required_secret_returns_recoverable_redacted_error() {
        with_temp_home(|| {
            write_tool_manifest(
                "iwencai-custom",
                r#"{
                    "id":"iwencai-custom","name":"Iwencai","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_iwencai_query"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"IWENCAI_TEST_KEY","provider":"iwencai-test","required":true}]
                }"#,
            );
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());

            let err = mgr
                .install("iwencai-custom", &std::collections::HashMap::new())
                .unwrap_err();

            assert!(err.contains("IWENCAI_TEST_KEY"));
            assert!(!err.contains("test-secret"));
        });
    }

    /// Phase 2 镜像：install 成功后 bundles.json 出现 Preset 记录（凭据 key 从
    /// manifest 收敛），uninstall 成功后记录删除；两条路径的失败都不翻盘主操作。
    #[test]
    fn install_and_uninstall_mirror_bundle_store_record() {
        with_temp_home(|| {
            write_tool_manifest(
                "weather",
                r#"{
                    "id":"weather","name":"Weather","description":"d","version":"1","icon":"x","category":"c",
                    "mcp_tools":["mcp_weather_get_weather"],"command":"python","args":["server.py"],
                    "secret_env":[{"key":"AMAP_KEY","provider":"amap","required":true}]
                }"#,
            );
            let store = MemoryCredentialStore::default();
            store
                .set(
                    &mcp_secret_reference("weather", "env", "AMAP_KEY"),
                    &secret_value("amap"),
                )
                .unwrap();
            let mgr = MarketplaceManager::with_store(store);

            mgr.install("weather", &std::collections::HashMap::new())
                .unwrap();
            let bundle_store = store::BundleStore::new();
            let record = bundle_store
                .get("weather")
                .unwrap()
                .expect("install 应镜像登记 bundles.json");
            assert_eq!(record.source, store::BundleSource::Preset);
            assert!(record.installed);
            assert_eq!(
                record.credential_keys,
                vec!["AMAP_KEY".to_string()],
                "凭据 key 应从 manifest 收敛"
            );

            mgr.uninstall("weather").unwrap();
            assert!(
                bundle_store.get("weather").unwrap().is_none(),
                "uninstall 应镜像删除记录"
            );
        });
    }

    /// 刀十一：install 按需释放包资源到 bundles/<id>/mcp/（内嵌目录供清单，
    /// 无需种 manifest），mcp.json 指向新布局；uninstall 清理包目录。
    #[test]
    fn install_releases_package_to_new_layout_and_uninstall_cleans() {
        with_temp_home(|| {
            // 生产环境 bundle/ 由启动释放流程创建；测试不跑释放，需先建
            std::fs::create_dir_all(paths::bundle_root()).unwrap();
            let mgr = MarketplaceManager::with_store(MemoryCredentialStore::default());
            let mut config = std::collections::HashMap::new();
            config.insert("AMAP_KEY".to_string(), "amap-test".to_string());
            mgr.install("weather", &config).unwrap();

            let mcp_dir = paths::bundles_root().join("weather").join("mcp");
            assert!(mcp_dir.join("server.py").is_file(), "脚本应释放到包目录");
            assert!(mcp_dir.join("manifest.json").is_file());
            assert!(
                !paths::bundle_mcp_servers_dir().join("weather").exists(),
                "不应再落旧布局"
            );
            let mcp = read_mcp_json();
            let arg0 = mcp["servers"]["weather"]["args"][0].as_str().unwrap();
            assert!(arg0.contains("bundles"), "mcp.json 应指向新布局: {arg0}");
            assert!(!arg0.contains("mcp-servers"), "不应指向旧布局: {arg0}");
            let record = store::BundleStore::new().get("weather").unwrap().unwrap();
            assert!(record.content_fingerprint.is_some(), "应登记包内容指纹");

            mgr.uninstall("weather").unwrap();
            assert!(
                !paths::bundles_root().join("weather").exists(),
                "卸载应清包目录"
            );
        });
    }

    /// 刀十一：存量 mcp.json 条目的旧布局路径迁移（重写为新包目录，幂等）。
    #[test]
    fn migrate_mcp_json_paths_rewrites_legacy_entries() {
        with_temp_home(|| {
            let legacy_script = paths::bundle_mcp_servers_dir()
                .join("weather")
                .join("server.py");
            let mcp_path = paths::mcp_config_path();
            std::fs::create_dir_all(mcp_path.parent().unwrap()).unwrap();
            std::fs::write(
                &mcp_path,
                serde_json::json!({
                    "servers": {"weather": {"command": "python3", "args": [legacy_script.to_string_lossy()]}}
                })
                .to_string(),
            )
            .unwrap();

            assert!(migrate_mcp_json_paths().unwrap(), "首次迁移应有改动");
            let mcp = read_mcp_json();
            let arg0 = mcp["servers"]["weather"]["args"][0].as_str().unwrap();
            let expected = mcp_catalog::package_mcp_dir("weather").join("server.py");
            assert_eq!(arg0, expected.to_string_lossy().as_ref());
            assert!(
                !migrate_mcp_json_paths().unwrap(),
                "二次迁移应幂等（无改动）"
            );
        });
    }
}
