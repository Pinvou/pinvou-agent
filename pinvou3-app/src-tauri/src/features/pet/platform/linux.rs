pub(crate) fn effective_window_size(size: (f64, f64)) -> (f64, f64) {
    const WEBVIEW_MIN: f64 = 200.0;
    (size.0.max(WEBVIEW_MIN), size.1.max(WEBVIEW_MIN))
}

pub(crate) fn apply_pet_window_policy(_window: &tauri::WebviewWindow) {}
