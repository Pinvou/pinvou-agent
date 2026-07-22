use std::collections::HashSet;

use parking_lot::Mutex;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

const TASK_COMPLETED_TITLE: &str = "任务已完成";
const TASK_COMPLETED_BODY: &str = "任务已成功完成。您可以在编辑器中查看结果。";

#[derive(Default)]
pub struct NotificationState {
    notified_turns: Mutex<HashSet<String>>,
}

impl NotificationState {
    pub fn should_notify(&self, key: String) -> bool {
        self.notified_turns.lock().insert(key)
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct NotificationPrefsOnly {
    notifications: crate::platform::prefs::NotificationPrefs,
}

pub fn task_completion_enabled() -> bool {
    let prefs = std::fs::read_to_string(crate::platform::paths::settings_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<NotificationPrefsOnly>(&raw).ok())
        .unwrap_or_default();
    prefs.notifications.enabled && prefs.notifications.task_completed
}

pub fn notify_task_completed(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = send_notify_send() {
            eprintln!(
                "[pinvou3-app] notify-send task completion notification failed: {e}; trying native notification"
            );
            if let Err(native) = send_native_notification(app) {
                eprintln!("[pinvou3-app] native task completion notification failed: {native}");
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        if let Err(e) = send_native_notification(app) {
            eprintln!("[pinvou3-app] native task completion notification failed: {e}");
        }
    }
}

fn send_native_notification(app: &AppHandle) -> Result<(), String> {
    app.notification()
        .builder()
        .title(TASK_COMPLETED_TITLE)
        .body(TASK_COMPLETED_BODY)
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn send_notify_send() -> Result<(), String> {
    use std::process::Command;

    Command::new("notify-send")
        .arg(TASK_COMPLETED_TITLE)
        .arg(TASK_COMPLETED_BODY)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn with_temp_pinvou3_home(test_name: &str, f: impl FnOnce(std::path::PathBuf)) {
        let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let old_home = std::env::var_os("PINVOU3_HOME");
        let root = std::env::temp_dir().join(format!(
            "pinvou3-notification-test-{test_name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("PINVOU3_HOME", &root);
        f(root.clone());
        match old_home {
            Some(value) => std::env::set_var("PINVOU3_HOME", value),
            None => std::env::remove_var("PINVOU3_HOME"),
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
    fn notification_state_deduplicates_turn_keys() {
        let state = NotificationState::default();
        assert!(state.should_notify("session:turn".to_string()));
        assert!(!state.should_notify("session:turn".to_string()));
        assert!(state.should_notify("session:next-turn".to_string()));
    }
}
