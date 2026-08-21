pub(crate) mod connector_cli;
pub(crate) mod dingtalk;
pub(crate) mod feishu;
pub(crate) mod ima;
// pub(crate)：bridge 启动路径调用 migrate_legacy_cli_binaries（旧布局迁移接线）
pub(crate) mod native_installer;
mod platform;
pub(crate) mod skill_gate;
pub(crate) mod tmeet;
pub(crate) mod wecom;
pub(crate) mod weibo;
