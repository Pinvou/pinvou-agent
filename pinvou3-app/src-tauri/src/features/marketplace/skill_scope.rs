//! 技能开关 scope 持久化的**兼容别名层**。
//!
//! 原 `disabled_skills.json`（技能 id × SessionMode）已随 scope 收敛（todo A 节）
//! 合并进 `scope.rs` 的单一 `disabled_bundles.json`（包 id × SessionMode）。本模块
//! 保留旧技能开关调用方的路径：全部 re-export 到统一实现，入参/出参统一归一为
//! **包 id**（技能 id 经 `bundle::skill_owner_package` 映射到所属包，companion → MCP/CLI
//! 包，独立技能 → 自身）。`skill:` 前缀跨文件借道已在此收敛中清除。
//!
//! 依赖方向不变：本模块是 marketplace 领域内别名层，不反向依赖 assistant 运行时。

pub use crate::features::marketplace::scope::{
    load_disabled_bundles as load_disabled_skills,
    load_disabled_bundles_for as load_disabled_skills_for, project_skills_enabled,
    remove_bundle_from_disabled_scopes as remove_skill_from_disabled_scopes,
    save_disabled_bundles_for as save_disabled_skills_for, set_project_skills_enabled,
    sync_deny_all_scopes_after_install as sync_deny_all_scopes_after_skill_install,
};
