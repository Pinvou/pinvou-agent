use std::collections::HashSet;

use parking_lot::Mutex;
use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::platform::prefs::{Language, NotificationPrefs, UserPrefs};

// Desktop notifications carry no click action (the native builder registers
// no on_action, and notify-send structurally reports none), so the copy must
// not promise click behavior in any language.
//
// This table is intentionally separate from `src/shared/i18n.js`: i18n.js
// lives inside the webview bundle, while these notifications are raised by
// the Rust host process — the engine's wait-for-input path fires even when
// the window is closed and the webview never loaded, so the host cannot
// consume webview-bundle resources at send time. Keeping the surface
// minimal: one title/body per kind per saved UI language (`UserPrefs::load`,
// the same language source the system tray localizes from), zh copy pinned
// byte-identical to the pre-localization strings by
// `notification_copy_follows_saved_language`. The in-app activity labels
// (`activityInputNeeded` / `activityTaskDone` in i18n) are a separate,
// shorter UI surface and are deliberately not reused here.
const TASK_COMPLETED_TITLE_ZH: &str = "任务已完成";
const TASK_COMPLETED_BODY_ZH: &str = "任务已成功完成。您可以在编辑器中查看结果。";
const TASK_COMPLETED_TITLE_EN: &str = "Task completed";
const TASK_COMPLETED_BODY_EN: &str =
    "The task completed successfully. You can review the results in the editor.";
const TASK_COMPLETED_TITLE_JA: &str = "タスクが完了しました";
const TASK_COMPLETED_BODY_JA: &str = "タスクが正常に完了しました。エディターで結果を確認できます。";
const INPUT_NEEDED_TITLE_ZH: &str = "需要你的输入";
const INPUT_NEEDED_BODY_ZH: &str = "任务正在等待你的回复，请打开品悟继续。";
const INPUT_NEEDED_TITLE_EN: &str = "Input needed";
const INPUT_NEEDED_BODY_EN: &str = "The task is waiting for your reply. Open Pinvou to continue.";
const INPUT_NEEDED_TITLE_JA: &str = "入力が必要です";
const INPUT_NEEDED_BODY_JA: &str = "タスクが返信を待っています。Pinvou を開いて続行してください。";

/// Copy per notification kind, keyed by the saved UI language (`UserPrefs::load`,
/// the same source the system tray localizes from). Resolved at send time so a
/// language switch takes effect without a restart.
fn task_completed_copy(language: Language) -> (&'static str, &'static str) {
    match language {
        Language::ZhHans => (TASK_COMPLETED_TITLE_ZH, TASK_COMPLETED_BODY_ZH),
        Language::En => (TASK_COMPLETED_TITLE_EN, TASK_COMPLETED_BODY_EN),
        Language::Ja => (TASK_COMPLETED_TITLE_JA, TASK_COMPLETED_BODY_JA),
    }
}

fn input_needed_copy(language: Language) -> (&'static str, &'static str) {
    match language {
        Language::ZhHans => (INPUT_NEEDED_TITLE_ZH, INPUT_NEEDED_BODY_ZH),
        Language::En => (INPUT_NEEDED_TITLE_EN, INPUT_NEEDED_BODY_EN),
        Language::Ja => (INPUT_NEEDED_TITLE_JA, INPUT_NEEDED_BODY_JA),
    }
}

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
    notifications: NotificationPrefs,
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
    // One effective rule for delivery and the Settings display: the UI derives
    // the input row from the master switch (the task row backfills as
    // `enabled && task_completed`, and the input row shows
    // `needs_input && task_completed`), so delivery must require all three.
    // The supported CLI can persist `enabled=true, task_completed=false`, and
    // on Windows/macOS an older file in that state defaults `needs_input` to
    // true — there the row displays off while a two-field gate would still
    // deliver. Stored choices stay untouched; only delivery agrees with what
    // the switch shows.
    prefs.notifications.enabled
        && prefs.notifications.task_completed
        && prefs.notifications.needs_input
}

fn saved_language() -> Language {
    UserPrefs::load().language
}

pub fn notify_task_completed(app: &AppHandle) {
    let (title, body) = task_completed_copy(saved_language());
    notify_desktop(app, title, body, "task completion");
}

pub fn notify_input_needed(app: &AppHandle) {
    let (title, body) = input_needed_copy(saved_language());
    notify_desktop(app, title, body, "input needed");
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

            // Master switch off must not deliver (mirrors the
            // task_completion_enabled sibling case).
            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":false,"needs_input":true}}"#,
            )
            .unwrap();
            assert!(!needs_input_enabled());

            std::fs::write(&settings, r#"{"notifications":{"enabled":true}}"#).unwrap();
            // A missing needs_input is filled from the struct Default (off on
            // Linux, on on macOS/Windows): pins the Linux opt-in default so it
            // cannot drift to always-on.
            assert_eq!(needs_input_enabled(), !cfg!(target_os = "linux"));

            std::fs::write(&settings, r#"{"notifications":{"needs_input":true}}"#).unwrap();
            // A missing enabled is filled from the struct Default (off on
            // Linux, on on macOS/Windows).
            assert_eq!(needs_input_enabled(), !cfg!(target_os = "linux"));

            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"task_completed":true,"needs_input":true}}"#,
            )
            .unwrap();
            assert!(needs_input_enabled());

            // The Settings row derives its display from the master switch, so
            // the CLI-authored split (master off, input intent on) must not
            // deliver either — including the legacy-file shape where the
            // missing needs_input defaults to true on Windows/macOS.
            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"task_completed":false,"needs_input":true}}"#,
            )
            .unwrap();
            assert!(!needs_input_enabled());
            std::fs::write(
                &settings,
                r#"{"notifications":{"enabled":true,"task_completed":false}}"#,
            )
            .unwrap();
            assert!(!needs_input_enabled());
        });
    }

    #[test]
    fn notification_copy_follows_saved_language() {
        with_temp_pinvou3_home("notification-language", |root| {
            let settings = root.join("settings.json");
            // zh copy is byte-identical to the pre-localization strings, so
            // zh users see zero behavior change.
            for (tag, input, completed) in [
                (
                    "zh-Hans",
                    ("需要你的输入", "任务正在等待你的回复，请打开品悟继续。"),
                    ("任务已完成", "任务已成功完成。您可以在编辑器中查看结果。"),
                ),
                (
                    "en",
                    (
                        "Input needed",
                        "The task is waiting for your reply. Open Pinvou to continue.",
                    ),
                    (
                        "Task completed",
                        "The task completed successfully. You can review the results in the editor.",
                    ),
                ),
                (
                    "ja",
                    (
                        "入力が必要です",
                        "タスクが返信を待っています。Pinvou を開いて続行してください。",
                    ),
                    (
                        "タスクが完了しました",
                        "タスクが正常に完了しました。エディターで結果を確認できます。",
                    ),
                ),
            ] {
                std::fs::write(&settings, format!(r#"{{"language":"{tag}"}}"#)).unwrap();
                let language = saved_language();
                assert_eq!(
                    input_needed_copy(language),
                    input,
                    "{tag} input-needed copy"
                );
                assert_eq!(
                    task_completed_copy(language),
                    completed,
                    "{tag} completion copy"
                );
            }
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
