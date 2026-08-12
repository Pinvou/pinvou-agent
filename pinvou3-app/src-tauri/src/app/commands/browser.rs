//! 浏览器 Tauri 命令：前端（桌面/Web/移动三端统一）经 invoke 驱动 BrowserManager。
//!
//! 事件（后端 → 前端）：
//! - `browser:frame`       截图流 JPEG 帧（base64 data + metadata + tab）
//! - `browser:navigation`  页面导航（url 变化）
//! - `browser:tabs-changed` 标签页增删
//! - `browser:activated`   Chrome 被（MCP wrapper 或本模块）启动、品悟接入完成

use serde_json::Value;
use tauri::State;

use crate::features::browser::{BrowserManager, TabInfo};

#[tauri::command]
pub async fn browser_ensure_started(mgr: State<'_, BrowserManager>) -> Result<(), String> {
    mgr.ensure_started().await
}

#[tauri::command]
pub async fn browser_stop(mgr: State<'_, BrowserManager>) -> Result<(), String> {
    mgr.stop().await
}

#[tauri::command]
pub async fn browser_status(mgr: State<'_, BrowserManager>) -> Result<Value, String> {
    Ok(mgr.status().await)
}

#[tauri::command]
pub async fn browser_navigate(
    url: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.navigate(url).await
}

#[tauri::command]
pub async fn browser_back(mgr: State<'_, BrowserManager>) -> Result<(), String> {
    mgr.go_back().await
}

#[tauri::command]
pub async fn browser_forward(mgr: State<'_, BrowserManager>) -> Result<(), String> {
    mgr.go_forward().await
}

#[tauri::command]
pub async fn browser_reload(mgr: State<'_, BrowserManager>) -> Result<(), String> {
    mgr.reload().await
}

#[tauri::command]
pub async fn browser_input(
    payload: Value,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.input_event(payload).await
}

#[tauri::command]
pub async fn browser_list_tabs(mgr: State<'_, BrowserManager>) -> Result<Vec<TabInfo>, String> {
    mgr.list_tabs().await
}

#[tauri::command]
pub async fn browser_create_tab(
    url: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.create_tab(url).await
}

#[tauri::command]
pub async fn browser_close_tab(
    target_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.close_tab(target_id).await
}

#[tauri::command]
pub async fn browser_activate_tab(
    session_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.activate_tab(session_id).await
}
