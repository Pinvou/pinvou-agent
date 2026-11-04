use std::cell::RefCell;

use crate::platform::prefs::Language;
use tauri::{
    Manager,
    tray::{MouseButton, MouseButtonState, TrayIconEvent},
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuItem, PredefinedMenuItem},
};

mod registration;
use registration::{icon_guid, registered};

const TRAY_ID: &str = "main-tray";

// tray-icon/muda's Rc-backed native resources stay on Tauri's UI thread.
// Setup, native event dispatch, Exit and the restart removal below run there.
thread_local! {
    static OWNED_TRAY: RefCell<Option<OwnedTray>> = const { RefCell::new(None) };
}

struct OwnedTray {
    _icon: TrayIcon,
    guid: u128,
    open: MenuItem,
    exit: MenuItem,
}

#[derive(serde::Deserialize)]
struct Labels {
    open: String,
    exit: String,
}

fn labels(language: Language) -> anyhow::Result<Labels> {
    // Shared strict-JSON literal modules keep browser imports at ES2021.
    let source = match language {
        Language::ZhHans => include_str!("../../../../../src/shared/i18n/tray/zh.js"),
        Language::En => include_str!("../../../../../src/shared/i18n/tray/en.js"),
        Language::Ja => include_str!("../../../../../src/shared/i18n/tray/ja.js"),
    };
    let json = source
        .trim()
        .strip_prefix("export const trayLabels = ")
        .and_then(|source| source.strip_suffix(';'))
        .ok_or_else(|| anyhow::anyhow!("invalid tray translation module"))?;
    Ok(serde_json::from_str(json)?)
}

fn activates_main(event: &TrayIconEvent) -> bool {
    matches!(event, TrayIconEvent::Click {
        id, button: MouseButton::Left, button_state: MouseButtonState::Up, ..
    } if id.as_ref() == TRAY_ID)
}

pub(crate) fn install(app: &tauri::AppHandle, language: Language) {
    if let Err(error) = try_install(app, language) {
        log::warn!("system tray unavailable; keeping normal window close behavior: {error}");
    }
}

fn try_install(app: &tauri::AppHandle, language: Language) -> anyhow::Result<()> {
    let labels = labels(language)?;
    let open = MenuItem::with_id("tray-open", labels.open, true, None);
    let exit = MenuItem::with_id("tray-exit", labels.exit, true, None);
    let menu = Menu::new();
    menu.append_items(&[&open, &PredefinedMenuItem::separator(), &exit])?;
    let image = app
        .default_window_icon()
        .ok_or_else(|| anyhow::anyhow!("application icon is missing"))?;
    let icon = Icon::from_rgba(image.rgba().to_vec(), image.width(), image.height())?;
    let guid = icon_guid(std::env::current_exe()?.as_os_str());
    let icon = TrayIconBuilder::new()
        .with_id(TRAY_ID)
        .with_guid(guid)
        .with_icon(icon)
        .with_tooltip("Pinvou Agent")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    OWNED_TRAY.with(|slot| {
        *slot.borrow_mut() = Some(OwnedTray {
            _icon: icon,
            guid,
            open,
            exit,
        })
    });
    // Tauri forwards the unified tray-icon/muda event streams before its
    // per-resource lookup, so these public global callbacks also cover our
    // GUID-addressable native icon. No private Tauri/native IDs are read.
    app.on_tray_icon_event(|app, event| {
        if activates_main(&event) {
            show_main(app);
        }
    });
    app.on_menu_event(|app, event| match event.id.as_ref() {
        "tray-open" => show_main(app),
        "tray-exit" => app.exit(0),
        _ => {}
    });
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        crate::platform::window_startup::activate_main_window(window, "system_tray");
    }
}

pub(crate) fn refresh_language(app: &tauri::AppHandle, language: Language) {
    let Ok(labels) = labels(language) else {
        log::warn!("invalid system tray translations");
        return;
    };
    if let Err(error) = app.run_on_main_thread(move || {
        OWNED_TRAY.with(|slot| {
            if let Some(tray) = slot.borrow().as_ref() {
                tray.open.set_text(labels.open);
                tray.exit.set_text(labels.exit);
            }
        });
    }) {
        log::warn!("failed to dispatch system tray language update: {error}");
    }
}

pub(crate) fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        let available = window.label() == "main"
            && OWNED_TRAY.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .is_some_and(|tray| registered(tray.guid))
            });
        match super::close_policy::close_to_tray(window.label(), available, || window.hide()) {
            Ok(true) => api.prevent_close(),
            Ok(false) => {}
            Err(error) => log::warn!("failed to hide main window to system tray: {error}"),
        }
    }
}

/// Called on the UI thread by RunEvent::Exit; dropping the owner unregisters
/// the native icon and destroys its helper window/menu on the owning thread.
pub(crate) fn on_exit() {
    OWNED_TRAY.with(|slot| drop(slot.borrow_mut().take()));
}

pub(crate) async fn before_restart(app: &tauri::AppHandle) {
    // restart can bypass Exit. Wait for UI-thread removal before the caller
    // invokes it, rather than leaving an icon outside framework resources.
    let (removed, completed) = tokio::sync::oneshot::channel();
    if let Err(error) = app.run_on_main_thread(move || {
        on_exit();
        let _ = removed.send(());
    }) {
        log::warn!("failed to dispatch system tray removal before restart: {error}");
        return;
    }
    if completed.await.is_err() {
        log::warn!("system tray removal interrupted before restart");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owned_left_button_release_restores_the_main_window() {
        let event = |id: &str, button, button_state| TrayIconEvent::Click {
            id: id.into(),
            position: (0.0, 0.0).into(),
            rect: Default::default(),
            button,
            button_state,
        };
        assert!(activates_main(&event(
            TRAY_ID,
            MouseButton::Left,
            MouseButtonState::Up
        )));
        for (id, button, state) in [
            (TRAY_ID, MouseButton::Right, MouseButtonState::Up),
            (TRAY_ID, MouseButton::Middle, MouseButtonState::Up),
            (TRAY_ID, MouseButton::Left, MouseButtonState::Down),
            ("unrelated-tray", MouseButton::Left, MouseButtonState::Up),
        ] {
            assert!(!activates_main(&event(id, button, state)));
        }
    }

    #[test]
    fn native_menu_has_explicit_copy_for_every_supported_language() {
        for language in [Language::ZhHans, Language::En, Language::Ja] {
            let labels = labels(language).unwrap();
            assert!(!labels.open.is_empty());
            assert!(!labels.exit.is_empty());
        }
    }
}
