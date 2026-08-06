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

pub(crate) fn prepare_main_focus_raise(_app: &tauri::AppHandle) {}

/// 原子地设置桌宠窗口的尺寸与位置(单次 `setFrame:display:YES`)。
///
/// 见 `pet_window::resize_pet_window` 的 macOS 分支注释:tao 把 `set_size` 与
/// `set_position` 各自 `dispatch_async` 到主队列,不是原子操作,展开气泡时会让
/// 人物向左上方闪现一帧。这里在主线程上用一条 `setFrame:display:` 同时提交新
/// 尺寸和新原点,`display:YES` 强制同步重绘,消除中间合成帧。
///
/// 锚点角固定:直接在 NSWindow 原生坐标(原点左下、y 朝上)里读当前 frame 做代数,
/// 与 `pet_window::resized_position` 的六种锚点几何等价,但省去 Tauri 物理坐标
/// 到 Cocoa 坐标的翻转转换。
pub(crate) fn resize_pet_window(
    window: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    anchor: crate::features::pet::pet_window::ScaleAnchor,
) {
    if objc2::MainThreadMarker::new().is_some() {
        resize_pet_window_impl(window, logical_size, anchor);
    } else {
        let app = window.app_handle().clone();
        let window = window.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            resize_pet_window_impl(&window, logical_size, anchor);
        }) {
            eprintln!("[pet] macOS 原子设置窗口 frame 失败: {error}");
        }
    }
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

fn resize_pet_window_impl(
    window: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    anchor: crate::features::pet::pet_window::ScaleAnchor,
) {
    use objc2::rc::Retained;
    use objc2_app_kit::NSWindow;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    use crate::features::pet::pet_window::ScaleAnchor;

    let raw: *mut std::ffi::c_void = match window.ns_window() {
        Ok(pointer) => pointer,
        Err(_) => return,
    };
    let Some(ns_window): Option<Retained<NSWindow>> =
        (unsafe { Retained::retain_autoreleased(raw.cast()) })
    else {
        return;
    };

    // NSWindow 的 frame 一律是 Cocoa 点(逻辑坐标),与 tao 调 setContentSize 前先
    // to_logical 一致——这里直接用传入的 logical_size,无需 scale_factor。
    let new_w = logical_size.0;
    let new_h = logical_size.1;
    let cur = ns_window.frame();
    // 按锚点角固定:保持该角在屏幕上不动,反推新原点。原点在左下、y 朝上,
    // 故底边锚点 origin.y 不动,顶边锚点 origin.y 上移 (旧高 - 新高)。
    let (mut origin_x, mut origin_y) = match anchor {
        ScaleAnchor::BottomLeft => (cur.origin.x, cur.origin.y),
        ScaleAnchor::BottomCenter => (cur.origin.x + (cur.size.width - new_w) / 2.0, cur.origin.y),
        ScaleAnchor::BottomRight => (cur.origin.x + cur.size.width - new_w, cur.origin.y),
        ScaleAnchor::TopLeft => (cur.origin.x, cur.origin.y + cur.size.height - new_h),
        ScaleAnchor::TopCenter => (
            cur.origin.x + (cur.size.width - new_w) / 2.0,
            cur.origin.y + cur.size.height - new_h,
        ),
        ScaleAnchor::TopRight => (
            cur.origin.x + cur.size.width - new_w,
            cur.origin.y + cur.size.height - new_h,
        ),
    };
    // 与 pet_window::resized_position 的 work_area 钳制对齐:用窗口当前所在屏的
    // visibleFrame(已扣除 Dock/菜单栏)把新原点夹在工作区内,避免长大后顶出屏幕。
    if let Some(screen) = ns_window.screen() {
        let vf = screen.visibleFrame();
        let min_x = vf.origin.x;
        let max_x = (vf.origin.x + vf.size.width - new_w).max(min_x);
        let min_y = vf.origin.y;
        let max_y = (vf.origin.y + vf.size.height - new_h).max(min_y);
        origin_x = origin_x.clamp(min_x, max_x);
        origin_y = origin_y.clamp(min_y, max_y);
    }
    let frame = NSRect::new(NSPoint::new(origin_x, origin_y), NSSize::new(new_w, new_h));
    ns_window.setFrame_display(frame, true);
}
