pub(super) mod detach;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod standard;

#[cfg(target_os = "linux")]
pub(super) use linux::apply_pet_window_policy;
#[cfg(target_os = "linux")]
pub(super) use linux::effective_window_size;
#[cfg(target_os = "linux")]
pub(super) use linux::finish_main_focus_raise;
#[cfg(target_os = "linux")]
pub(super) use linux::prepare_main_focus_raise;
#[cfg(target_os = "linux")]
pub(super) use linux::resize_pet_window;
#[cfg(target_os = "macos")]
pub(super) use macos::finish_main_focus_raise;
#[cfg(target_os = "macos")]
pub(super) use macos::prepare_main_focus_raise;
#[cfg(target_os = "macos")]
pub(super) use macos::{apply_pet_window_policy, effective_window_size, resize_pet_window};
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::apply_pet_window_policy;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::effective_window_size;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::finish_main_focus_raise;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::prepare_main_focus_raise;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) use standard::resize_pet_window;

/// 非 macOS 平台共享的两步 resize:set_size(保持窗口左上原点)后再按锚点角把
/// 原点移到目标位置。X11/Win32 的 resize 与定位本就独立原生调用,这里保持既有
/// 行为;macOS 的非原子问题由该平台单独的 setFrame:display: 路径处理。
pub(super) fn resize_pet_window_fallback(
    win: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    anchor: super::pet_window::ScaleAnchor,
) {
    let before = win.inner_size().ok().zip(win.inner_position().ok());
    let work_area = win.current_monitor().ok().flatten().map(|monitor| {
        let area = monitor.work_area();
        (
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        )
    });
    let sf = win.scale_factor().unwrap_or(1.0);
    let (nw, nh) = (
        (logical_size.0 * sf).round() as u32,
        (logical_size.1 * sf).round() as u32,
    );
    let _ = win.set_size(tauri::PhysicalSize::new(nw, nh));
    // 诊断:X11 的 resize 异步生效,这里的回读多为旧值(GB10 实测会拿到上一个
    // 状态的尺寸),只能当观测信号,绝不能拿来做定位数学——定位一律用请求值,
    // 请求值已经过 pet_window_effective_size 与真实钳制对齐。
    if let Ok(size) = win.inner_size() {
        if (size.width, size.height) != (nw, nh) {
            eprintln!(
                "[pet resize] requested {nw}x{nh} readback {}x{} (async, stale ok)",
                size.width, size.height
            );
        }
    }
    if let Some((size, pos)) = before {
        let (nx, ny) = super::pet_window::resized_position(
            (pos.x, pos.y),
            (size.width, size.height),
            (nw, nh),
            anchor,
            work_area,
        );
        let _ = win.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
}
