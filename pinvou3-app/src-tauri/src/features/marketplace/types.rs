//! Data types for the tool marketplace: manifest metadata and frontend display models.
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
    // --- 内置工具集长期契约 §3.1:内置语义字段(全部可选,缺省即普通插件) ---
    /// 内置插件标记:true = 随应用发布、不可卸载/不可停用(服务端纵深防御,
    /// 见 `super::builtin`)。
    #[serde(default)]
    pub builtin: bool,
    /// 展示可见性:"normal"(缺省)| 内置插件为 "system"(前端据此归入系统板块)。
    #[serde(default)]
    pub visibility: String,
    /// 数据安全等级:"L0" | "L1" | "L2"(语义由前端本地化展示)。
    #[serde(default)]
    pub security_level: String,
    /// 数据访问语义 scope 键(如 "sessions.read";前端负责本地化)。
    #[serde(default)]
    pub data_access: Vec<String>,
    /// 工具全名 → 功能 id 数组(多对多归属;功能开关按并集语义摘除工具,
    /// 见 `super::builtin::feature_disabled_tool_names`)。
    #[serde(default)]
    pub tool_features: std::collections::HashMap<String, Vec<String>>,
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

/// `MarketplaceToolInfo.exportable` 的 serde 缺省值：旧后端数据缺字段时按
/// 「可导出」处理 —— 与导入/回收站导出通道的既有行为一致，前端不因升级漏按钮。
pub(super) fn default_tool_exportable() -> bool {
    true
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
    /// 是否可导出为标准插件包 zip：`mcp_catalog` 预置目录包为 false（导出的 zip
    /// 受导入管线预置冲突保护、无法重新导入，后端 `export_installed_plugin` 也会
    /// 拒绝），迁移登记为 Preset 的手写自定义 MCP、上传包为 true。前端据此隐藏
    /// 详情页「导出」按钮，与后端 fail-fast 口径一致（避免按钮必然报错）。
    #[serde(default = "default_tool_exportable")]
    pub exportable: bool,
    // --- 内置工具集长期契约 §3.1:内置语义透传(仅内置插件非空,空值省略) ---
    /// 内置插件标记(来自 manifest `builtin`)。
    #[serde(default)]
    pub builtin: bool,
    /// 数据安全等级(仅内置插件填,如 "L0";前端本地化展示)。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_level: Option<String>,
    /// 数据访问语义 scope 键(仅内置插件填;前端本地化展示)。
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub data_access: Vec<String>,
    /// 该插件提供的 MCP 工具全名清单(前端详情展示用)。
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<String>,
    /// 内置插件随应用发布的 bundle 版本(仅内置插件填 Some,其余省略)。
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 旧 manifest（无内置语义字段）可解析：全部缺省 = 普通插件（契约 §3.1
    /// 「全部可选、缺省即普通插件」）。
    #[test]
    fn legacy_manifest_without_builtin_fields_parses_as_normal() {
        let json = r#"{
            "id":"weather","name":"Weather","description":"d","version":"1","icon":"x",
            "category":"c","mcp_tools":["get_weather"],"command":"python","args":["server.py"]
        }"#;
        let manifest: ToolManifest = serde_json::from_str(json).unwrap();
        assert!(!manifest.builtin);
        assert!(manifest.visibility.is_empty());
        assert!(manifest.security_level.is_empty());
        assert!(manifest.data_access.is_empty());
        assert!(manifest.tool_features.is_empty());
    }

    /// 内置 manifest（session-reader 内嵌快照）的 5 个契约字段解析正确，
    /// tool_features 的键与 mcp_tools 全名完全一致（共享契约硬约束）。
    #[test]
    fn session_reader_manifest_carries_builtin_contract_fields() {
        let manifest =
            crate::features::marketplace::mcp_catalog::embedded_manifest("session-reader")
                .unwrap()
                .expect("session-reader 在内嵌目录中");
        assert!(manifest.builtin);
        assert_eq!(manifest.visibility, "system");
        assert_eq!(manifest.security_level, "L0");
        assert_eq!(manifest.data_access, ["sessions.read".to_string()]);
        assert_eq!(manifest.tool_features.len(), 2);
        for tool in &manifest.mcp_tools {
            let features = manifest
                .tool_features
                .get(tool)
                .unwrap_or_else(|| panic!("tool_features 缺 {tool} 的归属"));
            assert_eq!(
                features,
                &["session-mention".to_string(), "long-memory".to_string()]
            );
        }
    }

    /// MarketplaceToolInfo 前端契约：内置插件带 security_level/data_access/
    /// mcp_tools/bundle_version；普通插件这些字段序列化时省略（契约干净）。
    #[test]
    fn tool_info_omits_builtin_fields_for_normal_plugins() {
        let normal = MarketplaceToolInfo {
            id: "weather".into(),
            name: "Weather".into(),
            description: "d".into(),
            version: "1".into(),
            installed: true,
            companion_skills: vec![],
            source: "builtin".into(),
            exportable: false,
            builtin: false,
            security_level: None,
            data_access: vec![],
            mcp_tools: vec![],
            bundle_version: None,
        };
        let json = serde_json::to_value(&normal).unwrap();
        assert_eq!(json["builtin"], false);
        for key in [
            "security_level",
            "data_access",
            "mcp_tools",
            "bundle_version",
        ] {
            assert!(json.get(key).is_none(), "空值字段 {key} 应省略: {json}");
        }

        let builtin = MarketplaceToolInfo {
            id: "session-reader".into(),
            builtin: true,
            security_level: Some("L0".into()),
            data_access: vec!["sessions.read".into()],
            mcp_tools: vec!["mcp_session-reader_read_session".into()],
            bundle_version: Some("0.32-test".into()),
            ..normal
        };
        let json = serde_json::to_value(&builtin).unwrap();
        assert_eq!(json["builtin"], true);
        assert_eq!(json["security_level"], "L0");
        assert_eq!(json["data_access"], serde_json::json!(["sessions.read"]));
        assert_eq!(json["bundle_version"], "0.32-test");
    }
}
