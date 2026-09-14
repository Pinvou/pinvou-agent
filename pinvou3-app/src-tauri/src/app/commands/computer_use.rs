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
use crate::features::computer_use::ComputerUseShared;

/// Empty-string defense for the grant/revoke (session_id) and confirm/deny
/// (confirm_id) identifiers (review finding): an empty string has no
/// business meaning and must not silently succeed and pollute guard state.
fn ensure_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    Ok(())
}

/// Return projection of `computer_use_get_status` (the frontend renders the
/// grant/stop state from it).
#[derive(Debug, Clone, Serialize)]
pub struct ComputerUseStatus {
    /// Settings master toggle (in-memory mirror of settings.json
    /// `computer_use.enabled`).
    pub enabled: bool,
    /// Whether this session currently holds a valid input grant (not
    /// revoked/cleared by stop; grants have no idle expiry).
    pub granted: bool,
    /// Emergency-stop flag (set by `computer_use_stop`, cleared when the
    /// master toggle is re-enabled).
    pub stopped: bool,
    /// Whether the current OS has a computer_use backend implementation.
    pub platform_supported: bool,
}

#[tauri::command]
pub fn computer_use_get_status(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> ComputerUseStatus {
    ComputerUseStatus {
        enabled: shared.is_enabled(),
        granted: shared.has_active_grant(&session_id),
        stopped: shared.is_stopped(),
        platform_supported: crate::features::computer_use::backend_supported(),
    }
}

/// Grant this session mouse/keyboard control (session grant). The grant
/// lives until explicitly revoked (revoke / stop / master toggle off) and
/// has no idle expiry — no mainstream product puts an idle clock on a
/// session-scoped grant (same semantics as Claude Code's "allow for this
/// session"). Granting is refused while the master toggle is off or the
/// emergency stop is latched: a grant issued in that state would silently
/// sleep until the toggle is re-enabled / the stop is reset, meaning one
/// click from a stale frontend would override the user's current global
/// intent (review finding; the disabled UI does not render the grant
/// button in the first place — this is the line of defense against a
/// stale/desynced frontend).
#[tauri::command]
pub fn computer_use_grant(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    if !shared.is_enabled() {
        return Err(
            "computer use is disabled; enable it in settings before granting control".into(),
        );
    }
    if shared.is_stopped() {
        return Err("computer use is stopped; resume it before granting control".into());
    }
    shared.grant_session(&session_id);
    Ok(())
}

/// Revoke this session's input grant. The in-app grant expires
/// immediately; this also triggers the backend to close the persistent
/// OS-level grant (Wayland portal session) — on a detached thread so this
/// command does not block (review finding: grants previously lived until
/// process exit, contradicting the "stoppable at any time" promise).
/// Emergency (not plain) release: a physically held left button is
/// unpressed first, and the control lane survives an in-flight action.
#[tauri::command]
pub fn computer_use_revoke(
    session_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("session_id", &session_id)?;
    shared.revoke_session(&session_id);
    shared.backends.emergency_release(&session_id);
    Ok(())
}

/// Emergency stop: latch the stop flag and revoke every session grant,
/// and trigger all backends to close persistent OS-level grants (detached
/// threads, see [`computer_use_revoke`]).
#[tauri::command]
pub fn computer_use_stop(shared: State<'_, Arc<ComputerUseShared>>) {
    shared.stop_all();
    shared.backends.emergency_release_all();
}

/// The user confirms an intercepted T3 consequential action in the
/// frontend: mint a single-use approval token. confirm_id must come from a
/// `computer_use:confirm_required` event (still pending); unknown ids are
/// rejected so the model cannot mint approvals for self-invented ids.
#[tauri::command]
pub fn computer_use_confirm(
    confirm_id: String,
    shared: State<'_, Arc<ComputerUseShared>>,
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
    // mint_confirmation only mints for an existing, unexpired pending and
    // returns true (review finding: it previously no-op'd silently and the
    // frontend showed failure as success); false surfaces an explicit
    // error. The parenthetical keeps "unknown or expired": the frontend
    // bridge matches that phrase to recognize "this pending expired" and
    // clear the confirmation dialog locally (expiry is not a user denial);
    // the wording must not break that contract.
    if shared.mint_confirmation(&confirm_id) {
        Ok(())
    } else {
        Err(format!(
            "confirmation request no longer exists (unknown or expired): {confirm_id}"
        ))
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
) -> Result<(), String> {
    ensure_non_empty("confirm_id", &confirm_id)?;
    if shared.deny_confirmation(&confirm_id) {
        Ok(())
    } else {
        Err(format!(
            "unknown or expired confirm_id (already decided?): {confirm_id}"
        ))
    }
}

/// Set the master toggle. Persist to disk first, then flip the in-memory
/// flag (same order as set_voice_shortcut_enabled): if the write fails the
/// in-memory state must not diverge from settings.json. Re-enabling clears
/// the stop flag (the guard's established semantics) but restores no
/// session grants; disabling revokes all session grants and clears pending
/// confirmations (review finding: otherwise old grants and old approval
/// tokens would survive a disable/enable cycle). Finally, hot-refresh the
/// disallowed_tools (review finding: the tool_policy closure only
/// re-evaluates when refreshed; without an explicit refresh the catalog of
/// already-running engines would lag until some unrelated policy refresh;
/// same established pattern as the marketplace/connector commands calling
/// `pool.refresh_disallowed_tools()`). The refresh makes BOTH toggle
/// directions immediate on every live engine: the tool is always
/// constructed (see the tool_factory in lib.rs), so enabling just removes
/// it from the disallow list — no engine rebuild required.
#[tauri::command]
pub async fn computer_use_set_enabled(
    enabled: bool,
    shared: State<'_, Arc<ComputerUseShared>>,
    pool: State<'_, EnginePool>,
) -> Result<(), String> {
    UserPrefs::update_transaction(|prefs| {
        prefs.computer_use.enabled = enabled;
        Ok(())
    })?;
    shared.set_enabled(enabled);
    if enabled {
        shared.reset_stop();
    } else {
        shared.revoke_all_sessions();
        // Master toggle off: terminate every backend's persistent OS-level
        // grant as well (detached threads).
        // Emergency variant: also unpress any physically held left button.
        shared.backends.emergency_release_all();
    }
    pool.refresh_disallowed_tools().await;
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
            })
        );
    }

    /// Review-fix regression: empty/whitespace-only identifiers for
    /// grant/revoke (session_id) and confirm/deny (confirm_id) must fail
    /// explicitly instead of silently succeeding.
    #[test]
    fn empty_identifiers_are_rejected() {
        assert!(ensure_non_empty("session_id", "").is_err());
        assert!(ensure_non_empty("session_id", "   ").is_err());
        assert!(ensure_non_empty("confirm_id", "").is_err());
        assert!(ensure_non_empty("confirm_id", "cu-0123456789abcdef").is_ok());
        assert!(ensure_non_empty("session_id", "s1").is_ok());
    }
}
