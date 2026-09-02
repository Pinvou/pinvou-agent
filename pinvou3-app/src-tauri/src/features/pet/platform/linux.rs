pub(crate) fn effective_window_size(size: (f64, f64)) -> (f64, f64) {
    const WEBVIEW_MIN: f64 = 200.0;
    (size.0.max(WEBVIEW_MIN), size.1.max(WEBVIEW_MIN))
}

pub(crate) fn apply_pet_window_policy(_window: &tauri::WebviewWindow) {}

pub(crate) fn resize_pet_window(
    win: &tauri::WebviewWindow,
    logical_size: (f64, f64),
    anchor: crate::features::pet::pet_window::ScaleAnchor,
) {
    super::resize_pet_window_fallback(win, logical_size, anchor);
}

pub(crate) fn prepare_main_focus_raise(app: &tauri::AppHandle) {
    use tauri::Emitter;

    if let Err(error) = app.emit_to("main", "pet:activation_guard", ()) {
        eprintln!("[pet nav] emit activation guard failed: {error}");
    }
}

pub(crate) fn finish_main_focus_raise(window: &tauri::WebviewWindow) {
    use std::sync::atomic::AtomicU64;

    const HOLD_MS: u64 = 180;
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    let generation = next_focus_raise_generation(&GENERATION);
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(HOLD_MS)).await;
        if focus_raise_is_current(&GENERATION, generation) {
            let _ = window.set_always_on_top(false);
        }
    });
}

fn next_focus_raise_generation(counter: &std::sync::atomic::AtomicU64) -> u64 {
    use std::sync::atomic::Ordering;
    counter.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
}

fn focus_raise_is_current(counter: &std::sync::atomic::AtomicU64, generation: u64) -> bool {
    use std::sync::atomic::Ordering;
    counter.load(Ordering::Acquire) == generation
}

#[cfg(test)]
mod tests {
    use super::{focus_raise_is_current, next_focus_raise_generation};
    use std::sync::atomic::AtomicU64;

    #[test]
    fn latest_focus_raise_generation_owns_delayed_clear() {
        let counter = AtomicU64::new(0);
        let first = next_focus_raise_generation(&counter);
        assert!(focus_raise_is_current(&counter, first));

        let second = next_focus_raise_generation(&counter);
        assert!(
            !focus_raise_is_current(&counter, first),
            "an earlier wakeup must not clear a newer raise"
        );
        assert!(focus_raise_is_current(&counter, second));
    }
}
