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
        let mut turns = self.notified_turns.lock();
        // 同 key 重复（终态事件重放）先短路：否则下方前缀清理会把这条本身
        // 也清掉、再插回时误报"首次"。
        if turns.contains(&key) {
            return false;
        }
        // key 形如 `{session_id}:{turn_id}`：同一 session 只需保留最新一条去重
        // 记录（重复终态事件总是紧跟同一 turn）。不清前缀会让集合随完成轮数
        // 无界增长。
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

    crate::platform::os::spawn_detached_and_reap(
        Command::new("notify-send")
            .arg(TASK_COMPLETED_TITLE)
            .arg(TASK_COMPLETED_BODY),
    )
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
        assert!(!state.should_notify("session:next-turn".to_string()));
        // next-turn 落位后同 session 只保留最新一条：旧 turn key 已按前缀淘汰
        // （集合有界），其重现按新事件处理——重复终态事件总是紧跟同一 turn，
        // 跨 turn 重现的旧 key 不存在真实的去重需求。
        assert!(state.should_notify("session:turn".to_string()));
        // 其他 session 不受影响。
        assert!(state.should_notify("other:turn".to_string()));
    }
}
