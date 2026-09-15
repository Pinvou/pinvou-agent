//! Linux native browser surface layout.
//!
//! Tauri 2.11 builds every Linux child WebView in Tao's default `GtkBox`.
//! WebKitGTK packs children of that box with `expand=true, fill=true`, and Wry
//! can only apply child bounds when the original parent was `GtkFixed`. Keep
//! the application WebView as the base of a `GtkOverlay`, reparent task browser
//! WebViews into a fixed overlay, and apply their logical allocation directly.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::Duration,
};

use gtk::prelude::*;
use tauri::Webview;

use super::super::NativeSurfaceBounds;

const OVERLAY_WIDGET_NAME: &str = "pinvou-native-browser-overlay";
const FIXED_WIDGET_NAME: &str = "pinvou-native-browser-fixed";
const GTK_DISPATCH_TIMEOUT: Duration = Duration::from_millis(750);
const DISPATCH_PENDING: u8 = 0;
const DISPATCH_RUNNING: u8 = 1;
const DISPATCH_CANCELLED: u8 = 2;
const DISPATCH_FINISHED: u8 = 3;

pub(super) fn prepare(main_webview: &Webview) -> Result<(), String> {
    with_webview_result(
        main_webview,
        "native browser overlay preinitialization",
        |main| prepare_main_widget(main).map(|_| ()),
    )
}

pub(super) fn attach(webview: &Webview) -> Result<(), String> {
    let label = webview.label().to_string();
    with_webview_result(
        webview,
        "native browser surface attachment",
        move |native| {
            attach_widget(native).map(|_| ()).map_err(|error| {
                // A failed attach must not leave the child visible in Tauri's
                // fill/expand GtkBox while the caller decides whether to retry.
                native.hide();
                format!("Linux native browser surface attachment failed ({label}): {error}")
            })
        },
    )
}

pub(super) fn show(webview: &Webview, bounds: Option<NativeSurfaceBounds>) -> Result<(), String> {
    let label = webview.label().to_string();
    with_webview_result(webview, "native browser surface show", move |native| {
        let result = attach_widget(native).and_then(|fixed| {
            // `show_active_workspace` hides every tab before selecting the active
            // one. GTK3 discards `size_allocate` for an invisible non-toplevel
            // widget, so the old order updated GtkFixed's child position but left
            // WebKit's page viewport at its previous size. Make both widgets visible
            // first, then perform the allocation synchronously. GTK cannot paint
            // between these calls because this closure is one main-thread dispatch.
            fixed.show();
            native.show();
            if let Some(bounds) = bounds {
                apply_bounds(&fixed, native, bounds)?;
            }
            Ok(())
        });
        result.map_err(|error| {
            // Fail closed: a layout failure must never reveal the WebView
            // in Tauri's fill/expand GtkBox and cover the application.
            native.hide();
            if let Some(fixed) = native
                .parent()
                .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
            {
                hide_empty_overlay(&fixed);
            }
            format!("Linux native browser surface show failed ({label}): {error}")
        })
    })
}

pub(super) fn hide(webview: &Webview) -> Result<(), String> {
    with_webview_result(webview, "native browser surface hide", |native| {
        native.hide();
        if let Some(fixed) = native
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
        {
            hide_empty_overlay(&fixed);
        }
        Ok(())
    })
}

/// `with_webview` executes inline when called from Tauri's main thread, but
/// only queues the closure when called from a worker. The enqueue `Ok(())` is
/// therefore not an acknowledgement that GTK mutated the physical surface.
/// Round-trip the closure result so callers can keep their logical visibility
/// state unchanged and retry after a real failure.
///
/// A short wait also breaks a possible lock inversion: a worker may hold the
/// browser host lock while the GTK thread is waiting for that lock. If the GTK
/// closure has not started at the deadline, changing Pending -> Cancelled makes
/// the eventually delivered closure a no-op. Once a closure is Running we wait
/// for its synchronous GTK mutation to finish, so show/hide can never land
/// after this function has returned an error. Operations passed here must stay
/// GTK-local and must never acquire BrowserManager/host locks.
fn with_webview_result(
    webview: &Webview,
    action: &'static str,
    operation: impl FnOnce(&webkit2gtk::WebView) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let state = Arc::new(AtomicU8::new(DISPATCH_PENDING));
    let closure_state = Arc::clone(&state);
    // Capacity one is required for the main-thread inline path: the closure
    // sends its ACK before `with_webview` returns and before we start receiving.
    let (sender, receiver) = mpsc::sync_channel(1);
    let dispatch_result = webview.with_webview(move |platform| {
        if !begin_webview_operation(&closure_state) {
            return;
        }
        let native = platform.inner();
        let result = operation(&native);
        closure_state.store(DISPATCH_FINISHED, Ordering::Release);
        let _ = sender.send(result);
    });
    if let Err(error) = dispatch_result {
        state.store(DISPATCH_CANCELLED, Ordering::Release);
        return Err(format!("Failed to schedule Linux {action}: {error}"));
    }
    receive_webview_result(&receiver, &state, action, GTK_DISPATCH_TIMEOUT)
}

fn begin_webview_operation(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            DISPATCH_PENDING,
            DISPATCH_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn receive_webview_result(
    receiver: &Receiver<Result<(), String>>,
    state: &AtomicU8,
    action: &str,
    timeout: Duration,
) -> Result<(), String> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Disconnected) => Err(format!(
            "Linux {action} was interrupted before GTK acknowledgement"
        )),
        Err(RecvTimeoutError::Timeout) => match state.compare_exchange(
            DISPATCH_PENDING,
            DISPATCH_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Err(format!(
                "Linux {action} timed out waiting for the GTK main thread; the unexecuted operation was cancelled"
            )),
            Err(DISPATCH_RUNNING | DISPATCH_FINISHED) => receiver.recv().map_err(|_| {
                format!("Linux {action} was interrupted before GTK acknowledgement")
            })?,
            Err(_) => Err(format!("Linux {action} was cancelled")),
        },
    }
}

fn attach_widget(native: &webkit2gtk::WebView) -> Result<gtk::Fixed, String> {
    if let Some(fixed) = native
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
    {
        return Ok(fixed);
    }

    let parent = native
        .parent()
        .ok_or_else(|| "WebView has no GTK parent container".to_string())?;
    let vbox = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "WebView is not mounted in Tauri's default GtkBox".to_string())?;

    if let Some((_, fixed)) = find_overlay_host(&vbox) {
        vbox.remove(native);
        fixed.put(native, 0, 0);
        native.set_size_request(1, 1);
        return Ok(fixed);
    }

    let children = vbox.children();
    let native_widget: gtk::Widget = native.clone().upcast();
    let (main_index, main_widget) = children
        .iter()
        .enumerate()
        .find(|(_, child)| *child != &native_widget && child.is::<webkit2gtk::WebView>())
        .map(|(index, child)| (index, child.clone()))
        .ok_or_else(|| "Main app WebView was not found; cannot create overlay".to_string())?;

    // install_overlay's only failure path (parent-changed guard) runs before it
    // mutates anything, so detaching native only after it succeeded keeps the
    // failed attach retryable: native still has its GTK parent for the next
    // attempt. Removing native first would strand it parentless and make every
    // retry fail at the "no GTK parent container" guard above.
    let fixed = install_overlay(&vbox, &main_widget, main_index)?;
    vbox.remove(native);
    fixed.put(native, 0, 0);
    native.set_size_request(1, 1);
    Ok(fixed)
}

fn prepare_main_widget(main: &webkit2gtk::WebView) -> Result<gtk::Fixed, String> {
    if let Some(overlay) = main
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Overlay>().ok())
    {
        return overlay
            .children()
            .into_iter()
            .find_map(|child| {
                (child.widget_name() == FIXED_WIDGET_NAME)
                    .then(|| child.downcast::<gtk::Fixed>().ok())
                    .flatten()
            })
            .ok_or_else(|| "Browser overlay is missing its GtkFixed container".to_string());
    }

    let parent = main
        .parent()
        .ok_or_else(|| "Main app WebView has no GTK parent container".to_string())?;
    let vbox = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "Main app WebView is not mounted in Tauri's default GtkBox".to_string())?;
    if let Some((_, fixed)) = find_overlay_host(&vbox) {
        return Ok(fixed);
    }
    let main_widget: gtk::Widget = main.clone().upcast();
    let main_index = vbox
        .children()
        .iter()
        .position(|child| child == &main_widget)
        .ok_or_else(|| "Main app WebView is absent from its GTK parent container".to_string())?;
    install_overlay(&vbox, &main_widget, main_index)
}

fn install_overlay(
    vbox: &gtk::Box,
    main_widget: &gtk::Widget,
    main_index: usize,
) -> Result<gtk::Fixed, String> {
    let expected_parent: gtk::Widget = vbox.clone().upcast();
    if main_widget.parent().as_ref() != Some(&expected_parent) {
        return Err("Main app WebView GTK parent changed before attachment".to_string());
    }
    let overlay = gtk::Overlay::new();
    overlay.set_widget_name(OVERLAY_WIDGET_NAME);
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    let fixed = gtk::Fixed::new();
    fixed.set_widget_name(FIXED_WIDGET_NAME);
    fixed.set_hexpand(true);
    fixed.set_vexpand(true);
    fixed.set_halign(gtk::Align::Fill);
    fixed.set_valign(gtk::Align::Fill);

    vbox.remove(main_widget);
    overlay.add(main_widget);
    overlay.add_overlay(&fixed);
    // GtkOverlay children intercept pointer input by default. Keep the fixed
    // layout window transparent to input so clicks outside the bounded
    // WebKitWebView reach the main application. WebKitWebView owns a child
    // GdkWindow, which remains interactive under GTK's pass-through model.
    overlay.set_overlay_pass_through(&fixed, true);
    vbox.pack_start(&overlay, true, true, 0);
    vbox.reorder_child(&overlay, main_index as i32);

    // Do not call show_all(): staged browser tabs must remain hidden until the
    // host publishes them. The empty fixed overlay must also remain unmapped,
    // otherwise it can block the main app before any browser surface exists.
    main_widget.show();
    overlay.show();
    Ok(fixed)
}

fn hide_empty_overlay(fixed: &gtk::Fixed) {
    if !fixed.children().iter().any(|child| child.is_visible()) {
        fixed.hide();
    }
}

fn find_overlay_host(vbox: &gtk::Box) -> Option<(gtk::Overlay, gtk::Fixed)> {
    let overlay = vbox.children().into_iter().find_map(|child| {
        (child.widget_name() == OVERLAY_WIDGET_NAME)
            .then(|| child.downcast::<gtk::Overlay>().ok())
            .flatten()
    })?;
    let fixed = overlay.children().into_iter().find_map(|child| {
        (child.widget_name() == FIXED_WIDGET_NAME)
            .then(|| child.downcast::<gtk::Fixed>().ok())
            .flatten()
    })?;
    Some((overlay, fixed))
}

fn apply_bounds(
    fixed: &gtk::Fixed,
    native: &webkit2gtk::WebView,
    bounds: NativeSurfaceBounds,
) -> Result<(), String> {
    if !fixed.is_visible() || !native.is_visible() {
        return Err("WebView must be visible before applying Linux native bounds".to_string());
    }
    let scale = f64::from(native.scale_factor());
    if scale <= 0.0 {
        return Err("WebView scale factor is invalid".to_string());
    }
    let logical = logical_bounds(bounds, scale);
    fixed.move_(native, logical.x, logical.y);
    native.set_size_request(logical.width, logical.height);
    native.size_allocate(&gtk::Allocation::new(
        logical.x,
        logical.y,
        logical.width,
        logical.height,
    ));
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LogicalBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn logical_bounds(bounds: NativeSurfaceBounds, scale: f64) -> LogicalBounds {
    let position = tauri::PhysicalPosition::new(bounds.x, bounds.y).to_logical::<i32>(scale);
    let size = tauri::PhysicalSize::new(bounds.width, bounds.height).to_logical::<i32>(scale);
    LogicalBounds {
        x: position.x,
        y: position.y,
        width: size.width.max(1),
        height: size.height.max(1),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{atomic::AtomicU8, mpsc},
        thread,
        time::Duration,
    };

    use super::{
        DISPATCH_PENDING, DISPATCH_RUNNING, LogicalBounds, begin_webview_operation, logical_bounds,
        receive_webview_result,
    };
    use crate::features::browser::NativeSurfaceBounds;

    #[test]
    fn physical_renderer_bounds_are_converted_to_gtk_logical_coordinates() {
        assert_eq!(
            logical_bounds(
                NativeSurfaceBounds {
                    x: 640,
                    y: 120,
                    width: 800,
                    height: 1000,
                },
                2.0,
            ),
            LogicalBounds {
                x: 320,
                y: 60,
                width: 400,
                height: 500,
            },
        );
    }

    #[test]
    fn pending_dispatch_timeout_cancels_late_gtk_mutation() {
        let state = AtomicU8::new(DISPATCH_PENDING);
        let (_sender, receiver) = mpsc::sync_channel(1);

        let error =
            receive_webview_result(&receiver, &state, "test hide", Duration::from_millis(1))
                .unwrap_err();

        assert!(error.contains("was cancelled"));
        assert!(!begin_webview_operation(&state));
    }

    #[test]
    fn running_dispatch_is_acknowledged_before_returning() {
        let state = AtomicU8::new(DISPATCH_RUNNING);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(5));
            sender.send(Ok(())).unwrap();
        });

        assert!(
            receive_webview_result(&receiver, &state, "test show", Duration::from_millis(1),)
                .is_ok()
        );
        worker.join().unwrap();
    }
}
