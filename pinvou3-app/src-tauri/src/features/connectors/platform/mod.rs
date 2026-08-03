//! 连接器按需安装的平台适配：目标 lock、可执行文件命名和权限。

use std::path::Path;

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../resources/platforms/linux/aarch64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../resources/platforms/linux/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../resources/platforms/macos/aarch64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../resources/platforms/macos/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const LOCK_JSON: &str = include_str!(
    "../../../../resources/platforms/windows/x86_64/bundle/connectors/connectors.lock.json"
);
#[cfg(not(any(
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "x86_64"),
)))]
const LOCK_JSON: &str = "";

pub fn lock_json() -> &'static str {
    LOCK_JSON
}

pub fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
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
