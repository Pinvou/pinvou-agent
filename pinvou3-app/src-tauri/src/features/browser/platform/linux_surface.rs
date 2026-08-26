//! Linux native browser surface layout.
//!
//! Tauri 2.11 builds every Linux child WebView in Tao's default `GtkBox`.
//! WebKitGTK packs children of that box with `expand=true, fill=true`, and Wry
//! can only apply child bounds when the original parent was `GtkFixed`. Keep
//! the application WebView as the base of a `GtkOverlay`, reparent task browser
//! WebViews into a fixed overlay, and apply their logical allocation directly.

use gtk::prelude::*;
use tauri::Webview;

use super::super::NativeSurfaceBounds;

const OVERLAY_WIDGET_NAME: &str = "pinvou-native-browser-overlay";
const FIXED_WIDGET_NAME: &str = "pinvou-native-browser-fixed";

pub(super) fn prepare(main_webview: &Webview) -> Result<(), String> {
    main_webview
        .with_webview(|platform| {
            let main = platform.inner();
            if let Err(error) = prepare_main_widget(&main) {
                eprintln!("[browser] Linux 原生浏览器 overlay 预初始化失败: {error}");
            }
        })
        .map_err(|error| format!("调度 Linux 原生浏览器 overlay 预初始化失败: {error}"))
}

pub(super) fn attach(webview: &Webview) -> Result<(), String> {
    let label = webview.label().to_string();
    webview
        .with_webview(move |platform| {
            let native = platform.inner();
            if let Err(error) = attach_widget(&native) {
                native.hide();
                eprintln!("[browser] Linux 原生浏览器表面挂载失败 ({label}): {error}");
            }
        })
        .map_err(|error| format!("调度 Linux 原生浏览器表面挂载失败: {error}"))
}

pub(super) fn show(webview: &Webview, bounds: Option<NativeSurfaceBounds>) -> Result<(), String> {
    let label = webview.label().to_string();
    webview
        .with_webview(move |platform| {
            let native = platform.inner();
            let result = attach_widget(&native).and_then(|fixed| {
                if let Some(bounds) = bounds {
                    apply_bounds(&fixed, &native, bounds)?;
                }
                native.show();
                Ok(())
            });
            if let Err(error) = result {
                // Fail closed: a layout failure must never reveal the WebView
                // in Tauri's fill/expand GtkBox and cover the application.
                native.hide();
                eprintln!("[browser] Linux 原生浏览器表面显示失败 ({label}): {error}");
            }
        })
        .map_err(|error| format!("调度 Linux 原生浏览器表面显示失败: {error}"))
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
        .ok_or_else(|| "WebView 没有 GTK 父容器".to_string())?;
    let vbox = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "WebView 未挂载在 Tauri 默认 GtkBox".to_string())?;

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
        .ok_or_else(|| "找不到应用主 WebView，无法建立 overlay".to_string())?;

    vbox.remove(native);
    let fixed = install_overlay(&vbox, &main_widget, main_index)?;
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
            .ok_or_else(|| "浏览器 overlay 缺少 GtkFixed 容器".to_string());
    }

    let parent = main
        .parent()
        .ok_or_else(|| "应用主 WebView 没有 GTK 父容器".to_string())?;
    let vbox = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "应用主 WebView 未挂载在 Tauri 默认 GtkBox".to_string())?;
    if let Some((_, fixed)) = find_overlay_host(&vbox) {
        return Ok(fixed);
    }
    let main_widget: gtk::Widget = main.clone().upcast();
    let main_index = vbox
        .children()
        .iter()
        .position(|child| child == &main_widget)
        .ok_or_else(|| "应用主 WebView 不在其 GTK 父容器中".to_string())?;
    install_overlay(&vbox, &main_widget, main_index)
}

fn install_overlay(
    vbox: &gtk::Box,
    main_widget: &gtk::Widget,
    main_index: usize,
) -> Result<gtk::Fixed, String> {
    let expected_parent: gtk::Widget = vbox.clone().upcast();
    if main_widget.parent().as_ref() != Some(&expected_parent) {
        return Err("应用主 WebView GTK 父容器在挂载前发生变化".to_string());
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
    vbox.pack_start(&overlay, true, true, 0);
    vbox.reorder_child(&overlay, main_index as i32);

    // Do not call show_all(): staged browser tabs must remain hidden until the
    // host publishes them. Only the structural containers and main app view
    // are made visible here.
    main_widget.show();
    fixed.show();
    overlay.show();
    Ok(fixed)
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
    let scale = f64::from(native.scale_factor());
    if scale <= 0.0 {
        return Err("WebView scale factor 无效".to_string());
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
    use super::{logical_bounds, LogicalBounds};
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
}
