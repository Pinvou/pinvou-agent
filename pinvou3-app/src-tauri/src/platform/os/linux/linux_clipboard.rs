//! Linux 剪贴板图像读取（输入框粘贴图片兜底）。
//!
//! WebKitGTK 的 paste 事件不向页面暴露图像数据：剪贴板为图像（或文本+图像）时
//! `clipboardData` 的 items/types 均为空（实测 2.52.6），前端过滤恒为空。本模块经
//! GTK 剪贴板（X11/Wayland 由 GTK 抽象统一覆盖）回读图像并编码为 PNG bytes。
//! 前端消费经 `pasteImageClipboardRead` 能力位，见 `platform/capabilities.rs`。

use std::sync::mpsc;
use std::time::Duration;

use gtk::glib;

/// 主线程剪贴板回程上限。owner 进程死亡时请求通常立即失败；live 但挂起的
/// owner 无法靠协议层兜底，该超时在主循环上主动放弃本次读取，保证 UI 不因
/// 单次粘贴被冻结（代价是慢 owner 的图像晚到后被丢弃）。
const CLIPBOARD_READ_TIMEOUT: Duration = Duration::from_secs(3);

pub fn read_clipboard_image(app: &tauri::AppHandle) -> Option<Vec<u8>> {
    let (sender, receiver) = mpsc::channel();
    // GTK 剪贴板只能在主（GTK）线程访问；命令运行在 Tauri 异步线程，
    // 派发一次异步回读到主循环，结果经 channel 带回。
    if let Err(error) = app.run_on_main_thread(move || {
        request_clipboard_image(sender);
    }) {
        log::warn!("[pinvou3][clipboard] main-thread dispatch failed: {error}");
        return None;
    }
    match receiver.recv_timeout(CLIPBOARD_READ_TIMEOUT) {
        Ok(image) => image,
        Err(error) => {
            log::warn!("[pinvou3][clipboard] clipboard roundtrip did not answer: {error}");
            None
        }
    }
}

/// 派发异步图像请求并安排超时放弃；仅可在主（GTK）线程调用。
/// 刻意不用 `wait_for_image`：它在嵌套主循环里自旋，owner 挂起会冻结整个 UI
/// 且不受命令线程的 recv_timeout 约束。异步路径下主循环始终可泵，最坏情况
/// 只是本次粘贴返回 None。
fn request_clipboard_image(sender: mpsc::Sender<Option<Vec<u8>>>) {
    let clipboard = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    // 超时放弃只对本次粘贴回答 None；图像随后才到时通道已关闭，发送被忽略。
    let for_timeout = sender.clone();
    glib::timeout_add_local(CLIPBOARD_READ_TIMEOUT, move || {
        let _ = for_timeout.send(None);
        glib::ControlFlow::Break
    });
    clipboard.request_image(move |_clipboard, image| {
        let answer = image.and_then(|image| match image.save_to_bufferv("png", &[]) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                log::warn!("[pinvou3][clipboard] clipboard image PNG encode failed: {error}");
                None
            }
        });
        let _ = sender.send(answer);
    });
}
