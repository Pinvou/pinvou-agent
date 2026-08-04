//! 重新导出垫片——类型定义已迁至 `features::sessions::mode_state`。
//!
//! `SessionModeState` / `ActiveSkillBinding` / `SerializableMode` 是 session 域聚合
//! （persona/review/workflow/knowledge 四特性字段），违反 `core/README.md` 的
//! "feature 内部类型不入 core"准则。Wave 3 将类型定义迁回 `features::sessions`，
//! 此文件保留重新导出以维持外部 `crate::core::mode_state::{...}` import 兼容。

pub use crate::features::sessions::mode_state::{
    ActiveSkillBinding, MountedCollection, MountedCollectionsSnapshot, SerializableMode,
    SessionModeState,
};
