#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopCapabilities {
    pub(crate) show_super_permission_settings: bool,
    pub(crate) uses_bundled_dependency_installer: bool,
    pub(crate) uses_homebrew_dependency_installer: bool,
    pub(crate) task_completion_notifications_default: bool,
    pub(crate) local_vllm_supported: bool,
    pub(crate) codex_acp_supported: bool,
    pub(crate) browser_native_display: bool,
    pub(crate) browser_agent_automation: bool,
    /// Native keyboard hook for the global Alt voice shortcut: implemented on
    /// Windows only (Alt+Space is intercepted by the system menu, so a native
    /// hook is required); on other platforms the native layer is a no-op and
    /// only the in-window Alt fallback is available.
    pub(crate) voice_shortcut_native: bool,
    /// The Linux WebView (WebKitGTK) carries no image data to the page in
    /// paste events (an image clipboard surfaces an empty clipboardData), so
    /// composer pastes fall back to a native clipboard read; the macOS/Windows
    /// WebViews carry image items natively and need no fallback.
    pub(crate) paste_image_clipboard_read: bool,
    /// System-wide shortcut registration (`tauri-plugin-global-shortcut`):
    /// works on Windows and macOS; on Linux the underlying X11 key grabs only
    /// reach the compositor's input stack on a real X11 session, so Wayland
    /// sessions (including XWayland, whose X server sees no system-wide keys)
    /// must not offer the feature.
    pub(crate) global_shortcuts_available: bool,
}

fn browser_product_enabled_for(os: &str) -> bool {
    matches!(os, "windows" | "linux" | "macos")
}

/// Single semantic product gate for the embedded browser. Runtime MCP, the native workspace,
/// and public capabilities must consume this function together, preventing a half-enabled
/// state where the Agent has tools but the user has no visible surface. All three desktop
/// platforms ship the browser; a platform that cannot display or automate it must be hidden
/// here rather than half-registered anywhere downstream.
pub(crate) fn browser_product_enabled() -> bool {
    browser_product_enabled_for(std::env::consts::OS)
}

pub(crate) fn current() -> DesktopCapabilities {
    let global_shortcuts_available =
        global_shortcuts_available_with(&|key| std::env::var(key).ok(), std::env::consts::OS);
    // Ambiguous XWayland signal: the one available-resolution that can still
    // yield a partially-dead registration (grabs fire only while an X11
    // window has focus). Warn once for diagnosis; the pure decision helpers
    // above stay side-effect free.
    if global_shortcut_session_is_ambiguous(&|key| std::env::var(key).ok(), std::env::consts::OS) {
        static AMBIGUOUS_SESSION_WARNED: std::sync::Once = std::sync::Once::new();
        AMBIGUOUS_SESSION_WARNED.call_once(|| {
            log::warn!(
                "XDG_SESSION_TYPE=x11 with WAYLAND_DISPLAY set: global shortcuts will \
                 only fire while an X11 window has focus"
            );
        });
    }
    DesktopCapabilities {
        show_super_permission_settings: cfg!(target_os = "linux"),
        uses_bundled_dependency_installer: cfg!(target_os = "windows"),
        // macOS 用 Homebrew 安装依赖(对称 Windows 的 uses_bundled_dependency_installer),
        // 让前端按语义能力选择 Homebrew 专属文案,而非裸判 os 字符串。
        uses_homebrew_dependency_installer: cfg!(target_os = "macos"),
        task_completion_notifications_default: !cfg!(target_os = "linux"),
        local_vllm_supported: cfg!(target_os = "linux"),
        codex_acp_supported: supports_codex_acp(std::env::consts::OS),
        // Enable display and Agent automation atomically. Separate platform cfg declarations
        // could give the model tools while the user cannot see the same page. macOS
        // acceptance builds use this same semantic helper.
        browser_native_display: browser_product_enabled(),
        browser_agent_automation: browser_product_enabled(),
        voice_shortcut_native: cfg!(target_os = "windows"),
        paste_image_clipboard_read: cfg!(target_os = "linux"),
        global_shortcuts_available,
    }
}

fn supports_codex_acp(os: &str) -> bool {
    matches!(os, "linux" | "windows" | "macos")
}

/// Whether the OS session allows system-wide shortcut registration, decided
/// from an injectable environment lookup so tests can feed fake sessions.
fn global_shortcuts_available_with(env: &dyn Fn(&str) -> Option<String>, os: &str) -> bool {
    match os {
        "windows" | "macos" => true,
        "linux" => linux_session_allows_global_shortcuts(env),
        _ => false,
    }
}

/// Non-empty, trimmed environment lookup; mirrors the reader used by the
/// computer-use session detection.
fn session_env(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    env(key).filter(|value| !value.trim().is_empty())
}

/// Linux session gate for global shortcuts: available on a real X11 session,
/// unavailable on Wayland (XWayland's X server receives no system-wide keys,
/// so X11 key grabs silently do nothing there) and on sessions with no
/// graphical signal at all.
///
/// The env-var precedence mirrors `detect_session` in
/// features/computer_use/platform/linux.rs (XDG_SESSION_TYPE first, then
/// WAYLAND_DISPLAY — its strong and weak corroboration arms both resolve to
/// Wayland — and DISPLAY only as the last fallback). One deliberate
/// divergence, per the feature contract: `DISPLAY` alone does NOT imply X11
/// (XWayland also sets it, and a session that reports neither a session type
/// nor a Wayland socket cannot be told apart from XWayland), so the bare
/// DISPLAY arm resolves to unavailable instead of X11 — a false negative on an
/// exotic bare-X11 environment beats a silently dead registration on the
/// common XWayland case.
fn linux_session_allows_global_shortcuts(env: &dyn Fn(&str) -> Option<String>) -> bool {
    // Precedence mirrors detect_session: XDG_SESSION_TYPE is the primary
    // signal, WAYLAND_DISPLAY the fallback (its strong and weak corroboration
    // arms both resolve to Wayland), DISPLAY only as the last resort.
    let session_type =
        session_env(env, "XDG_SESSION_TYPE").map(|value| value.trim().to_ascii_lowercase());
    if session_type.as_deref() == Some("x11") {
        return true;
    }
    if session_env(env, "WAYLAND_DISPLAY").is_some() {
        // Wayland socket, whether or not XDG_RUNTIME_DIR corroborates it and
        // whether or not the session type var was set/unrecognized.
        return false;
    }
    // Remaining resolutions ("wayland" session type without a socket, "tty",
    // unrecognized, unset, bare DISPLAY, nothing): without an explicit
    // XDG_SESSION_TYPE=x11 there is no confirmed X11 session, and DISPLAY
    // alone is ambiguous (XWayland sets it too) — keep the feature off.
    false
}

/// An `XDG_SESSION_TYPE=x11` session that also carries a Wayland socket: the
/// only available-resolution where a registration can be partially dead
/// (X11 grabs fire only while an X11 window has focus). Pure, so tests can
/// pin it; `current()` warns once when this resolves true.
fn global_shortcut_session_is_ambiguous(env: &dyn Fn(&str) -> Option<String>, os: &str) -> bool {
    os == "linux"
        && global_shortcuts_available_with(env, os)
        && session_env(env, "WAYLAND_DISPLAY").is_some()
}

pub(crate) const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub(crate) const fn is_musl() -> bool {
    cfg!(all(target_os = "linux", target_env = "musl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_acp_is_available_on_desktop_platforms() {
        assert!(supports_codex_acp("linux"));
        assert!(supports_codex_acp("windows"));
        assert!(supports_codex_acp("macos"));
        assert!(!supports_codex_acp("android"));
    }

    #[test]
    fn browser_product_gate_matches_desktop_platforms() {
        assert!(browser_product_enabled_for("windows"));
        assert!(browser_product_enabled_for("linux"));
        assert!(browser_product_enabled_for("macos"));
        assert!(!browser_product_enabled_for("android"));
    }

    /// Fake environment: a fixed key→value table over the injectable lookup.
    fn fake_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn global_shortcuts_track_the_os_before_the_session() {
        let empty = fake_env(&[]);
        assert!(global_shortcuts_available_with(&empty, "windows"));
        assert!(global_shortcuts_available_with(&empty, "macos"));
        // Unsupported OS strings stay unavailable even with an X11 session.
        assert!(!global_shortcuts_available_with(&empty, "android"));
        let x11 = fake_env(&[("XDG_SESSION_TYPE", "x11")]);
        assert!(!global_shortcuts_available_with(&x11, "android"));
    }

    #[test]
    fn linux_x11_session_allows_global_shortcuts() {
        let env = fake_env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]);
        assert!(linux_session_allows_global_shortcuts(&env));
        // Case-insensitive, whitespace-tolerant session type.
        let padded = fake_env(&[("XDG_SESSION_TYPE", " X11 ")]);
        assert!(linux_session_allows_global_shortcuts(&padded));
    }

    #[test]
    fn linux_wayland_session_denies_global_shortcuts() {
        // Plain Wayland session with its socket and DISPLAY (XWayland).
        let env = fake_env(&[
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("DISPLAY", ":0"),
        ]);
        assert!(!linux_session_allows_global_shortcuts(&env));
        // Session type says Wayland without any WAYLAND_DISPLAY socket.
        let type_only = fake_env(&[("XDG_SESSION_TYPE", "wayland")]);
        assert!(!linux_session_allows_global_shortcuts(&type_only));
        // Fallback arm: unset session type, Wayland socket + runtime dir.
        let socket_only = fake_env(&[
            ("WAYLAND_DISPLAY", "wayland-1"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        assert!(!linux_session_allows_global_shortcuts(&socket_only));
        // Weak corroboration (no XDG_RUNTIME_DIR) is still Wayland, mirroring
        // detect_session.
        let weak_socket = fake_env(&[("WAYLAND_DISPLAY", "wayland-1")]);
        assert!(!linux_session_allows_global_shortcuts(&weak_socket));
    }

    #[test]
    fn linux_bare_display_does_not_imply_x11() {
        // DISPLAY alone is ambiguous (XWayland also sets it): the feature
        // contract resolves it to unavailable instead of X11 (deliberate
        // divergence from detect_session's bare-DISPLAY arm).
        let display_only = fake_env(&[("DISPLAY", ":0")]);
        assert!(!linux_session_allows_global_shortcuts(&display_only));
        // Non-graphical and signal-less sessions stay unavailable too.
        let tty = fake_env(&[("XDG_SESSION_TYPE", "tty"), ("DISPLAY", ":0")]);
        assert!(!linux_session_allows_global_shortcuts(&tty));
        let empty = fake_env(&[]);
        assert!(!linux_session_allows_global_shortcuts(&empty));
    }

    #[test]
    fn only_x11_with_a_wayland_socket_is_ambiguous() {
        // The one available-resolution that can yield a partially-dead
        // registration: session type says X11 while a Wayland socket is set.
        let ambiguous = fake_env(&[
            ("XDG_SESSION_TYPE", "x11"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ]);
        assert!(global_shortcut_session_is_ambiguous(&ambiguous, "linux"));
        let padded_socket = fake_env(&[("XDG_SESSION_TYPE", "x11"), ("WAYLAND_DISPLAY", " ")]);
        assert!(
            !global_shortcut_session_is_ambiguous(&padded_socket, "linux"),
            "a whitespace-only socket is filtered by the non-empty reader"
        );
        // An honest X11 session or an unavailable Wayland session is not
        // ambiguous, and neither is a socket on a non-Linux OS.
        let plain_x11 = fake_env(&[("XDG_SESSION_TYPE", "x11")]);
        assert!(!global_shortcut_session_is_ambiguous(&plain_x11, "linux"));
        let wayland = fake_env(&[
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ]);
        assert!(!global_shortcut_session_is_ambiguous(&wayland, "linux"));
        let empty = fake_env(&[]);
        assert!(!global_shortcut_session_is_ambiguous(&empty, "macos"));
    }
}
