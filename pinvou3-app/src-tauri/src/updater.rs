//! 应用内升级：检查 latest.json → 下载 deb（sha256 校验）→ pkexec apt 安装 → 重启。
//!
//! 设计要点：
//! - **更新源是静态 HTTP**：服务器只托管 `latest.json` + deb 文件，零服务端逻辑。
//!   Tauri 官方 updater plugin 不支持 deb，所以这里自建轻量机制。
//! - **URL 不进 settings.json**：更新源是基础设施不是用户偏好，可改会成攻击面。
//!   `PINVOU3_UPDATE_URL` env 可覆盖（e2e 测试指本地 http server）。
//! - **下载目录用 `~/.pinvou3/updates/`** 而非 /tmp：tmpfs 受内存限制且重启清空。
//! - **安装走 pkexec**（弹系统密码框），与 super_permission.rs 同套路；但这里用
//!   `.output()` 捕 stderr 透传 apt 真实报错（lock 占用 / 磁盘不足等）。
//! - **inode 语义**：apt 替换 `/usr/bin/pinvou3` 后老进程持旧 inode 继续跑无害，
//!   `app.restart()` 按路径 exec 才换到新版。所以装完不强制重启，前端给按钮。

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

const UPDATE_MANIFEST_URL: &str = "https://www.ma-xiao.com/pinvou3/latest.json";

/// 下载停滞看门狗阈值：连上后单次等待数据超过此时长即判定挂死。
/// 用「单 chunk 间隔」而非「总耗时」做超时——慢网持续小流量不会被误杀，
/// 只有真正长时间收不到任何字节（更新源挂起 / 半开连接）才中断。
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 下载取消标志。前端 `cancel_download` 置位，下载循环每轮检查一次。
/// 进程级单例：同一时刻只允许一个下载在跑（前端 updateDownloading gate 保证）。
static DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

fn manifest_url() -> String {
    std::env::var("PINVOU3_UPDATE_URL").unwrap_or_else(|_| UPDATE_MANIFEST_URL.to_string())
}

/// 服务器上的 latest.json（由 scripts/release-deb.sh 生成上传）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestManifest {
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

/// check_for_update 的返回值；前端原样传回 download_update。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub available: bool,
    pub current_version: String,
    pub latest_version: String,
    pub notes: String,
    pub pub_date: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// 拉 latest.json 与当前版本比较。网络失败返回 Err——启动静默检查由前端吞掉，
/// 手动检查才展示错误。
#[tauri::command]
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
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
    let current = env!("CARGO_PKG_VERSION");
    Ok(UpdateInfo {
        // 仅严格大于才提示 → 服务器版本 ≤ 本地时天然降级保护
        available: is_newer(&m.version, current),
        current_version: current.to_string(),
        latest_version: m.version,
        notes: m.notes,
        pub_date: m.pub_date,
        url: m.url,
        sha256: m.sha256.to_lowercase(),
        size: m.size,
    })
}

/// 下载 deb 到 `~/.pinvou3/updates/`，流式写盘 + 边下边算 sha256，
/// 进度走 `update:progress` 事件。返回 deb 绝对路径。
#[tauri::command]
pub async fn download_update(info: UpdateInfo, app: AppHandle) -> Result<String, String> {
    let dir = paths::updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败: {e}"))?;
    let dest = dir.join(format!("pinvou3_{}_amd64.deb", info.latest_version));
    let expected = info.sha256.to_lowercase();

    // 同版本已下载且校验通过 → 跳过重复下载
    if dest.exists() && file_sha256(&dest).as_deref() == Some(expected.as_str()) {
        return Ok(dest.to_string_lossy().into_owned());
    }

    // 清掉历史 deb 防堆积（目录里只留本次要装的）
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "deb") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    // 磁盘空间预检：下载前比对 info.size 与目标盘可用空间，不足提前报错（而非下到
    // 一半 ENOSPC）。留 64MB 余量给 apt 解包安装；取不到可用空间则跳过预检不误报。
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

    // 连接超时单设 10s；但**不设总超时**——deb 几十 MB，总超时会误杀慢网。
    // 挂死场景改由下面的 stall 看门狗(单 chunk 间隔超时)覆盖。
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
    // 新一轮下载，清掉上次残留的取消标志
    DOWNLOAD_CANCEL.store(false, Ordering::SeqCst);
    loop {
        // 取消优先：正常下载中点取消，下一个 chunk 到达即退出（挂死由 stall 超时兜底）
        if DOWNLOAD_CANCEL.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(&dest);
            return Err("已取消下载".to_string());
        }
        // stall 看门狗：单次等数据超时即判定挂死，区别于慢网持续小流量
        let chunk = match timeout(DOWNLOAD_STALL_TIMEOUT, resp.chunk()).await {
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&dest);
                return Err(format!(
                    "下载停滞：超过 {}s 无数据，已中断（网络异常或更新源无响应）",
                    DOWNLOAD_STALL_TIMEOUT.as_secs()
                ));
            }
            Ok(Err(e)) => return Err(format!("下载中断: {e}")),
            Ok(Ok(None)) => break, // 流正常结束
            Ok(Ok(Some(c))) => c,
        };
        file.write_all(&chunk).map_err(|e| format!("写盘失败: {e}"))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        // 每 256KB 发一次进度，避免事件风暴
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
    Ok(dest.to_string_lossy().into_owned())
}

/// pkexec 提权 apt 安装下载好的 deb。成功后**不自动重启**，前端展示「重启」按钮。
#[tauri::command]
pub async fn install_update(deb_path: String) -> Result<(), String> {
    let canon = validate_deb_path(Path::new(&deb_path))?;
    // DEBIAN_FRONTEND 防 dpkg conffile 冲突卡交互；--reinstall 容错同版本重装。
    // 路径已白名单校验（~/.pinvou3/updates/ 下 .deb 且无引号），注入面可控。
    let script = format!(
        "DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall '{}'",
        canon.display()
    );
    // pkexec 等用户输密码可能很久，放 blocking 线程别占 async runtime
    let output = tokio::task::spawn_blocking(move || {
        Command::new("pkexec").args(["sh", "-c", &script]).output()
    })
    .await
    .map_err(|e| format!("安装任务失败: {e}"))?
    .map_err(|e| format!("pkexec 启动失败: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(match code {
        126 => "用户取消授权".to_string(),
        127 => "未授权或 pkexec 不可用".to_string(),
        _ => {
            // 透传 apt stderr 末尾几行：lock 占用 / 磁盘不足 / 依赖缺失等真实原因
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(4).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("安装失败 (exit {code}): {}", tail.join(" / "))
        }
    })
}

/// 重启应用使新版本生效（exec 新 inode）。
#[tauri::command]
pub async fn restart_app(app: AppHandle) -> Result<(), String> {
    app.restart();
}

/// 置位取消标志，让正在跑的 `download_update` 循环下一轮自行退出并清理半成品。
/// 仅对下载阶段有效；install 阶段(pkexec/apt)已交给系统，不在此中断。
#[tauri::command]
pub fn cancel_download() {
    DOWNLOAD_CANCEL.store(true, Ordering::SeqCst);
}

/// 校验 deb 路径：必须真实存在、canonicalize 后在 `~/.pinvou3/updates/` 内、
/// `.deb` 结尾、不含单引号（要嵌进 sh -c 的单引号串）。防前端传任意路径喂给 root apt。
fn validate_deb_path(path: &Path) -> Result<PathBuf, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("deb 文件不存在: {e}"))?;
    let dir = paths::updates_dir()
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

fn file_sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// 目录所在文件系统的可用字节数。调 coreutils `df`（deb 环境必装）解析，
/// 命令/解析失败返回 None，调用方据此跳过磁盘预检而非误报空间不足。
fn available_bytes(dir: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["--output=avail", "-B1"])
        .arg(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // 输出两行：表头 "Avail" + 数字（单位 1 字节，-B1）
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

/// 三段数字比较（非字典序：0.10.0 > 0.2.0）。任一边解析失败按「不更新」处理。
fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_numeric_not_lexicographic() {
        assert!(is_newer("0.10.0", "0.2.0")); // 字典序会判反
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "0.2.0"));
    }

    #[test]
    fn semver_equal_or_lower_not_newer() {
        assert!(!is_newer("0.2.0", "0.2.0")); // 相等不提示
        assert!(!is_newer("0.1.9", "0.2.0")); // 降级保护
    }

    #[test]
    fn semver_malformed_not_newer() {
        assert!(!is_newer("abc", "0.2.0"));
        assert!(!is_newer("1.2", "0.2.0")); // 必须三段
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
            "url": "https://www.ma-xiao.com/pinvou3/pinvou3_0.3.0_amd64.deb",
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
        // notes/pub_date/size 缺省不报错（发布脚本字段齐全，这里防御手写清单）
        let m: LatestManifest =
            serde_json::from_str(r#"{"version":"0.3.0","url":"u","sha256":"s"}"#).unwrap();
        assert_eq!(m.notes, "");
        assert_eq!(m.size, 0);
    }

    #[test]
    fn validate_deb_path_whitelist() {
        // 串行锁：PINVOU3_HOME 是进程级 env，并行测试会互相覆盖
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root = std::env::temp_dir().join("pinvou3-updater-test");
        std::env::set_var("PINVOU3_HOME", &root);

        let updates = paths::updates_dir();
        std::fs::create_dir_all(&updates).unwrap();
        let good = updates.join("pinvou3_9.9.9_amd64.deb");
        std::fs::write(&good, b"fake").unwrap();
        assert!(validate_deb_path(&good).is_ok());

        // 目录外文件拒绝
        let outside = root.join("evil.deb");
        std::fs::write(&outside, b"fake").unwrap();
        assert!(validate_deb_path(&outside).is_err());

        // 非 .deb 拒绝
        let txt = updates.join("note.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(validate_deb_path(&txt).is_err());

        // 不存在的文件拒绝（canonicalize 失败）
        assert!(validate_deb_path(&updates.join("ghost.deb")).is_err());

        // 路径穿越拒绝（canonicalize 解析 .. 后落在目录外）
        let traversal = updates.join("../evil.deb");
        assert!(validate_deb_path(&traversal).is_err());

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }
}
