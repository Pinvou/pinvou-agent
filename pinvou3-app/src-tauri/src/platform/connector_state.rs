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

pub fn eip_skills_visible() -> bool {
    marker("eip").join("connected.flag").is_file()
}

pub fn zhidao_skills_visible() -> bool {
    marker("zhidao").join("connected.flag").is_file()
}
