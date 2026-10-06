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

// `pub` re-exports (the standard escape hatch for exporting a `pub` item
// from a crate-private module), lifted across the crate boundary by the
// `pub use` in features::mod: the CLI's ensure-cli lane appends to the
// shared install log and reads the acceleration-prefix env name.
pub use connector_cli::{append_cli_install_log, rotate_cli_install_log_if_oversized};
pub use native_installer::GITHUB_ASSET_MIRROR_PREFIX_ENV;
// Round-40 review: the CLI's ensure-cli lane writes the same license
// side-files the GUI installer does (identical texts, identical location).
pub use native_installer::write_managed_license;
