//! 应用内升级：
//! - Linux：检查 latest.json → 下载 deb（sha256 校验）→ pkexec apt 安装 → 重启。
//! - Windows：查询 H3C OTA → 下载 zip（MD5 校验）→ 解析 FullPack/OtaInfo → 启动 MSI → 下次启动反馈。
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
    pub package_md5: String,
    #[serde(default)]
    pub software_id: String,
    #[serde(default)]
    pub sn: String,
    #[serde(default)]
    pub update_type: String,
    #[serde(default)]
    pub platform: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DownloadUpdateResult {
    #[allow(dead_code)]
    Path(String),
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

#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 拉 latest.json 与当前版本比较。网络失败返回 Err——启动静默检查由前端吞掉，
/// 手动检查才展示错误。
#[tauri::command]
pub async fn check_for_update() -> Result<UpdateInfo, String> {
    let current = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    crate::os::check_for_update_info(&client, current).await
}

/// 下载更新包到 `~/.pinvou3/updates/`，流式写盘 + 校验，进度走
/// `update:progress` 事件。Linux 返回 deb 路径字符串；Windows 返回
/// 已解析出的 zip/MSI 信息对象。
#[tauri::command]
pub async fn download_update(
    info: UpdateInfo,
    app: AppHandle,
) -> Result<DownloadUpdateResult, String> {
    crate::os::download_update_package(&info, app, &DOWNLOAD_CANCEL, DOWNLOAD_STALL_TIMEOUT).await
}

/// 安装下载好的更新包。Linux 走 pkexec apt；Windows 启动 MSI，成功启动后退出进程。
#[tauri::command]
pub async fn install_update(
    deb_path: Option<String>,
    installer_path: Option<String>,
    info: Option<UpdateInfo>,
    app: AppHandle,
) -> Result<(), String> {
    let exit_after_start = tokio::task::spawn_blocking(move || {
        crate::os::install_downloaded_update(deb_path, installer_path, info)
    })
    .await
    .map_err(|e| format!("安装任务失败: {e}"))??;
    if exit_after_start {
        app.exit(0);
    }
    Ok(())
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

/// Windows OTA 安装后反馈升级结果；其他平台无待反馈记录时静默成功。
#[tauri::command]
pub async fn report_pending_update_result() -> Result<PendingUpdateReportResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    crate::os::report_pending_update_result_info(&client, env!("CARGO_PKG_VERSION")).await
}
