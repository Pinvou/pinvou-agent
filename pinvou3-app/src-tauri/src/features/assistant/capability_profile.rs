//! 能力档案（Capability Profile）：per-mode 声明式能力配置。
//!
//! 把「哪个模式能用什么能力」从散落的 if-else / 编译期常量收敛为**一份档案、
//! 一个解析器（[`SessionPolicy::resolve`]）、一个生效通道**（disallowed_tools）。
//! 技能线不做设计期差量：技能可见性由运行时双 scope 开关 +
//! 组合目录治理（`features/assistant/skill_materialization.rs`）闭环。
//!
//! v1 语义：
//!   - **编译内嵌 JSON，不写用户数据**（规避版本迁移；"运行期不变"是 v1
//!     语义，未来可编辑化需重议 respawn）；
//!   - 档案 = **基础集 + 差量**：基础集由底座默认工具面承担，档案只声明差量
//!     （exclude / extra_hidden）——上游新增工具仍受 `allowed_tools` 白名单约束，
//!     模式只表达差异；
//!   - 用户运行时开关（disabled_connectors / disabled_skills）与档案叠加：
//!     档案是设计期默认，用户开关覆盖。

use std::sync::OnceLock;

use serde::Deserialize;

use crate::core::session_mode::SessionMode;
use crate::features::marketplace::ConnectorScope;

/// 档案文件（编译内嵌；与 bundle 运行时资源无关，纯设计期配置）。
const PROFILES_JSON: &str = include_str!("../../../resources/common/capability-profiles.json");

/// 工具线档案：差量 + 模式固有隐藏。
/// - `exclude`：在基础集上再藏（走 disallowed_tools 通道，spawn 初值 + 热刷，
///   下轮生效）——"该模式还**不想要**什么"（可变策略，可被用户开关覆盖）；
/// - `extra_hidden`：模式固有隐藏（并入 disallowed_tools 通道）——"该模式
///   **不可能有**什么"（模式身份的一部分，恒定不可被用户开关覆盖）。
/// 基础集语义由底座默认工具面（`allowed_tools` 白名单）承担，不再用 JSON 声明。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolsProfile {
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub extra_hidden: Vec<String>,
}

/// 连接器线档案：该模式的连接器 scope（决定禁用集取哪个 scope）。
/// 缺省回退 Plain（与 `SessionMode` 缺省一致）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectorsProfile {
    #[serde(default = "default_connector_scope")]
    pub scope: ConnectorScope,
}

fn default_connector_scope() -> ConnectorScope {
    ConnectorScope::Plain
}

/// 单模式能力档案。
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityProfile {
    /// 档案所属模式（"plain" / "code"）。
    pub mode: String,
    #[serde(default)]
    pub tools: ToolsProfile,
    #[serde(default)]
    pub connectors: ConnectorsProfile,
}

#[derive(Debug, Clone, Deserialize)]
struct ProfilesFile {
    profiles: Vec<CapabilityProfile>,
}

fn profiles() -> &'static Vec<CapabilityProfile> {
    static PROFILES: OnceLock<Vec<CapabilityProfile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        let file: ProfilesFile = serde_json::from_str(PROFILES_JSON).unwrap_or_else(|e| {
            // 档案是编译内嵌的受控资源：解析失败属打包错误，崩溃优于静默降级。
            panic!("capability-profiles.json 解析失败（打包错误）: {e}");
        });
        file.profiles
    })
}

/// 该模式的能力档案。缺省回退 plain 档案（与 `SessionPolicy` 缺省 Plain 一致）。
pub fn profile_for(mode: SessionMode) -> &'static CapabilityProfile {
    let wanted = mode.as_str();
    profiles()
        .iter()
        .find(|p| p.mode == wanted)
        .unwrap_or_else(|| {
            profiles()
                .iter()
                .find(|p| p.mode == "plain")
                .expect("能力档案必须含 plain 条目")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_parse_and_lookup() {
        let plain = profile_for(SessionMode::Plain);
        assert_eq!(plain.mode, "plain");
        let code = profile_for(SessionMode::Code);
        assert_eq!(code.mode, "code");
        // Git 是代码会话的结构化能力；普通工作会话保持原有工具面。
        assert_eq!(plain.tools.exclude, ["Git"]);
        assert!(code.tools.exclude.is_empty());
        for name in &code.tools.extra_hidden {
            assert!(
                !name.is_empty() && !name.contains(' '),
                "extra_hidden 必须是合法工具名: {name}"
            );
        }
    }
}
