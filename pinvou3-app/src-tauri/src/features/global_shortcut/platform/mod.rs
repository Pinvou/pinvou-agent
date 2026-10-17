//! Bring-to-front composition for the global shortcut trigger.
//!
//! Reuses the shared `platform::window_startup` primitives (cold-start gate
//! plus the per-OS present/focus path) and adds the Linux-only transient
//! raise that works around Mutter/X11 focus-steal protection — the same
//! workaround the pet window's raise uses (features/pet/platform/linux.rs):
//! show/unminimize alone only marks the window demand-attention, so a short
//! always-on-top window forces the raise and is cleared again once focus has
//! landed.

use tauri::{AppHandle, Manager};

/// Present the main window in response to the global shortcut. A missing
/// main window (teardown race) is logged and ignored. When the Linux
/// cold-start gate deferred the presentation (nothing was enqueued), the
/// transient raise is skipped too — there is no present to land after; the
/// press itself is dropped for that moment, bounded by the startup overlay's
/// own reveal (frontend mount or the 8s fallback), mirroring the established
/// second-instance defer contract.
pub(super) fn bring_main_window_to_front(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        log::debug!("global shortcut trigger ignored: main window is gone");
        return;
    };
    // Cold-start gate + present: Linux defers while the hidden startup
    // overlay is still up and presents via GTK on the main thread; Windows/
    // macOS present via show + unminimize + set_focus, with macOS unhiding
    // the app first (a Cmd+H-hidden NSApplication ignores window-level
    // calls).
    let deferred = crate::platform::window_startup::activate_main_window(window.clone());
    if deferred {
        return;
    }
    raise_after_present(&window);
}

/// Linux: transient always-on-top raise. The whole composition runs inline on
/// the main thread (dispatched by `handle_triggered`), so these calls execute
/// right after the present inside `activate_main_window` above.
#[cfg(target_os = "linux")]
fn raise_after_present(window: &tauri::WebviewWindow) {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// How long the transient always-on-top stays before being cleared: long
    /// enough for Mutter to complete the activation, short enough that a
    /// deliberate click-through is not blocked (same value as the pet raise).
    const HOLD_MS: u64 = 180;

    // Generation counter so a newer raise is never cleared by an older
    // timer firing late (two rapid shortcut presses).
    static GENERATION: AtomicU64 = AtomicU64::new(0);
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);

    if let Err(error) = window.set_always_on_top(true) {
        log::warn!("global shortcut set_always_on_top failed: {error}");
    }
    let focus_result = window.set_focus();
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(HOLD_MS)).await;
        // Check AND clear on the main thread: the raise of a newer press
        // runs inline on the main thread too, so joining the generation
        // check into the same thread's FIFO makes raise/clear order total —
        // a stale clear can never land after (and cancel) a newer raise.
        let app = window.app_handle().clone();
        let _ = app.run_on_main_thread(move || {
            if GENERATION.load(Ordering::Acquire) == generation {
                let _ = window.set_always_on_top(false);
            }
        });
    });
    if let Err(error) = focus_result {
        log::warn!("global shortcut set_focus failed: {error}");
    }
}

#[cfg(not(target_os = "linux"))]
fn raise_after_present(window: &tauri::WebviewWindow) {
    // activate_main_window already did show + unminimize + set_focus; the
    // focus-steal workaround is a Mutter/X11 problem only.
    let _ = window;
}
