//! Computer Use consent-gating command surface.
//!
//! The engine currently auto-approves every tool call, so consent gating is
//! built into the tool itself (`ComputerUseShared` is the single source of
//! truth); these commands are the only entry point through which the
//! frontend injects user decisions:
//! settings toggle / session grant / revoke / emergency stop / T3
//! confirmation / platform permission onboarding.
//! Grants and confirmation tokens live only in memory; the only persisted
//! piece is the `computer_use.enabled` master toggle.

use std::sync::Arc;

use super::prelude::*;
use crate::features::computer_use::{ComputerUseShared, EVENT_STATE_CHANGED, GrantOutcome};

/// Empty-string defense for the grant/revoke (session_id) and confirm/deny
/// (confirm_id) identifiers: an empty string has no business meaning and
/// must not silently succeed and pollute guard state.
fn ensure_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

/// The `computer_use_confirm` error for a confirm_id that no longer mints.
/// The parenthetical "unknown or expired" is a frontend contract: the
/// bridge matches that phrase to recognize an expired pending and clear
/// the confirmation dialog locally (expiry is not a user denial); the
/// wording must not break that contract.
fn confirm_unknown_error(confirm_id: &str) -> String {
    format!("confirmation request no longer exists (unknown or expired): {confirm_id}")
}

/// The `computer_use_deny` error for a confirm_id that is not pending (and
/// has no unspent token to retract). Carries the same "unknown or expired"
/// frontend contract phrase as [`confirm_unknown_error`].
fn deny_unknown_error(confirm_id: &str) -> String {
    format!("unknown or expired confirm_id (already decided?): {confirm_id}")
}

/// Broadcast a consent-state transition to every window (`app.emit`):
/// a request resolved in one window collapses the phantom dialog the other
/// windows still show for the same session, and a stop/toggle elsewhere is
/// reflected without waiting for the 30s reconciler tick. Payload is a
/// minimal hint; consumers reconcile through `computer_use_get_status`.
/// The remote transport rejects this event (it is a first-party consent
/// signal, like grant_required/confirm_required).
fn notify_state_changed(app: &AppHandle, reason: &'static str, session_id: Option<&str>) {
    let _ = app.emit(
        EVENT_STATE_CHANGED,
        serde_json::json!({
            "reason": reason,
            "session_id": session_id,
        }),
    );
}

/// Return projection of `computer_use_get_status` (the frontend renders the
/// grant/stop state from it).
#[derive(Debug, Clone, Serialize)]
pub struct ComputerUseStatus {
    /// Settings master toggle (in-memory mirror of settings.json
    /// `computer_use.enabled`).
    pub enabled: bool,
    /// Whether this session currently holds a valid input grant (the grant
    /// is bound to the tool instance's lifetime: a revoke / stop /
    /// master-switch-off, or engine idle reclaim, ends it — see guard.rs).
    /// Always false when the request carries no session id.
    pub granted: bool,
    /// Emergency-stop flag (set by `computer_use_stop`, cleared when the
    /// master toggle is re-enabled).
    pub stopped: bool,
    /// Whether the current OS has a computer_use backend implementation.
    pub platform_supported: bool,
    /// Server truth for the consent UI: whether this session's grant
    /// request is still unanswered. Lets every window collapse a grant
    /// dialog resolved elsewhere and reconstruct it after a reload.
    pub pending_grant: bool,
    /// The newest unexpired `confirm_required` payload for this session,
    /// exactly as broadcast (null when nothing is pending — the key is
    /// always present so the frontend can tell "server says none" apart
    /// from an older backend that predates the field). Every window
    /// reconciles from it: a dialog decided in one window collapses in the
    /// others instead of waiting for a click that would fail with
    /// "unknown or expired".
    pub pending_confirm: Option<serde_json::Value>,
}

#[tauri::command]
pub fn computer_use_get_status(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> ComputerUseStatus {
    ComputerUseStatus {
        enabled: shared.is_enabled(),
        // An empty session id means "no active session" (e.g. the settings
        // page probing platform support before any session exists); there is
        // no grant to report in that case.
        granted: !session_id.is_empty() && shared.has_active_grant(&session_id),
        stopped: shared.is_stopped(),
        platform_supported: crate::features::computer_use::backend_supported(),
        pending_grant: !session_id.is_empty() && shared.grant_request_pending(&session_id),
        pending_confirm: if session_id.is_empty() {
            None
        } else {
            shared.pending_payload_for_session(&session_id)
        },
    }
}

/// Grant this session mouse/keyboard control (session grant). The grant is
/// bound to the tool instance's lifetime: it ends on revoke / stop /
/// master-switch off, or when the engine reclaims the tool (its `Drop`
/// revokes) — including the engine pool's idle reaper silently reaping an
/// idle engine, in which case the grant lapses and the user must grant
/// again. Granting is refused while the master toggle is off or the
/// emergency stop is latched: a grant issued in that state would silently
/// sleep until the toggle is re-enabled / the stop is reset, meaning one
/// click from a stale frontend would override the user's current global
/// intent (the disabled UI does not render the grant button in the first
/// place — this is the line of defense against a stale/desynced frontend).
#[tauri::command]
pub fn computer_use_grant(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
    app: AppHandle,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    // The authoritative switch/stop check lives inside grant_session, under
    // the sessions lock in the same critical section as the insert (a grant
    // cannot slip in after a revoke sweep and survive the off period); the
    // match below only maps the refusal to a specific user-facing message.
    match shared.grant_session(&session_id) {
        GrantOutcome::Granted => {
            notify_state_changed(&app, "granted", Some(&session_id));
            Ok(())
        }
        GrantOutcome::Disabled => {
            Err("computer use is disabled; enable it in settings before granting control".into())
        }
        GrantOutcome::Stopped => {
            Err("computer use is stopped; resume it before granting control".into())
        }
    }
}

/// Revoke this session's input grant. The in-app grant expires
/// immediately; this also triggers the backend to close the persistent
/// OS-level grant (Wayland portal session) — on a detached thread so this
/// command does not block. Emergency (not plain) release: a physically held
/// left button is unpressed first, and the control lane survives an
/// in-flight action.
#[tauri::command]
pub fn computer_use_revoke(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
    app: AppHandle,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    shared.revoke_session(&session_id);
    shared.backends.emergency_release(&session_id);
    notify_state_changed(&app, "revoked", Some(&session_id));
    Ok(())
}

/// Emergency stop: latch the stop flag and revoke every session grant,
/// and trigger all backends to close persistent OS-level grants (detached
/// threads, see [`computer_use_revoke`]).
#[tauri::command]
pub fn computer_use_stop(shared: State<'_, Arc<ComputerUseShared>>, app: AppHandle) {
    shared.stop_all();
    shared.backends.emergency_release_all();
    notify_state_changed(&app, "stopped", None);
}

/// The user confirms an intercepted T3 consequential action in the
/// frontend: mint a single-use approval token. confirm_id must come from a
/// `computer_use:confirm_required` event (still pending); unknown ids are
/// rejected so the model cannot mint approvals for self-invented ids.
#[tauri::command]
pub fn computer_use_confirm(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
    app: AppHandle,
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
    // mint_confirmation only mints for an existing, unexpired pending while
    // the switch is on and no stop is latched (both re-checked under the
    // consent lock), returning true; false surfaces an explicit error — a
    // silent no-op would let the frontend show failure as success.
    if shared.mint_confirmation(&confirm_id) {
        notify_state_changed(&app, "confirmed", None);
        Ok(())
    } else {
        Err(confirm_unknown_error(&confirm_id))
    }
}

/// The user explicitly "denies" an intercepted T3 action in the frontend:
/// drop the pending. A denial records no server-side state — a same-action
/// retry by the model re-runs screening and mints a **new** pending and
/// re-emits the confirmation event (mainstream-model stance: a denial is
/// only model-visible context, not a stored penalty state).
#[tauri::command]
pub fn computer_use_deny(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
    app: AppHandle,
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
    if shared.deny_confirmation(&confirm_id) {
        notify_state_changed(&app, "denied", None);
        Ok(())
    } else {
        Err(deny_unknown_error(&confirm_id))
    }
}

/// Set the master toggle. Persist to disk first, then flip the in-memory
/// flag (same order as set_voice_shortcut_enabled): if the write fails the
/// in-memory state must not diverge from settings.json. The consent sweeps
/// ride inside the guard's `set_enabled` (re-enable resets the stop flag
/// and sweeps consent created in the stopped window; disable revokes all
/// session grants and clears pending confirmations and minted tokens —
/// otherwise old grants and old approval tokens would survive a
/// disable/enable cycle), so this command only adds what the guard does
/// not own: the backend OS-grant termination on disable. Finally,
/// hot-refresh the
/// disallowed_tools: the tool_policy closure only re-evaluates when
/// refreshed; without an explicit refresh the catalog of already-running
/// engines would lag until some unrelated policy refresh (same established
/// pattern as the marketplace/connector commands calling
/// `pool.refresh_disallowed_tools()`). The refresh makes BOTH toggle
/// directions immediate on every live engine: the tool is always
/// constructed (see the tool_factory in lib.rs), so enabling just removes
/// it from the disallow list — no engine rebuild required.
#[tauri::command]
pub async fn computer_use_set_enabled(
    enabled: bool,
    shared: State<'_, Arc<ComputerUseShared>>,
    pool: State<'_, EnginePool>,
    app: AppHandle,
) -> Result<(), String> {
    // Reject on platforms without a backend: the toggle used to persist
    // happily where computer use can never work, leaving the UI to discover
    // it via a status round-trip. The frontend already rolls the optimistic
    // flip back and surfaces the error.
    if enabled && !crate::features::computer_use::backend_supported() {
        return Err("computer use has no backend on this operating system".to_string());
    }
    UserPrefs::update_transaction(|prefs| {
        prefs.computer_use.enabled = enabled;
        Ok(())
    })?;
    // set_enabled itself performs the consent sweeps (re-enable: reset_stop
    // semantics; disable: revoke_all_sessions semantics — see guard.rs), so
    // the explicit reset_stop/revoke_all_sessions calls the command used to
    // pair with the flip are gone: no caller can flip the flag without the
    // sweep anymore.
    shared.set_enabled(enabled);
    if !enabled {
        // Master toggle off: terminate every backend's persistent OS-level
        // grant as well (detached threads). Stays in the command: the guard
        // does not own the backend registry.
        // Emergency variant: also unpress any physically held left button.
        shared.backends.emergency_release_all();
    }
    pool.refresh_disallowed_tools().await;
    // Windows that keep their own slice (detached shells) re-read immediately
    // instead of discovering a toggle made elsewhere at the next reconciler
    // tick — this is the other-window-enable gap the inert-branch re-read
    // already bridges, now pushed proactively.
    notify_state_changed(&app, "enabled-changed", None);
    Ok(())
}

/// Trigger platform permission onboarding: macOS raises the Screen
/// Recording + Accessibility system dialogs; Windows/Linux have no system
/// permission flow and get an explicit unsupported error (never a silent
/// no-op).
#[tauri::command]
pub fn computer_use_request_permissions() -> Result<(), String> {
    crate::features::computer_use::request_permissions().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The status projection's JSON keys are a frontend contract (the
    /// computerUse feature consumes them directly).
    #[test]
    fn status_serializes_contract_keys() {
        let status = ComputerUseStatus {
            enabled: true,
            granted: false,
            stopped: false,
            platform_supported: true,
            pending_grant: true,
            pending_confirm: Some(serde_json::json!({
                "session_id": "s1",
                "action": "left_click",
                "confirm_id": "cu-abc"
            })),
        };
        let Ok(value) = serde_json::to_value(&status) else {
            panic!("ComputerUseStatus must serialize");
        };
        assert_eq!(
            value,
            serde_json::json!({
                "enabled": true,
                "granted": false,
                "stopped": false,
                "platform_supported": true,
                "pending_grant": true,
                "pending_confirm": {
                    "session_id": "s1",
                    "action": "left_click",
                    "confirm_id": "cu-abc"
                },
            })
        );
        // A served pending_confirm is exactly the confirm_required event
        // payload; None still serializes as a PRESENT null key (asserted
        // below), so consumers can tell "server says none" apart from an
        // older backend that predates the field.
        let no_confirm = ComputerUseStatus {
            pending_grant: false,
            pending_confirm: None,
            ..status
        };
        let Ok(value) = serde_json::to_value(&no_confirm) else {
            panic!("ComputerUseStatus must serialize");
        };
        // The key must stay PRESENT as null: the frontend distinguishes
        // "server says none" (null) from an older backend (key absent) and
        // only reconciles against the former.
        assert_eq!(
            value.get("pending_confirm"),
            Some(&serde_json::Value::Null),
            "{value}"
        );
    }

    /// The frontend bridge matches the exact phrase "unknown or expired" in
    /// the confirm/deny error text to recognize an expired pending and clear
    /// the stale dialog locally (the 5-minute TTL would otherwise dead-lock
    /// the modal). The commands build those errors through
    /// `confirm_unknown_error` / `deny_unknown_error`; assert on the
    /// constructed strings so a rewording of the helper breaks the test
    /// (a raw source-text count could be satisfied by a comment mentioning
    /// the phrase).
    #[test]
    fn confirm_error_phrases_keep_the_frontend_contract() {
        let confirm_id = "cu-0123456789abcdef";
        assert!(
            confirm_unknown_error(confirm_id).contains("unknown or expired"),
            "the confirm error must carry the frontend contract phrase"
        );
        assert!(
            deny_unknown_error(confirm_id).contains("unknown or expired"),
            "the deny error must carry the frontend contract phrase"
        );
    }

    /// Empty/whitespace-only identifiers for grant/revoke (session_id) and
    /// confirm/deny (confirm_id) must fail explicitly instead of silently
    /// succeeding.
    #[test]
    fn empty_identifiers_are_rejected() {
        assert!(ensure_non_empty("session_id", "").is_err());
        assert!(ensure_non_empty("session_id", "   ").is_err());
        assert!(ensure_non_empty("confirm_id", "").is_err());
        assert!(ensure_non_empty("confirm_id", "cu-0123456789abcdef").is_ok());
        assert!(ensure_non_empty("session_id", "s1").is_ok());
    }
}
