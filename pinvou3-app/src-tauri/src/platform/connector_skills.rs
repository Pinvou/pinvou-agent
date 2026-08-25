//! 连接器配套技能目录名 —— 跨 feature 单一真相源。
//!
//! 企微技能表同时被两处消费：`features::runtime_bundle` 的解包门控
//! （`apply_wecom_skills`，写/删 `bundles/wecom/skills/`）与 `features::marketplace`
//! 的注册表（wecom 卡 skills 列表）、扁平布局迁移（`migrate_flat_skills_layout`）、
//! `cli_bundle_of_skill` 反查与 `legacy_cli_records` 首启登记。两个 feature 互相
//! 引用会被架构守卫判为循环依赖，故表下沉到 platform 层共用
//! （五轮评审必修 3：marketplace 侧曾按 0.1.9 旧表写死 7 技能，与门控侧的
//! 1.1.0 新表分叉，污染上述全部消费面）。

/// 14 个企微域技能目录名(门控写 / 删与 marketplace 各消费面共用)。wecom-cli 1.1.0
/// 起上游按服务模型重排(`msg`→`message`、`schedule`→`calendar`,新增 disk/
/// doc-manage/email/media/sheet/smartpage/shared),本地结构跟随上游,不再维持
/// 0.1.9 时代的「sheet/smartpage 并入 doc」合并形态。
pub const WECOM_SKILL_DIRS: [&str; 14] = [
    "wecomcli-calendar",
    "wecomcli-contact",
    "wecomcli-disk",
    "wecomcli-doc",
    "wecomcli-doc-manage",
    "wecomcli-email",
    "wecomcli-media",
    "wecomcli-meeting",
    "wecomcli-message",
    "wecomcli-shared",
    "wecomcli-sheet",
    "wecomcli-smartpage",
    "wecomcli-smartsheet",
    "wecomcli-todo",
];

/// 0.1.9 时代的旧技能目录名:1.1.0 重排后已不存在于包内,但存量用户解包目录里
/// 可能残留,继续加载会教模型已死的命令(`msg`/`schedule` 服务)。runtime_bundle
/// 每次门控时清理旧扁平目录;marketplace 扁平布局迁移遇到这两个名走删除而非
/// 搬移（搬进 `bundles/wecom/skills/` 后门控清理够不到，会永久残留）。
pub const WECOM_LEGACY_SKILL_DIRS: [&str; 2] = ["wecomcli-msg", "wecomcli-schedule"];
