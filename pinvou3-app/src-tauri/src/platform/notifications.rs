use std::collections::HashSet;

use parking_lot::Mutex;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

const TASK_COMPLETED_TITLE: &str = "任务已完成";
const TASK_COMPLETED_BODY: &str = "任务已成功完成。您可以在编辑器中查看结果。";
const INPUT_NEEDED_TITLE: &str = "需要你的输入";
const INPUT_NEEDED_BODY: &str = "任务正在等待你的回复，点击打开品悟继续。";

#[derive(Default)]
pub struct NotificationState {
    notified_turns: Mutex<HashSet<String>>,
}

impl NotificationState {
    pub fn should_notify(&self, key: String) -> bool {
        let mut turns = self.notified_turns.lock();
        // A duplicate with the same key (terminal-event replay)
        // short-circuits first: otherwise the prefix cleanup below would
        // remove this very entry and re-inserting it would be misreported
        // as "first".
        if turns.contains(&key) {
            return false;
        }
        // Keys look like `{session_id}:{turn_id}`: only the newest dedup
        // record per session needs to survive (duplicate terminal events
        // always follow the same turn). Without the prefix cleanup the set
        // would grow unbounded with completed turns.
        let session_prefix = match key.split_once(':') {
            Some((session, _)) => format!("{session}:"),
            None => String::new(),
        };
        if !session_prefix.is_empty() {
            turns.retain(|existing| !existing.starts_with(&session_prefix));
        }
        turns.insert(key)
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct NotificationPrefsOnly {
    notifications: crate::platform::prefs::NotificationPrefs,
}

fn read_notification_prefs() -> NotificationPrefsOnly {
    std::fs::read_to_string(crate::platform::paths::settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<NotificationPrefsOnly>(&raw).ok())
        .unwrap_or_default()
}

pub fn task_completion_enabled() -> bool {
    let prefs = read_notification_prefs();
    prefs.notifications.enabled && prefs.notifications.task_completed
}

pub fn needs_input_enabled() -> bool {
    let prefs = read_notification_prefs();
    prefs.notifications.enabled && prefs.notifications.needs_input
}

pub fn notify_task_completed(app: &AppHandle) {
    notify_desktop(
        app,
        TASK_COMPLETED_TITLE,
        TASK_COMPLETED_BODY,
        "task completion",
    );
}

pub fn notify_input_needed(app: &AppHandle) {
    notify_desktop(app, INPUT_NEEDED_TITLE, INPUT_NEEDED_BODY, "input needed");
}

fn notify_desktop(app: &AppHandle, title: &str, body: &str, kind: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = send_notify_send(title, body) {
            eprintln!(
                "[pinvou3-app] notify-send {kind} notification failed: {e}; trying native notification"
            );
            if let Err(native) = send_native_notification(app, title, body) {
                eprintln!("[pinvou3-app] native {kind} notification failed: {native}");
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Err(e) = send_native_notification(app, title, body) {
            eprintln!("[pinvou3-app] native {kind} notification failed: {e}");
        }
    }
}

fn send_native_notification(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn send_notify_send(title: &str, body: &str) -> Result<(), String> {
    use std::process::Command;

    crate::platform::os::spawn_detached_and_reap(Command::new("notify-send").arg(title).arg(body))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 不再自建局部 ENV_LOCK:改借 crate 级唯一的 bridge::paths::tests::ENV_LOCK,
    // 与所有 mutate PINVOU3_HOME 的测试共享同一把锁,消除并发数据竞争。

    fn with_temp_pinvou3_home(test_name: &str, f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let old_home = std::env::var_os("PINVOU3_HOME");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-notification-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
        unsafe { std::env::set_var("PINVOU3_HOME", &root) };
        f(root.clone());
        match old_home {
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            Some(value) => unsafe { std::env::set_var("PINVOU3_HOME", value) },
            // SAFETY: holding platform::paths::tests::ENV_LOCK; env writes serialized in-process.
            None => unsafe { std::env::remove_var("PINVOU3_HOME") },
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn task_completion_enabled_follows_settings_switch() {
        with_temp_pinvou3_home("settings-switch", |root| {
            let settings = root.join("settings.json");

            assert_eq!(task_completion_enabled(), !cfg!(target_os = "linux"));

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"task_completed":false}}"#,
            )
            .unwrap();
            assert!(!task_completion_enabled());

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":false,"task_completed":true}}"#,
            )
            .unwrap();
            assert!(!task_completion_enabled());

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"task_completed":true}}"#,
            )
            .unwrap();
            assert!(task_completion_enabled());
        });
    }

    #[test]
    fn needs_input_enabled_follows_settings_switch() {
        with_temp_pinvou3_home("needs-input-switch", |root| {
            let settings = root.join("settings.json");

            assert_eq!(needs_input_enabled(), !cfg!(target_os = "linux"));

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"needs_input":false}}"#,
            )
            .unwrap();
            assert!(!needs_input_enabled());

            std::fs::write(&settings, r#"{"notifications":{"needs_input":true}}"#).unwrap();
            // 缺失的 enabled 从结构体 Default 补齐（Linux 默认关、macOS/Windows 默认开）。
            assert_eq!(needs_input_enabled(), !cfg!(target_os = "linux"));

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"needs_input":true}}"#,
            )
            .unwrap();
            assert!(needs_input_enabled());
        });
    }

    #[test]
    fn notification_state_deduplicates_turn_keys() {
        let state = NotificationState::default();
        assert!(state.should_notify("session:turn".to_string()));
        assert!(!state.should_notify("session:turn".to_string()));
        assert!(state.should_notify("session:next-turn".to_string()));
        assert!(!state.should_notify("session:next-turn".to_string()));
        // Once the next turn lands, only the newest record per session
        // survives: the old turn's key was already pruned by prefix (the
        // set is bounded) and its reappearance is treated as a new event —
        // duplicate terminal events always follow the same turn, so an old
        // key reappearing across turns has no real dedup need.
        assert!(state.should_notify("session:turn".to_string()));
        // Other sessions are unaffected.
        assert!(state.should_notify("other:turn".to_string()));
    }
}
