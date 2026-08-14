//! 产品模式轴：plain（work，沙箱执行根）与 code（真实项目目录绑定）。
//!
//! 与运行时轴（原生/ACP）正交。上移到 core：store（持久化）、bridge 的
//! `SessionPolicy`（行为策略）等多个 feature 共用同一类型，方向保持
//! app → features → platform/core。模式身份还包含能力开关的包默认策略
//! （[`PackDefaultPolicy`]，见下方注释为什么放 core）。

use serde::{Deserialize, Serialize};

/// 产品模式轴：plain（work，沙箱执行根）与 code（真实项目目录绑定）。
/// 与运行时轴 `AgentBackend`（Deepseek=原生、其余=ACP）正交。
/// 持久化保持原 `code_session` 键与布尔格式（见 codex_acp store 的
/// `session_mode_serde`），新旧版本读写 `session-agents.json` 完全兼容。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    #[default]
    Plain,
    Code,
}

/// 该模式未初始化能力开关时的包默认策略（连接器/技能市场条目）。
///
/// 这是**模式身份**而非用户偏好：DenyAll = 外部能力是 prompt-injection 面，
/// 该模式的会话默认禁用全部已装条目、由用户显式开启（安全姿态）；AllowAll =
/// 默认全开。策略放 core（不放 assistant/marketplace）：marketplace 的 load
/// 路径需要它兜底，而 assistant 已依赖 marketplace，放 assistant 会形成
/// feature 依赖环（见 marketplace/skill_scope.rs 头注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackDefaultPolicy {
    AllowAll,
    DenyAll,
}

impl SessionMode {
    /// 全部已注册模式。静态表/泛化遍历（MODE_TABLE、DenyAll 同步钩子）以此为准，
    /// 新增模式漏挂时由穷尽性测试兜底。
    pub const ALL: &[SessionMode] = &[SessionMode::Plain, SessionMode::Code];

    pub fn is_code(self) -> bool {
        matches!(self, Self::Code)
    }
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }

    /// kebab-case 模式名，与 serde 序列化一致——持久化键（disabled_* 开关文件
    /// 的 scope 键）与前端协议字符串都以此为单一真源。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Code => "code",
        }
    }

    /// 反解前端/落盘的 scope 字符串；未知名称返回 None（调用方决定报错或回退）。
    pub fn from_scope_str(scope: &str) -> Option<SessionMode> {
        match scope {
            "plain" => Some(Self::Plain),
            "code" => Some(Self::Code),
            _ => None,
        }
    }

    /// 该模式能力开关未初始化时的包默认策略（见 [`PackDefaultPolicy`]）。
    /// plain 默认全开，code 默认全禁已装条目（外部能力显式开启）。
    pub fn pack_default_policy(self) -> PackDefaultPolicy {
        match self {
            Self::Plain => PackDefaultPolicy::AllowAll,
            Self::Code => PackDefaultPolicy::DenyAll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// as_str 必须与 serde 序列化结果逐字节一致：落盘键与前端协议都走这个名字，
    /// 两边漂移会静默写出一组永远读不到的 scope 键。
    #[test]
    fn as_str_matches_serde_serialization() {
        for mode in SessionMode::ALL {
            let serialized = serde_json::to_string(mode).unwrap();
            assert_eq!(serialized, format!("\"{}\"", mode.as_str()), "{mode:?}");
            assert_eq!(SessionMode::from_scope_str(mode.as_str()), Some(*mode));
        }
    }

    /// ALL 必须覆盖全部变体（新增模式漏挂 ALL → 表驱动遍历静默漏掉该模式）。
    #[test]
    fn all_covers_every_variant() {
        assert_eq!(SessionMode::ALL.len(), 2);
        assert!(SessionMode::ALL.contains(&SessionMode::Plain));
        assert!(SessionMode::ALL.contains(&SessionMode::Code));
    }

    #[test]
    fn from_scope_str_rejects_unknown() {
        assert_eq!(SessionMode::from_scope_str("cdoe"), None);
        assert_eq!(SessionMode::from_scope_str("CODE"), None);
        assert_eq!(SessionMode::from_scope_str(""), None);
    }

    #[test]
    fn pack_default_policy_per_mode() {
        assert_eq!(
            SessionMode::Plain.pack_default_policy(),
            PackDefaultPolicy::AllowAll
        );
        assert_eq!(
            SessionMode::Code.pack_default_policy(),
            PackDefaultPolicy::DenyAll
        );
    }
}
