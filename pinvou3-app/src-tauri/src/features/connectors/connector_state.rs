//! 连接器技能可见性标记文件。
//!
//! Wave 3 从 `platform/connector_state.rs` 迁入——这些标记文件是连接器域
//! 概念（feishu/wecom/dingtalk/tmeet 技能 gate），不是跨功能平台原语。
//! 唯一消费者：`features/runtime_bundle/platform/extraction.rs`。

use crate::platform::paths;

fn marker(name: &str) -> std::path::PathBuf {
    paths::pinvou3_home().join(name)
}

pub fn feishu_skills_visible() -> bool {
    !marker("feishu_disabled").is_file()
}

pub fn wecom_skills_visible() -> bool {
    !marker("wecom_disabled").is_file()
}

pub fn dingtalk_skills_visible() -> bool {
    !marker("dingtalk_disabled").is_file()
}

pub fn tmeet_skills_visible() -> bool {
    !marker("tmeet_disabled").is_file()
}
