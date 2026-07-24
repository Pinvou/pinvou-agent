pub(crate) fn effective_window_size(size: (f64, f64)) -> (f64, f64) {
    size
}

pub(crate) fn apply_pet_window_policy(_window: &tauri::WebviewWindow) {}

pub(crate) fn prepare_main_focus_raise(_app: &tauri::AppHandle) {}

pub(crate) fn finish_main_focus_raise(window: &tauri::WebviewWindow) {
    let _ = window.set_always_on_top(false);
}
