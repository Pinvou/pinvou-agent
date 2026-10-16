use super::prelude::*;

use std::sync::Mutex;

/// Wire result of `set_global_shortcut_config` (camelCase on the wire).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GlobalShortcutApplyResult {
    /// "registered" | "disabled" | "invalid" | "conflict".
    pub status: String,
    /// Canonical accelerator form when the shortcut is active; None when it
    /// is not (invalid / disabled / conflict).
    pub accelerator: Option<String>,
    /// Human-readable detail for invalid/conflict/unavailable outcomes.
    pub message: Option<String>,
}

impl GlobalShortcutApplyResult {
    fn from_outcome(outcome: crate::features::global_shortcut::ApplyOutcome) -> Self {
        Self {
            status: outcome.status.as_str().to_string(),
            accelerator: outcome.accelerator,
            message: outcome.message,
        }
    }

    fn invalid(message: String) -> Self {
        Self {
            status: "invalid".to_string(),
            accelerator: None,
            message: Some(message),
        }
    }
}

/// Serializes configure commands against each other: the apply (plugin
/// register/unregister) and the settings persist below must be atomic per
/// command, or two racing invokes could interleave apply A → apply B →
/// persist B → persist A and leave settings.json describing a config the
/// plugin never applied — [`crate::features::global_shortcut`]'s REGISTERED
/// state serializes apply entry only, not the apply→persist pair. The startup
/// replay is not a user of this lock — the event loop cannot dispatch webview
/// invokes until `lib.rs` setup returns, so the replay can never overlap a
/// command invoke; that lifecycle claim is the only reason the replay needs
/// no lock here.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

fn config_lock() -> std::sync::MutexGuard<'static, ()> {
    // A panic while holding the lock must not brick the feature for the rest
    // of the process: recover the guard (same style as the feature's
    // REGISTERED guard).
    CONFIG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether an apply outcome warrants persisting the requested config:
/// `Registered` and `Disabled` are the outcomes where the request was
/// accepted (applied, or already in place), so settings.json must describe
/// it; `Invalid` and `Conflict` changed nothing (a bad parse, or another
/// application owns the combination), so prefs must keep describing what is
/// actually still active. `Disabled` additionally covers an unavailable
/// session: the request was refused before the plugin was touched and
/// nothing became active — persisting `enabled` there only records the
/// user's intent, and replays re-check availability before acting on it.
fn should_persist(status: crate::features::global_shortcut::ApplyStatus) -> bool {
    use crate::features::global_shortcut::ApplyStatus;
    matches!(status, ApplyStatus::Registered | ApplyStatus::Disabled)
}

/// Configure the system-wide bring-to-front shortcut (opt-in, OFF by
/// default): validate the accelerator, apply the registration change, then
/// persist both fields.
///
/// The whole body runs as ONE blocking-worker join (`join_blocking`, the
/// settings-family convention — the settings transaction holds the
/// cross-process settings.json.lock and must not park a runtime worker),
/// serialized on [`CONFIG_LOCK`], so apply → persist is atomic per command.
///
/// Ordering and failure semantics:
/// - when enabling, a parse failure returns `invalid` and touches neither
///   the plugin nor prefs (the currently active shortcut keeps working);
///   when disabling, unparsable input is tolerated (the feature parses
///   lazily) and the persist keeps the existing accelerator instead of the
///   garbage;
/// - a registration failure (e.g. another application owns the combination)
///   returns `conflict` and leaves prefs untouched, so the persisted config
///   still describes what is actually active;
/// - a persist failure rolls the registration change back best-effort (the
///   persisted prefs are still authoritative — replay them, on this same
///   blocking worker) and returns Err.
#[tauri::command]
pub(crate) async fn set_global_shortcut_config(
    app: AppHandle,
    enabled: bool,
    accelerator: String,
) -> Result<GlobalShortcutApplyResult, String> {
    super::settings::join_blocking("set_global_shortcut_config", move || {
        let _config = config_lock();
        // Validate/parse first and store the canonical form: prefs hold the
        // normalized display form so comparisons never see alias spellings.
        // A disable tolerates unparsable input — there is no new parse to
        // feed — and then persists the EXISTING accelerator instead.
        let canonical = match crate::features::global_shortcut::normalize_accelerator(&accelerator)
        {
            Ok(canonical) => Some(canonical),
            Err(message) if enabled => return Ok(GlobalShortcutApplyResult::invalid(message)),
            Err(_) => None,
        };
        // Apply the registration change (the feature re-checks session
        // availability; a conflict result skips persistence below). The
        // unparsable-disable case hands the raw input over: apply_config
        // parses lazily and never touches it while disabling.
        let outcome = crate::features::global_shortcut::apply_config(
            &app,
            enabled,
            canonical.as_deref().unwrap_or(&accelerator),
        )?;
        if !should_persist(outcome.status) {
            return Ok(GlobalShortcutApplyResult::from_outcome(outcome));
        }
        // Persist inside the same lock+join: only when the outcome warrants
        // it, and with the canonical form (or the existing accelerator when
        // disabling with unparsable input).
        let persist = crate::platform::prefs::UserPrefs::update_transaction(|prefs| {
            prefs.global_shortcut_enabled = enabled;
            if let Some(canonical) = &canonical {
                prefs.global_shortcut_accelerator = canonical.clone();
            }
            Ok(())
        });
        match persist {
            // update_transaction yields the updated prefs; only its success
            // or failure matters here.
            Ok(_) => Ok(GlobalShortcutApplyResult::from_outcome(outcome)),
            Err(persist_error) => {
                // Roll the registration back to the still-authoritative
                // persisted config. The transaction aborted, so disk still
                // holds the old values; replaying them restores
                // "settings.json describes what is active". The replay runs
                // here on the blocking worker (never the async runtime), and
                // a rollback failure is logged only — the persist error is
                // the one worth surfacing.
                let prefs = crate::platform::prefs::UserPrefs::load();
                if let Err(rollback_error) = crate::features::global_shortcut::apply_config(
                    &app,
                    prefs.global_shortcut_enabled,
                    &prefs.global_shortcut_accelerator,
                ) {
                    // Diagnosability: when the replay itself failed, record
                    // what disk still holds so the divergence between the
                    // active registration and settings.json is visible.
                    log::warn!(
                        "global shortcut rollback after persist failure failed: {rollback_error} (disk holds enabled={} accelerator={:?})",
                        prefs.global_shortcut_enabled,
                        prefs.global_shortcut_accelerator
                    );
                }
                Err(persist_error)
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The persist-ordering contract, pinned: Invalid and Conflict changed
    /// nothing, so prefs must keep describing the still-active state.
    #[test]
    fn should_persist_persists_only_state_changing_outcomes() {
        use crate::features::global_shortcut::ApplyStatus;
        assert!(should_persist(ApplyStatus::Registered));
        assert!(should_persist(ApplyStatus::Disabled));
        assert!(!should_persist(ApplyStatus::Invalid));
        assert!(!should_persist(ApplyStatus::Conflict));
    }

    /// The wire shape is load-bearing across the language boundary: the
    /// frontend mirror (`bridge/settings.js`) reads `status` and
    /// `accelerator` by name, and a rename here would pass every frontend
    /// test while silently disabling the snapshot mirror. Pin the exact
    /// serialized keys.
    #[test]
    fn wire_result_serializes_the_pinned_shape() {
        let value = serde_json::to_value(GlobalShortcutApplyResult {
            status: "registered".to_string(),
            accelerator: Some("control+alt+KeyP".to_string()),
            message: None,
        })
        .expect("result must serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "status": "registered",
                "accelerator": "control+alt+KeyP",
                "message": null,
            }),
            "serialized field names are part of the wire contract"
        );
    }
}
