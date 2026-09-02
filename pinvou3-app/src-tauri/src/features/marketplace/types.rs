//! 工具市场的数据类型:manifest 元数据、前端展示模型、迁移结果。
//!
//! 这里只放类型定义与对应的 serde 默认函数,不含任何业务逻辑。

use serde::{Deserialize, Serialize};

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
    /// Platform-specific, hash-locked Python wheels. A matching target is installed into an
    /// isolated user environment; only non-Windows targets may use the legacy pip fallback.
    #[serde(default)]
    pub(crate) python_dependencies: Option<super::python_dependencies::PythonDependencyLock>,
    #[serde(default)]
    pub servers: Vec<RemoteServer>,
    /// 配套技能 id:装该 MCP 时一并装、卸时一并删(让"一个能力"=引擎+引导整体装卸)。
    #[serde(default)]
    pub companion_skills: Vec<String>,
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

impl RemoteServer {
    /// 是否需要 OAuth 授权：scopes / oauth 配置 / oauth_resource 任一声明即算。
    /// `oauth_remote_server_name` 与包清单 `oauth` 标记共用本判据，口径单点维护——
    /// 仅带 url 的远程 MCP（如智慧芽，Bearer key 走 secret_headers）不算 OAuth。
    pub fn requires_oauth(&self) -> bool {
        !self.scopes.is_empty()
            || self.oauth.is_some()
            || self
                .oauth_resource
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }
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

pub(super) fn default_target() -> String {
    "env".to_string()
}

pub(super) fn default_required() -> bool {
    true
}

pub(super) fn default_bearer_scheme() -> String {
    "Bearer".to_string()
}

/// `MarketplaceToolInfo.source` 的 serde 缺省值：旧数据/旧前端缺字段时按
/// 「非上传」处理（builtin）——宁可少提示「移入回收站」，不对自定义 MCP 说谎。
pub(super) fn default_tool_source() -> String {
    "builtin".to_string()
}

// ---------------------------------------------------------------------------
// mcp.json 明文密钥迁移结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpSecretMigrationResult {
    pub migrated_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub messages: Vec<String>,
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
    /// 包来源："upload" | "preset" | "builtin" —— 与 bundles.json 的 BundleSource
    /// 对应（无记录的内置市场条目 = builtin；migrate_custom_mcp_layout 迁移的手写
    /// 自定义 MCP 登记为 preset）。前端据此区分卸载文案：仅 upload 卸载进回收站
    /// （M4：此前非内置后端卡一律标 userUploaded，自定义 MCP 卸载提示「已移入
    /// 回收站」而实际保留目录，文案说谎）。
    #[serde(default = "default_tool_source")]
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceToolValidation {
    pub tool_id: String,
    pub connected: bool,
    pub tools: Vec<String>,
}
