//! 能力档案（Capability Profile）：per-mode 声明式能力配置。
//!
//! 把「哪个模式能用什么能力」从散落的 if-else / 编译期常量收敛为**一份档案、
//! 一个解析器（[`SessionPolicy::resolve`]）、三个生效通道**（skills_dir 组合
//! 目录 / disallowed_tools / hidden_tools）。设计全程见
//! `.luzeyang/capability-unified/`（00-README / 02-实施交接 / 03-留档）。
//!
//! v1 语义：
//!   - **编译内嵌 JSON，不写用户数据**（规避版本迁移；"运行期不变"是 v1
//!     语义，未来可编辑化需重议 respawn）；
//!   - 档案 = **基础集 + 差量**（base + exclude/include）：上游新增工具仍被
//!     基础集（底座隐藏常量）挡住，模式只表达差异；
//!   - 用户运行时开关（disabled_connectors / disabled_skills）与档案叠加：
//!     档案是设计期默认，用户开关覆盖。

use std::sync::OnceLock;

use serde::Deserialize;

use crate::core::session_mode::SessionMode;

/// 档案文件（编译内嵌；与 bundle 运行时资源无关，纯设计期配置）。
const PROFILES_JSON: &str = include_str!("../../../resources/common/capability-profiles.json");

/// 技能线档案：设计期默认的技能排除与项目级 skills 开关。
/// v1：`exclude` 空（技能开关仍由用户 `disabled_skills.json` 双 scope 持久化
/// 驱动）；`include_project` 与用户开关（`skill_scope::project_skills_enabled`）
/// 叠加，v1 默认 false。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillsProfile {
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub include_project: bool,
}

/// 工具线档案：base + 差量。
/// - `base`：继承的基础集（"default" = 底座隐藏常量的补集，即当前全部会话
///   实际可见集）；
/// - `exclude`：在基础集上再藏（走 disallowed_tools 通道，spawn 初值 + 热刷，
///   下轮生效）；
/// - `include`：从基础集之外放出（走 EngineConfig.hidden_tools 注入通道，
///   fork ②，respawn 生效——hidden 集 = 常量 − include）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolsProfile {
    #[serde(default = "default_base_name")]
    pub base: String,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
}

fn default_base_name() -> String {
    "default".to_string()
}

/// 连接器线档案：设计期 scope 默认（v1 空——scope 选择仍由
/// [`SessionPolicy::connector_scope`] 的 mode 映射承担，用户开关覆盖）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectorsProfile {
    #[serde(default)]
    pub scope_defaults: std::collections::HashMap<String, bool>,
}

/// 单模式能力档案。
#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityProfile {
    /// 档案所属模式（"plain" / "code"）。
    pub mode: String,
    #[serde(default)]
    pub skills: SkillsProfile,
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
    let wanted = match mode {
        SessionMode::Plain => "plain",
        SessionMode::Code => "code",
    };
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
        // v1 差量语义：exclude 空（无设计期再藏），include 只含已评估放出的工具
        assert!(plain.tools.exclude.is_empty());
        assert!(plain.tools.include.is_empty(), "plain 不得放出任何工具");
        assert!(code.tools.exclude.is_empty());
        for name in &code.tools.include {
            assert!(
                !name.is_empty() && !name.contains(' '),
                "include 必须是合法工具名: {name}"
            );
        }
    }

    #[test]
    fn skills_profile_defaults_closed() {
        // 项目级 skills 默认关（prompt-injection 面，显式开启才扫描）
        assert!(!profile_for(SessionMode::Code).skills.include_project);
        assert!(profile_for(SessionMode::Code).skills.exclude.is_empty());
    }
}
