//! 应用内升级：
//! - Linux：检查 latest.json → 下载 deb（sha256 校验）→ pkexec apt 安装 → 重启。
//! - Windows：社区版首发暂不提供应用内更新，安装包由 GitHub Releases 分发。
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

mod platform;

pub(crate) fn cleanup_stale_backup() {
    platform::cleanup_stale_backup();
}

/// 下载停滞看门狗阈值：连上后单次等待数据超过此时长即判定挂死。
/// 用「单 chunk 间隔」而非「总耗时」做超时——慢网持续小流量不会被误杀，
/// 只有真正长时间收不到任何字节（更新源挂起 / 半开连接）才中断。
const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 下载取消标志。前端 `cancel_download` 置位，下载循环每轮检查一次。
/// 进程级单例：同一时刻只允许一个下载在跑（前端 updateDownloading gate 保证）。
static DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);

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
    #[serde(default)]
    pub platform: String,
    /// 多平台清单:`{ "macos-universal": PlatformAsset, "linux-arm64": ..., ... }`。
    /// 旧版 latest.json 没这字段 → 空 map → 调用方回退到顶层 url/sha256/size。
    #[serde(default)]
    pub platforms: std::collections::HashMap<String, PlatformAsset>,
}

/// 多平台更新清单的单平台资产。`latest.json` 的 `platforms` map 每个值用这个类型。
/// 客户端按平台 key（如 `macos-universal`、`linux-arm64`）选自己平台的资产;缺失则回退到顶层 url/sha256/size。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlatformAsset {
    pub url: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub restart_after_install: bool,
    #[serde(default)]
    pub notes: String,
    /// 本平台资产自己的版本号。各平台独立发版(Mac 先发 0.7.0、Linux 还在 0.6.3)时,
    /// 客户端用**本平台**版本判要不要升级,而不是读顶层 .version —— 后者代表「最近一次
    /// Linux 发版」,Mac 客户端读它会被误导。旧 manifest / 旧 platform 条目无此字段 → 空
    /// 串 → 调用方回退到顶层 version(向后兼容)。
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DownloadUpdateResult {
    #[allow(dead_code)]
    Path(String),
    // Prepared 为平台更新流程的预留变体,当前各平台实现均不构造,
    // 故在 lib 视角下被误报为 dead code;保留以备后续平台接入。
    #[allow(dead_code)]
    Prepared(PreparedUpdate),
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedUpdate {
    pub package_path: String,
    pub installer_path: String,
    pub latest_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingUpdateReportResult {
    pub had_pending: bool,
    pub reported: bool,
    pub result: String,
    pub message: String,
}
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 拉 latest.json 与当前版本比较。网络失败返回 Err——启动静默检查由前端吞掉，
/// 手动检查才展示错误。
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    platform::check_for_update_info(&client, current).await
}

/// 下载更新包到 `~/.pinvou3/updates/`，流式写盘 + 校验，进度走
/// `update:progress` 事件。
pub async fn download_update(
    info: UpdateInfo,
    app: AppHandle,
) -> Result<DownloadUpdateResult, String> {
    platform::download_update_package(&info, app, &DOWNLOAD_CANCEL, DOWNLOAD_STALL_TIMEOUT).await
}

/// 安装下载好的更新包。Linux 走 pkexec apt；macOS 打开已校验的安装镜像。
pub async fn install_update(
    deb_path: Option<String>,
    installer_path: Option<String>,
    info: Option<UpdateInfo>,
    app: AppHandle,
) -> Result<(), String> {
    let exit_after_start = tokio::task::spawn_blocking(move || {
        platform::install_downloaded_update(deb_path, installer_path, info)
    })
    .await
    .map_err(|e| format!("安装任务失败: {e}"))??;
    if exit_after_start {
        app.exit(0);
    }
    Ok(())
}

/// 重启应用使新版本生效（exec 新 inode）。
pub async fn restart_app(app: AppHandle) -> Result<(), String> {
    app.restart();
}

/// 置位取消标志，让正在跑的 `download_update` 循环下一轮自行退出并清理半成品。
/// 仅对下载阶段有效；install 阶段(pkexec/apt)已交给系统，不在此中断。
pub fn cancel_download() {
    DOWNLOAD_CANCEL.store(true, Ordering::SeqCst);
}

/// 查询是否有待处理的升级结果；社区平台默认静默成功。
pub async fn report_pending_update_result() -> Result<PendingUpdateReportResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    platform::report_pending_update_result_info(&client, env!("CARGO_PKG_VERSION")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_info_platforms_defaults_empty() {
        // 旧版 latest.json(无 platforms 字段)反序列化应得空 map,不报错。
        let json = r#"{
            "available": true,
            "current_version": "0.1.0",
            "latest_version": "0.2.0",
            "notes": "",
            "pub_date": "",
            "url": "https://example.com/p.pkg",
            "sha256": "abc",
            "size": 0
        }"#;
        let info: UpdateInfo = serde_json::from_str(json).unwrap();
        assert!(info.platforms.is_empty());
    }

    #[test]
    fn platform_asset_roundtrip() {
        let asset = PlatformAsset {
            url: "https://example.com/m.dmg".to_string(),
            format: "dmg".to_string(),
            sha256: "deadbeef".to_string(),
            size: 1024,
            restart_after_install: false,
            notes: "mac only".to_string(),
            version: "0.7.0".to_string(),
        };
        let json = serde_json::to_string(&asset).unwrap();
        let back: PlatformAsset = serde_json::from_str(&json).unwrap();
        assert_eq!(back.url, asset.url);
        assert_eq!(back.size, 1024);
        assert_eq!(back.format, "dmg");
        assert_eq!(back.version, "0.7.0");
    }
}
