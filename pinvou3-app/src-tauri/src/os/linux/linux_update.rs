use std::path::{Path, PathBuf};
use std::process::Command;

pub fn check_update_platform_support() -> Result<(), String> {
    Ok(())
}

pub fn install_update_package(path: &Path) -> Result<(), String> {
    let canon = validate_deb_path(path)?;
    let script = format!(
        "DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall '{}'",
        canon.display()
    );
    let output = Command::new("pkexec")
        .args(["sh", "-c", &script])
        .output()
        .map_err(|e| format!("pkexec 启动失败: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match code {
        126 => "用户取消授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("安装失败 (exit {code}): {}", tail.join(" / "))
        }
    })
}

fn validate_deb_path(path: &Path) -> Result<PathBuf, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("deb 文件不存在: {e}"))?;
    let dir = crate::bridge::paths::updates_dir()
        .canonicalize()
        .map_err(|e| format!("更新目录不存在: {e}"))?;
    if !canon.starts_with(&dir) {
        return Err("非法路径：deb 必须在更新下载目录内".to_string());
    }
    if canon.extension().is_none_or(|x| x != "deb") {
        return Err("非法路径：只接受 .deb 文件".to_string());
    }
    if canon.to_string_lossy().contains('\'') {
        return Err("非法路径：含引号".to_string());
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_deb_path_whitelist() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root = std::env::temp_dir().join("pinvou3-updater-test");
        std::env::set_var("PINVOU3_HOME", &root);

        let updates = crate::bridge::paths::updates_dir();
        std::fs::create_dir_all(&updates).unwrap();
        let good = updates.join("pinvou3_9.9.9_amd64.deb");
        std::fs::write(&good, b"fake").unwrap();
        assert!(validate_deb_path(&good).is_ok());

        let outside = root.join("evil.deb");
        std::fs::write(&outside, b"fake").unwrap();
        assert!(validate_deb_path(&outside).is_err());

        let txt = updates.join("note.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(validate_deb_path(&txt).is_err());

        assert!(validate_deb_path(&updates.join("ghost.deb")).is_err());

        let traversal = updates.join("../evil.deb");
        assert!(validate_deb_path(&traversal).is_err());

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
