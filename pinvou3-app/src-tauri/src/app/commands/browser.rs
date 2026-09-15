//! Browser Tauri commands. The frontend drives BrowserManager through unified invoke calls.
//! `get_platform_capabilities` declares support, so neither layer guesses the operating system.
//!
//! Backend-to-frontend events:
//! - `browser:navigation`: page URL changed
//! - `browser:tabs-changed`: tabs were added or removed
//! - `browser:activated`: MCP wrapper or this module prepared the conversation workspace
//! - `browser:stopped`: browser stopped or crashed; the frontend hides the workspace

use serde_json::Value;
use tauri::State;

use crate::features::browser::{BrowserManager, NativeSurfaceBounds, TabInfo};

/// The host issues a globally increasing generation at renderer/HMR lifecycle start. Within
/// one generation, the frontend assigns a strictly increasing sequence starting at 1 to
/// each show, hide, and cleanup request.
///
/// Retain Tauri's asynchronous execution context. On macOS, a blocking command would run
/// directly on WKWebView's URL-scheme main-thread callback stack. This command later acquires
/// the browser-state lock and could otherwise deadlock with a persistence worker reading
/// WebView state through the main thread.
#[tauri::command(async)]
pub fn browser_begin_surface_generation(mgr: State<'_, BrowserManager>) -> u64 {
    mgr.begin_surface_generation()
}

/// Hosts the current conversation's system-native WebView surface in the specified main
/// window region. `false` means the native workspace does not exist yet; the frontend shows
/// an error and offers retry without switching to a screenshot stream.
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

/// Lazily creates the current task's blank workspace when the user first expands the browser
/// side panel in normal mode.
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
    request_id: String,
    mgr: State<'_, BrowserManager>,
) -> Result<(), String> {
    mgr.navigate(&session_id, url, &request_id).await
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
