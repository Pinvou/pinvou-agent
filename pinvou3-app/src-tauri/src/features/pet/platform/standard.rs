pub(crate) fn effective_window_size(size: (f64, f64)) -> (f64, f64) {
    size
}

pub(crate) fn apply_pet_window_policy(_window: &tauri::WebviewWindow) {}

pub(crate) fn resize_pet_window(
    win: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    anchor: crate::features::pet::pet_window::ScaleAnchor,
) {
    super::resize_pet_window_fallback(win, logical_size, anchor);
}

pub(crate) fn prepare_main_focus_raise(_app: &tauri::AppHandle) {}

pub(crate) fn finish_main_focus_raise(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(false);
}
