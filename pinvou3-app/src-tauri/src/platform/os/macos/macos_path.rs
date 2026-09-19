use std::path::PathBuf;

// Unix 通用 helper 从 posix.rs 继承（与 linux/linux_path.rs 同一套去重）：
// 连接器 CLI 解析、npm prefix、路径身份 key 等语义一致实现收口于 posix.rs。
pub use super::super::posix::{
    apply_user_npm_prefix, connector_cli_command, filesystem_path_identity_key,
    platform_compat_path,
};
// `user_home_dir` 不在此重复定义：与 unsupported.rs 的实现逐字相同（HOME 缺失
// 回退 std::env::temp_dir()），经 macos/mod.rs 的 `pub use super::unsupported::*`
// glob 继承。

pub fn pandoc_tool_path() -> PathBuf {
    PathBuf::from("pandoc")
}
