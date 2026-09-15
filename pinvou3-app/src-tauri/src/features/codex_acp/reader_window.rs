//! 独立代码阅读器窗口（单例 + 前端 tab 复用，Win11 记事本模式）。
//!
//! 主窗口代码弹窗的「新窗口打开」把文件交给本模块：窗口已存在则发
//! `code-reader:open` 事件让 ReaderApp 新增/激活 tab 并聚焦；不存在则先入
//! pending 队列再建窗，ReaderApp 挂载后经 `take_code_reader_pending` 拉取，
//! 避免窗口加载与事件推送之间的时序竞态。

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

pub const READER_LABEL: &str = "code-reader";
pub const READER_OPEN_EVENT: &str = "code-reader:open";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReaderOpenRequest {
    pub session_id: Option<String>,
    pub workspace_path: Option<String>,
    pub relative_path: String,
    /// 打开模式：`None`/`"file"` = 文件内容（preview），`"diff"` = 工作区变更差异。
    #[serde(default)]
    pub kind: Option<String>,
}

static PENDING_OPEN: Mutex<Vec<ReaderOpenRequest>> = Mutex::new(Vec::new());

fn pending_open() -> std::sync::MutexGuard<'static, Vec<ReaderOpenRequest>> {
    PENDING_OPEN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

pub fn open_code_reader(app: &AppHandle, request: ReaderOpenRequest) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(READER_LABEL) {
        // 推拉双保险：事件广播（pet_window 同通路）追求即时性；同时入 pending 队列，
        // ReaderApp 在窗口获得焦点时会再次拉取，保证事件投递失败时文件也不丢。
        // 先 emit 后入队：emit 失败则原样报错、不入队，避免“报失败却又悄悄打开”。
        app.emit(READER_OPEN_EVENT, request.clone())
            .map_err(|error| format!("通知代码阅读器打开文件失败: {error}"))?;
        pending_open().push(request);
        // 聚焦失败（部分窗口管理器限制）不影响文件打开。
        let _ = window.set_focus();
        return Ok(());
    }
    pending_open().push(request);
    WebviewWindowBuilder::new(app, READER_LABEL, WebviewUrl::App("reader.html".into()))
        .title("代码阅读器")
        .inner_size(960.0, 720.0)
        .min_inner_size(480.0, 320.0)
        .build()
        .map_err(|error| format!("创建代码阅读器窗口失败: {error}"))?;
    Ok(())
}

pub fn take_pending_open() -> Vec<ReaderOpenRequest> {
    std::mem::take(&mut *pending_open())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_open_drains_in_fifo_order() {
        assert!(take_pending_open().is_empty());
        let first = ReaderOpenRequest {
            session_id: None,
            workspace_path: Some("D:/proj".to_string()),
            relative_path: "src/main.rs".to_string(),
            kind: None,
        };
        let second = ReaderOpenRequest {
            session_id: Some("session-1".to_string()),
            workspace_path: None,
            relative_path: "README.md".to_string(),
            kind: Some("diff".to_string()),
        };
        pending_open().push(first.clone());
        pending_open().push(second.clone());
        assert_eq!(take_pending_open(), vec![first, second]);
        assert!(take_pending_open().is_empty());
    }
}
