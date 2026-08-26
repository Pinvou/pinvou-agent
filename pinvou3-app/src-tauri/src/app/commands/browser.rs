//! 浏览器 Tauri 命令：前端经统一 invoke 驱动 BrowserManager；平台能力由
//! `get_platform_capabilities` 声明，不在前端或命令层猜测操作系统。
//!
//! 事件（后端 → 前端）：
//! - `browser:navigation`  页面导航（url 变化）
//! - `browser:tabs-changed` 标签页增删
//! - `browser:activated`   对话浏览器工作区被（MCP wrapper 或本模块）准备完成
//! - `browser:stopped`     浏览器停止/崩溃（前端隐藏工作区）

use serde_json::Value;
use tauri::State;

use crate::features::browser::{BrowserManager, NativeSurfaceBounds, TabInfo};

/// renderer/HMR 生命周期开始时由宿主签发全局递增 generation。前端在同一
/// generation 内为每次 show/hide/cleanup 使用从 1 开始严格递增的 sequence。
///
/// 保持 Tauri async execution context：macOS 的 blocking command 会直接运行在
/// WKWebView 的 URL scheme 主线程回调栈中；这里随后会获取浏览器状态锁，可能与
/// 后台持久化读取 WebView 状态形成主线程/worker 锁环。
#[tauri::command(async)]
pub fn browser_begin_surface_generation(mgr: State<'_, BrowserManager>) -> u64 {
    mgr.begin_surface_generation()
}

/// 将当前对话的系统原生 WebView 表面承载到 Tauri 主窗口指定区域。
/// 返回 false 表示原生工作区尚未创建；前端显示错误与重试，不切换截图流。
#[tauri::command]
pub async fn browser_show_native_surface(
    session_id: String,
    bounds: NativeSurfaceBounds,
    visibility_generation: u64,
    visibility_sequence: u64,
    window: tauri::Window,
    mgr: State<'_, BrowserManager>,
) -> Result<bool, String> {
    mgr.show_native_surface(
        &window,
        &session_id,
        bounds,
        visibility_generation,
        visibility_sequence,
    )
    .await
}

#[tauri::command(async)]
pub fn browser_hide_native_surface(
    session_id: String,
    visibility_generation: u64,
    visibility_sequence: u64,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.hide_native_surface(&session_id, visibility_generation, visibility_sequence)
}

#[tauri::command]
pub async fn browser_stop(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.stop_for_session(&session_id).await
}

#[tauri::command]
pub async fn browser_status(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<Value, String> {
    Ok(mgr.status(&session_id).await)
}

/// 普通模式中用户首次展开浏览器侧栏时按需创建当前任务的空白工作区。
#[tauri::command]
pub async fn browser_prepare(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<Value, String> {
    mgr.prepare_for_user(&session_id).await
}

#[tauri::command(async)]
pub fn browser_hand_back_to_agent(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<Value, String> {
    mgr.hand_back_to_agent(&session_id)
}

#[tauri::command]
pub async fn browser_navigate(
    session_id: String,
    url: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.navigate(&session_id, url).await
}

#[tauri::command]
pub async fn browser_back(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.go_back(&session_id).await
}

#[tauri::command]
pub async fn browser_forward(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.go_forward(&session_id).await
}

#[tauri::command]
pub async fn browser_reload(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.reload(&session_id).await
}

#[tauri::command]
pub async fn browser_list_tabs(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<Vec<TabInfo>, String> {
    mgr.list_tabs(&session_id).await
}

#[tauri::command]
pub async fn browser_create_tab(
    session_id: String,
    url: String,
    background: Option<bool>,
    mgr: State<'_, BrowserManager>,
) -> Result<String, String> {
    mgr.create_tab(&session_id, url, background.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn browser_close_tab(
    session_id: String,
    target_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.close_tab(&session_id, target_id).await
}

#[tauri::command]
pub async fn browser_activate_tab(
    session_id: String,
    target_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.activate_tab(&session_id, target_id).await
}
