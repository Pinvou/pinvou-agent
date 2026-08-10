//! 产品模式轴：plain（work，沙箱执行根）与 code（真实项目目录绑定）。
//!
//! 与运行时轴（原生/ACP）正交。上移到 core：store（持久化）、bridge 的
//! `SessionPolicy`（行为策略）等多个 feature 共用同一类型，方向保持
//! app → features → platform/core。

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

impl SessionMode {
    pub fn is_code(self) -> bool {
        matches!(self, Self::Code)
    }
    pub fn is_plain(&self) -> bool {
        matches!(self, Self::Plain)
    }
    /// 档案条目的模式名（capability-profiles.json 的 `mode` 字段值）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Code => "code",
        }
    }
}
