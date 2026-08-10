use tauri::Manager;

pub(crate) fn effective_window_size(size: (f64, f64)) -> (f64, f64) {
    size
}

pub(crate) fn apply_pet_window_policy(window: &tauri::WebviewWindow) {
    if objc2::MainThreadMarker::new().is_some() {
        apply_pet_window_policy_impl(window);
    } else {
        let app = window.app_handle().clone();
        let window = window.clone();
        if let Err(error) = app.run_on_main_thread(move || apply_pet_window_policy_impl(&window)) {
            eprintln!("[pet] 应用 macOS 桌宠窗口策略失败: {error}");
        }
    }
}

pub(crate) fn prepare_main_focus_raise(app: &tauri::AppHandle) {
    // 桌宠是 NSStatusWindowLevel 的辅助窗，点击它不会可靠地把整个应用激活。
    // 只对主窗口 set_focus 在 Terminal/Safari 位于前台时会返回 Ok、却没有任何
    // 可见效果。先在 AppKit 主线程激活进程，再由调用方 show + set_focus。
    let _ = app.run_on_main_thread(|| {
        use objc2_app_kit::NSApplication;
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).activate();
        }
    });
}

pub(crate) fn finish_main_focus_raise(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(false);
}

fn apply_pet_window_policy_impl(window: &tauri::WebviewWindow) {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};

    let raw: *mut std::ffi::c_void = match window.ns_window() {
        Ok(pointer) => pointer,
        Err(_) => return,
    };
    let Some(ns_window): Option<Retained<NSWindow>> =
        (unsafe { Retained::retain_autoreleased(raw.cast()) })
    else {
        return;
    };
    let behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    ns_window.setCollectionBehavior(behavior);
    ns_window.setLevel(NSStatusWindowLevel);
}
