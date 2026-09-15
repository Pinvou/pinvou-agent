//! 连接器按需安装的平台适配：目标 lock、可执行文件命名和权限。
//!
//! lock 表与可执行文件命名已下沉为跨功能原语 `crate::platform::connector_lock`
//! （marketplace 首启导入也要对照 lock 表验存量二进制）；此处保留委托，
//! 既有调用方（native_installer）零改动。

use std::path::Path;

pub fn lock_json() -> &'static str {
    crate::platform::connector_lock::lock_json()
}

pub fn executable_name(name: &str) -> String {
    crate::platform::connector_lock::executable_name(name)
}

pub fn archive_member(name: &str) -> &'static str {
    match name {
        "dws" => {
            if cfg!(windows) {
                "dws.exe"
            } else {
                "dws"
            }
        }
        "lark-cli" => {
            if cfg!(windows) {
                "lark-cli.exe"
            } else {
                "lark-cli"
            }
        }
        "wecom-cli" => {
            if cfg!(windows) {
                "package/bin/wecom-cli.exe"
            } else {
                "package/bin/wecom-cli"
            }
        }
        _ => "",
    }
}

pub fn set_executable_permissions(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
