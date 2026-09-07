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

/// 声明全部已注册模式（单一真源）：展开为 `ALL` 常量 + 编译期穷尽哨兵。
/// 哨兵是无通配臂的 match——新增枚举变体而漏挂此列表时直接编译失败，
/// 兜底不依赖遍历 ALL 的测试（那对 ALL 本身漏项是自指的）。
macro_rules! declare_all_modes {
    ($($variant:ident),+ $(,)?) => {
        /// 全部已注册模式。静态表/泛化遍历（MODE_TABLE、DenyAll 同步钩子）以此为准，
        /// 编译期穷尽哨兵保证与枚举变体同步（见 `declare_all_modes`）。
        pub const ALL: &[SessionMode] = &[$(SessionMode::$variant),+];

        /// 编译期穷尽哨兵（无通配臂）：仅由 `declare_all_modes` 展开，不直接调用。
        #[allow(dead_code)]
        fn exhaustive_mode_guard(mode: SessionMode) {
            match mode {
                $(SessionMode::$variant => {})+
            }
        }
    };
}

impl SessionMode {
    declare_all_modes!(Plain, Code);

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
    /// 全部模式默认全禁已装条目（外部能力一律显式开启；plain 于工具开关收敛
    /// 版本从 AllowAll 翻为 DenyAll，存量用户由 scope.rs 的读时迁移播种
    /// 「保持原开关状态」，新装包不再默认进入任何会话）。
    pub fn pack_default_policy(self) -> PackDefaultPolicy {
        match self {
            Self::Plain => PackDefaultPolicy::DenyAll,
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

    #[test]
    fn from_scope_str_rejects_unknown() {
        assert_eq!(SessionMode::from_scope_str("cdoe"), None);
        assert_eq!(SessionMode::from_scope_str("CODE"), None);
        assert_eq!(SessionMode::from_scope_str(""), None);
    }

    #[test]
    fn pack_default_policy_per_mode() {
        // 全部模式 DenyAll：外部能力默认全禁、显式开启（plain 的存量体验由
        // scope.rs 读时迁移兜底，见 load_disabled_bundles_file_locked）。
        for mode in SessionMode::ALL {
            assert_eq!(
                mode.pack_default_policy(),
                PackDefaultPolicy::DenyAll,
                "{mode:?} 必须默认全禁"
            );
        }
    }
}
