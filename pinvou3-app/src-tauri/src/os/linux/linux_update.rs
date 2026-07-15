use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::time::timeout;

use crate::bridge::paths;

const UPDATE_MANIFEST_URL: &str = "https://pinvou.com/pinvou3/latest.json";

fn manifest_url() -> String {
    std::env::var("PINVOU3_UPDATE_URL").unwrap_or_else(|_| UPDATE_MANIFEST_URL.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatestManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

pub fn check_update_platform_support() -> Result<(), String> {
    Ok(())
}

pub async fn check_for_update_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    let m: LatestManifest = client
        .get(manifest_url())
        .send()
        .await
        .map_err(|e| format!("更新源连接失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("更新源响应异常: {e}"))?
        .json()
        .await
        .map_err(|e| format!("latest.json 解析失败: {e}"))?;
    Ok(crate::updater::UpdateInfo {
        // 仅严格大于才提示 → 服务器版本 ≤ 本地时天然降级保护
        available: is_newer(&m.version, current_version),
        current_version: current_version.to_string(),
        latest_version: m.version,
        notes: m.notes,
        pub_date: m.pub_date,
        url: m.url,
        sha256: m.sha256.to_lowercase(),
        size: m.size,
        package_md5: String::new(),
        software_id: String::new(),
        sn: String::new(),
        update_type: String::new(),
        platform: "linux".to_string(),
        ota_host: String::new(),
    })
}

/// Debian 架构名（deb 包文件名后缀），与 dpkg --print-architecture 一致。
fn deb_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    }
}

pub async fn download_update_package(
    info: &crate::updater::UpdateInfo,
    app: AppHandle,
    cancel: &AtomicBool,
    stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    check_update_platform_support()?;
    let dir = paths::updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败: {e}"))?;
    let dest = dir.join(format!("pinvou3_{}_{}.deb", info.latest_version, deb_arch()));
    let expected = info.sha256.to_lowercase();

    if dest.exists() && file_sha256(&dest).as_deref() == Some(expected.as_str()) {
        return Ok(crate::updater::DownloadUpdateResult::Path(
            dest.to_string_lossy().into_owned(),
        ));
    }

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "deb") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    if info.size > 0 {
        if let Some(avail) = available_bytes(&dir) {
            let need = info.size.saturating_add(64 * 1024 * 1024);
            if avail < need {
                return Err(format!(
                    "磁盘空间不足：需约 {} MB，当前可用 {} MB",
                    need / 1_048_576,
                    avail / 1_048_576
                ));
            }
        }
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let mut resp = client
        .get(&info.url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载响应异常: {e}"))?;

    let total = if info.size > 0 {
        info.size
    } else {
        resp.content_length().unwrap_or(0)
    };
    let mut file = std::fs::File::create(&dest).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    cancel.store(false, Ordering::SeqCst);
    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(&dest);
            return Err("已取消下载".to_string());
        }
        let chunk = match timeout(stall_timeout, resp.chunk()).await {
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&dest);
                return Err(format!(
                    "下载停滞：超过 {}s 无数据，已中断（网络异常或更新源无响应）",
                    stall_timeout.as_secs()
                ));
            }
            Ok(Err(e)) => return Err(format!("下载中断: {e}")),
            Ok(Ok(None)) => break,
            Ok(Ok(Some(c))) => c,
        };
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 262_144 || downloaded == total {
            last_emit = downloaded;
            let _ = app.emit(
                "update:progress",
                serde_json::json!({ "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(&dest);
        return Err(format!(
            "sha256 校验失败（期望 {expected} 实际 {actual}），已删除下载文件"
        ));
    }
    Ok(crate::updater::DownloadUpdateResult::Path(
        dest.to_string_lossy().into_owned(),
    ))
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

pub fn install_downloaded_update(
    deb_path: Option<String>,
    _installer_path: Option<String>,
    _info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    let deb_path = deb_path.ok_or_else(|| "缺少 deb 安装包路径".to_string())?;
    install_update_package(Path::new(&deb_path))?;
    Ok(false)
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: false,
        reported: false,
        result: String::new(),
        message: "当前平台没有待反馈升级结果".to_string(),
    })
}

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

fn available_bytes(dir: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["--output=avail", "-B1"])
        .arg(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .nth(1)?
        .trim()
        .parse()
        .ok()
}

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().trim_start_matches('v').splitn(3, '.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
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
    fn semver_numeric_not_lexicographic() {
        assert!(is_newer("0.10.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "0.2.0"));
    }

    #[test]
    fn semver_equal_or_lower_not_newer() {
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn semver_malformed_not_newer() {
        assert!(!is_newer("abc", "0.2.0"));
        assert!(!is_newer("1.2", "0.2.0"));
        assert!(!is_newer("9.9.9", "garbage"));
    }

    #[test]
    fn semver_tolerates_v_prefix_and_spaces() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer(" 0.3.0 ", "0.2.0"));
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let json = r#"{
            "version": "0.3.0",
            "notes": "测试",
            "pub_date": "2026-06-10T08:00:00Z",
            "url": "https://pinvou.com/pinvou3/pinvou3_0.3.0_amd64.deb",
            "sha256": "abc123",
            "size": 1024
        }"#;
        let m: LatestManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.version, "0.3.0");
        assert_eq!(m.size, 1024);
        let back = serde_json::to_string(&m).unwrap();
        let m2: LatestManifest = serde_json::from_str(&back).unwrap();
        assert_eq!(m2.url, m.url);
    }

    #[test]
    fn manifest_optional_fields_default() {
        let m: LatestManifest =
            serde_json::from_str(r#"{"version":"0.3.0","url":"u","sha256":"s"}"#).unwrap();
        assert_eq!(m.notes, "");
        assert_eq!(m.size, 0);
    }

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
