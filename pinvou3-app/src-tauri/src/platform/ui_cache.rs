//! Linux WebKit 前端缓存迁移。
//!
//! Tauri 的 app URL 在不同版本间保持不变。当前端从旧的静态 HTML/CSS 迁移到
//! Vite bundle 后，WebKit 可能继续复用旧 `index.html`，但旧 HTML 引用的
//! `styles/chat.css` 已不在新包内，最终把所有页面以裸 DOM 一起显示。

#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

/// 只在前端资源结构发生不兼容变化时升级，不跟随每个应用 patch 版本递增。
pub(crate) const UI_CACHE_SCHEMA: &str = "vite-react-1";

#[cfg(target_os = "linux")]
const LINUX_UI_ENV_DEFAULTS: &[(&str, &str)] = &[
    ("GDK_BACKEND", "x11"),
    ("GTK_IM_MODULE", "fcitx"),
    ("QT_IM_MODULE", "fcitx"),
    ("XMODIFIERS", "@im=fcitx"),
];

#[cfg(target_os = "linux")]
const WEBKIT_RENDERING_OVERRIDE_KEYS: &[&str] = &[
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "WEBKIT_DMABUF_RENDERER_FORCE_SHM",
    "WEBKIT_FORCE_DMABUF_RENDERER",
    "WEBKIT_WEB_RENDER_DEVICE_FILE",
];

/// Configure OS desktop variables before Tauri creates the first WebView.
pub(crate) fn configure_runtime_environment() {
    #[cfg(target_os = "windows")]
    if std::env::var_os("HOME").is_none() {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            std::env::set_var("HOME", profile);
        }
    }

    #[cfg(target_os = "linux")]
    {
        for (key, value) in LINUX_UI_ENV_DEFAULTS {
            if std::env::var_os(key).is_none() {
                std::env::set_var(key, value);
            }
        }
        let has_override = WEBKIT_RENDERING_OVERRIDE_KEYS
            .iter()
            .any(|key| std::env::var_os(key).is_some());
        let is_arm64_nvidia =
            cfg!(target_arch = "aarch64") && Path::new("/proc/driver/nvidia/version").is_file();
        if should_force_webkit_dmabuf_shm(is_arm64_nvidia, has_override) {
            std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1");
        }
    }
}

#[cfg(target_os = "linux")]
fn should_force_webkit_dmabuf_shm(
    is_linux_arm64_nvidia: bool,
    has_explicit_rendering_override: bool,
) -> bool {
    is_linux_arm64_nvidia && !has_explicit_rendering_override
}

#[cfg(target_os = "linux")]
const APP_IDENTIFIER: &str = "com.pinvou.pinvou3";
#[cfg(target_os = "linux")]
const MARKER_FILE: &str = ".ui-cache-schema";

#[cfg(target_os = "linux")]
fn linux_app_data_dir() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(data_home.join(APP_IDENTIFIER))
}

/// 在 WebView 创建前执行一次缓存结构迁移。
///
/// 只删除可重建的 `WebKitCache`；localStorage、IndexedDB、日志以及
/// `~/.pinvou3` 运行时数据均不在清理范围内。
#[cfg(target_os = "linux")]
pub(crate) fn migrate_before_webview() {
    let Some(app_data_dir) = linux_app_data_dir() else {
        eprintln!("[pinvou3-app] UI cache migration skipped: HOME/XDG_DATA_HOME unavailable");
        return;
    };

    match migrate_at(&app_data_dir) {
        Ok(true) => eprintln!("[pinvou3-app] WebKit UI cache migrated to schema {UI_CACHE_SCHEMA}"),
        Ok(false) => {}
        Err(error) => eprintln!(
            "[pinvou3-app] WebKit UI cache migration failed (entry URL remains versioned): {error}"
        ),
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn migrate_before_webview() {}

#[cfg(target_os = "linux")]
fn migrate_at(app_data_dir: &Path) -> std::io::Result<bool> {
    let marker = app_data_dir.join(MARKER_FILE);
    if std::fs::read_to_string(&marker)
        .map(|value| value.trim() == UI_CACHE_SCHEMA)
        .unwrap_or(false)
    {
        return Ok(false);
    }

    let webkit_cache = app_data_dir.join("WebKitCache");
    if webkit_cache.exists() {
        std::fs::remove_dir_all(&webkit_cache)?;
    }

    std::fs::create_dir_all(app_data_dir)?;
    let marker_tmp = app_data_dir.join(format!("{MARKER_FILE}.tmp"));
    std::fs::write(&marker_tmp, format!("{UI_CACHE_SCHEMA}\n"))?;
    std::fs::rename(marker_tmp, marker)?;
    Ok(true)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pinvou3-ui-cache-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn incompatible_schema_removes_only_webkit_cache() {
        let root = test_root("migrate");
        std::fs::create_dir_all(root.join("WebKitCache/Version 17")).unwrap();
        std::fs::write(root.join("WebKitCache/Version 17/old-index"), "stale").unwrap();
        std::fs::create_dir_all(root.join("localstorage")).unwrap();
        std::fs::write(root.join("localstorage/state"), "keep").unwrap();
        std::fs::write(root.join(MARKER_FILE), "legacy-html\n").unwrap();

        assert!(migrate_at(&root).unwrap());
        assert!(!root.join("WebKitCache").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("localstorage/state")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(MARKER_FILE))
                .unwrap()
                .trim(),
            UI_CACHE_SCHEMA
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_schema_preserves_existing_cache() {
        let root = test_root("current");
        std::fs::create_dir_all(root.join("WebKitCache")).unwrap();
        std::fs::write(root.join("WebKitCache/current"), "keep").unwrap();
        std::fs::write(root.join(MARKER_FILE), format!("{UI_CACHE_SCHEMA}\n")).unwrap();

        assert!(!migrate_at(&root).unwrap());
        assert_eq!(
            std::fs::read_to_string(root.join("WebKitCache/current")).unwrap(),
            "keep"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_main_url_uses_same_cache_schema() {
        let config = include_str!("../../tauri.conf.json");
        assert!(config.contains(&format!("\"identifier\": \"{APP_IDENTIFIER}\"")));
        assert!(config.contains(&format!("index.html?ui={UI_CACHE_SCHEMA}")));
    }

    #[test]
    fn arm64_nvidia_uses_shm_without_disabling_compositing() {
        assert!(should_force_webkit_dmabuf_shm(true, false));
        assert!(!should_force_webkit_dmabuf_shm(true, true));
        assert!(!should_force_webkit_dmabuf_shm(false, false));
        assert!(!LINUX_UI_ENV_DEFAULTS
            .iter()
            .any(|(key, _)| *key == "WEBKIT_DISABLE_COMPOSITING_MODE"));
    }
}

#[cfg(test)]
mod schema_tests {
    use super::UI_CACHE_SCHEMA;

    // 钉住 schema 值本身：它同时拼进主窗口与撕离窗口的入口 URL（见
    // pet/platform/detach.rs 与 tauri.conf.json），改动必须有意识。
    #[test]
    fn ui_cache_schema_is_pinned() {
        assert_eq!(UI_CACHE_SCHEMA, "vite-react-1");
    }
}
