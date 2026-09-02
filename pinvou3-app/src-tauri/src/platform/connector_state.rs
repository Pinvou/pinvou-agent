//! Persistent connector marker files shared by bundle staging and connector UI.

use std::path::PathBuf;

use super::paths;

fn marker(name: &str) -> PathBuf {
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

/// 按连接器 id 取技能可见性（布局迁移等需要按 id 分派的场景）。
pub fn skills_visible_for(id: &str) -> bool {
    match id {
        "feishu" => feishu_skills_visible(),
        "wecom" => wecom_skills_visible(),
        "dingtalk" => dingtalk_skills_visible(),
        "tmeet" => tmeet_skills_visible(),
        _ => false,
    }
}
