//! System-wide bring-to-front shortcut (`tauri-plugin-global-shortcut`):
//! while enabled (opt-in, OFF by default), pressing the accelerator
//! (default `CmdOrCtrl+Alt+P`) presents the main window while Pinvou runs
//! in the background.
//!
//! Split along the crate's platform rules: this file holds the pure,
//! OS-independent core (accelerator parsing/normalization, the apply-decision
//! state machine, and the register/unregister bookkeeping — the plugin itself
//! is cross-platform); `platform/` holds the bring-to-front composition,
//! whose Linux arm carries the Mutter/X11 raise workaround.
//!
//! Registration is driven from Rust only: the plugin is initialized once in
//! `lib.rs` with a plugin-level handler (which survives register/unregister
//! cycles), and this module owns exactly one shortcut at a time — unregister
//! never goes through `unregister_all`, so shortcuts other features might
//! register on the same plugin stay untouched. The authoritative persistence
//! is `settings.json` (`global_shortcut_enabled` / `global_shortcut_accelerator`,
//! written by the `set_global_shortcut_config` command); lib.rs replays them
//! at startup — a parse failure or register conflict logs and continues, it
//! never fails startup.
//!
//! The trigger path dispatches to the main thread (`handle_triggered` →
//! `run_on_main_thread`): the plugin handler fires on the global-hotkey X11
//! event thread, and the present composition crosses blocking getters that
//! wait on the main thread, so running it on the hotkey thread would risk a
//! circular wait against a concurrent register/unregister.
//!
//! Availability is gated by `platform::capabilities::global_shortcuts_available`
//! (Windows/macOS and X11 on Linux): on Wayland sessions the plugin's X11
//! backend would silently no-op, so `apply_config` refuses to touch it.

use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

// The default accelerator string is single-sourced as
// `crate::platform::prefs::DEFAULT_GLOBAL_SHORTCUT_ACCELERATOR` (the layer
// its serde default fn lives in — platform must not depend on features);
// tests below pin that this module and the persisted default agree on it.

mod platform;

/// The shortcut this module currently has registered, in canonical display
/// form. Unregistering reparses this string — safe because the canonical
/// form is round-trip-proven (the normalization tests pin that a canonical
/// display string parses back to the same shortcut). Written only under
/// [`apply_config`]'s critical section.
static REGISTERED: Mutex<Option<RegisteredShortcut>> = Mutex::new(None);

/// App handle captured by [`install`], used by the startup replay.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredShortcut {
    /// Canonical display form (the comparison key against prefs).
    accelerator: String,
}

/// Parse an accelerator with the plugin's `Shortcut` parser. Shared by
/// normalization and registration so both reject the same inputs.
fn parse_accelerator(raw: &str) -> Result<Shortcut, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("accelerator is empty".to_string());
    }
    Shortcut::from_str(trimmed).map_err(|error| format!("invalid accelerator {raw:?}: {error}"))
}

/// Normalize an accelerator into the plugin's canonical display form (e.g.
/// `"Ctrl+Alt+P"` → `"control+alt+KeyP"`; the `CmdOrCtrl` alias resolves to
/// control on Windows/Linux and cmd on macOS, so the canonical form is
/// OS-dependent). The canonical form is what gets stored in prefs and
/// compared, so alias/casing/spacing differences never accumulate.
pub(crate) fn normalize_accelerator(raw: &str) -> Result<String, String> {
    Ok(parse_accelerator(raw)?.to_string())
}

/// What [`apply_config`] must change given the requested config and the
/// currently registered accelerator (`None` = nothing registered). Pure so
/// the state transitions stay unit-tested without a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApplyPlan {
    /// Already in the requested state; touch nothing.
    Noop,
    /// Drop the current registration (requested disabled while registered).
    Unregister { previous: String },
    /// Drop `previous` (if any) and register the new accelerator.
    Replace {
        previous: Option<String>,
        accelerator: String,
    },
}

fn plan_apply(enabled: bool, accelerator: &str, current: Option<&str>) -> ApplyPlan {
    match (enabled, current) {
        (true, Some(previous)) if previous == accelerator => ApplyPlan::Noop,
        (true, previous) => ApplyPlan::Replace {
            previous: previous.map(str::to_string),
            accelerator: accelerator.to_string(),
        },
        (false, None) => ApplyPlan::Noop,
        (false, Some(previous)) => ApplyPlan::Unregister {
            previous: previous.to_string(),
        },
    }
}

/// Outcome status of an apply, mirrored 1:1 into the
/// `set_global_shortcut_config` wire result (`status` strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyStatus {
    /// The shortcut is registered (or already was).
    Registered,
    /// No shortcut is active (user disabled it, or the session cannot have
    /// one — see the availability gate).
    Disabled,
    /// The accelerator failed to parse; nothing was changed.
    Invalid,
    /// Registration failed (e.g. another application owns the combination);
    /// prefs must not be persisted in this case.
    Conflict,
}

impl ApplyStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ApplyStatus::Registered => "registered",
            ApplyStatus::Disabled => "disabled",
            ApplyStatus::Invalid => "invalid",
            ApplyStatus::Conflict => "conflict",
        }
    }
}

/// Result of an apply, carried to the command layer verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApplyOutcome {
    pub(crate) status: ApplyStatus,
    /// Canonical accelerator on success (`Registered`); `None` otherwise.
    pub(crate) accelerator: Option<String>,
    pub(crate) message: Option<String>,
}

impl ApplyOutcome {
    fn registered(accelerator: String) -> Self {
        Self {
            status: ApplyStatus::Registered,
            accelerator: Some(accelerator),
            message: None,
        }
    }

    fn disabled() -> Self {
        Self {
            status: ApplyStatus::Disabled,
            accelerator: None,
            message: None,
        }
    }

    fn unavailable_on_session() -> Self {
        Self {
            status: ApplyStatus::Disabled,
            accelerator: None,
            message: Some("global shortcuts are unavailable on this session".to_string()),
        }
    }

    fn conflict(message: String) -> Self {
        Self {
            status: ApplyStatus::Conflict,
            accelerator: None,
            message: Some(message),
        }
    }
}

fn registered_state() -> std::sync::MutexGuard<'static, Option<RegisteredShortcut>> {
    // A panic while holding the lock (there are none on these paths) must not
    // brick the feature for the rest of the process: recover the guard.
    REGISTERED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn register_shortcut(app: &AppHandle, shortcut: Shortcut) -> Result<(), String> {
    app.global_shortcut()
        .register(shortcut)
        .map_err(|error| format!("register failed: {error}"))
}

/// Unregister exactly this shortcut — never `unregister_all`, which would
/// tear down shortcuts other features registered on the same plugin.
fn unregister_shortcut_display(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let shortcut = parse_accelerator(accelerator)?;
    app.global_shortcut()
        .unregister(shortcut)
        .map_err(|error| format!("unregister failed: {error}"))
}

/// Apply the requested configuration against the current registration.
///
/// Idempotent and safe to call concurrently (startup replay and the settings
/// command serialize on the module state): consecutive calls unregister the
/// old shortcut before registering the new one. An `Invalid` parse or a
/// `Conflict` leaves the registration and prefs untouched.
pub(crate) fn apply_config(
    app: &AppHandle,
    enabled: bool,
    accelerator: &str,
) -> Result<ApplyOutcome, String> {
    // Parse lazily: the accelerator input is irrelevant when disabling (the
    // Noop and Unregister{previous} plans both need no new parse), so a
    // disable must not depend on the caller handing over a parsable string —
    // only enabling rejects unparsable input with `Invalid`.
    let parsed: Option<(Shortcut, String)> = if enabled {
        match parse_accelerator(accelerator) {
            Ok(shortcut) => {
                let canonical = shortcut.to_string();
                Some((shortcut, canonical))
            }
            Err(message) => {
                return Ok(ApplyOutcome {
                    status: ApplyStatus::Invalid,
                    accelerator: None,
                    message: Some(message),
                });
            }
        }
    } else {
        None
    };
    // Availability gate BEFORE touching the plugin: on unsupported sessions
    // (Linux Wayland) the X11 backend silently accepts registrations that do
    // nothing, so a fake success must be avoided up front. Disabling needs no
    // platform support (there is nothing registered there anyway).
    if enabled && !crate::platform::capabilities::current().global_shortcuts_available {
        return Ok(ApplyOutcome::unavailable_on_session());
    }

    let mut registered = registered_state();
    let plan = plan_apply(
        enabled,
        parsed
            .as_ref()
            .map(|(_, canonical)| canonical.as_str())
            .unwrap_or(accelerator),
        registered.as_ref().map(|r| r.accelerator.as_str()),
    );
    match plan {
        ApplyPlan::Noop => Ok(match parsed {
            // Enabled no-op: the exact accelerator is already registered.
            Some((_, canonical)) => ApplyOutcome::registered(canonical),
            // Disabled no-op: nothing registered, nothing to change.
            None => ApplyOutcome::disabled(),
        }),
        ApplyPlan::Unregister { previous } => {
            let outcome = unregister_shortcut_display(app, &previous);
            match outcome {
                Ok(()) => {
                    *registered = None;
                    Ok(ApplyOutcome::disabled())
                }
                Err(error) => {
                    // Residual risk (asymmetric with the Replace arm's zombie
                    // below): the plugin may still hold and fire the old
                    // combo while our bookkeeping says None, so a subsequent
                    // disable is a plan-level Noop that reports `disabled`
                    // anyway. Keep our state consistent with our own
                    // bookkeeping being uncertain.
                    *registered = None;
                    log::warn!("global shortcut unregister of {previous} failed: {error}");
                    Ok(ApplyOutcome::conflict(error))
                }
            }
        }
        ApplyPlan::Replace {
            previous,
            accelerator,
        } => {
            let Some((shortcut, _)) = parsed else {
                // Unreachable by construction: Replace only arises while
                // enabling, which parses above. A defensive Conflict keeps
                // the "never persist without a real apply" invariant (the
                // command skips persistence on Conflict) without a panic in
                // the settings path should that invariant ever break.
                return Ok(ApplyOutcome::conflict(
                    "internal: enable plan without a parsed accelerator".to_string(),
                ));
            };
            if let Some(previous) = &previous {
                if let Err(error) = unregister_shortcut_display(app, previous) {
                    // Best effort: the register attempt below decides the
                    // outcome (on Windows re-registering an OS-held combo
                    // fails and surfaces as a conflict).
                    log::warn!("global shortcut unregister of {previous} failed: {error}");
                }
                *registered = None;
            }
            match register_shortcut(app, shortcut) {
                Ok(()) => {
                    *registered = Some(RegisteredShortcut {
                        accelerator: accelerator.clone(),
                    });
                    Ok(ApplyOutcome::registered(accelerator))
                }
                Err(error) => {
                    log::warn!("global shortcut register of {accelerator} failed: {error}");
                    // Residual risk (the Replace arm's zombie, mirror of the
                    // Unregister arm's asymmetry above): when the unregister
                    // above ALSO failed, the old combo can still be live at
                    // the plugin level while our bookkeeping says None — the
                    // stale shortcut keeps firing until the next startup
                    // replay re-applies the persisted prefs.
                    Ok(ApplyOutcome::conflict(error))
                }
            }
        }
    }
}

/// Capture the app handle for the startup replay. Called once from `lib.rs`
/// after the plugin is initialized.
pub(crate) fn install(app: AppHandle) {
    let _ = APP_HANDLE.set(app);
}

/// Startup replay of the persisted prefs (`lib.rs` setup). Failures — parse
/// errors, register conflicts, a missing app handle — log and continue;
/// startup must never fail or panic because of the shortcut.
pub(crate) fn apply_saved_settings(enabled: bool, accelerator: &str) {
    let Some(app) = APP_HANDLE.get() else {
        log::warn!("global shortcut startup replay skipped: app handle not installed");
        return;
    };
    match apply_config(app, enabled, accelerator) {
        Ok(outcome) => log::info!(
            "global shortcut startup replay: status={} accelerator={:?} message={:?}",
            outcome.status.as_str(),
            outcome.accelerator,
            outcome.message
        ),
        Err(error) => log::warn!("global shortcut startup replay failed: {error}"),
    }
}

/// Plugin-level handler entry (`lib.rs`): the accelerator was pressed.
/// Only ever fires while our shortcut is registered; a missing main window
/// (mid-teardown) is logged and ignored, never a panic.
pub(crate) fn handle_triggered(app: &AppHandle) {
    // The plugin handler runs on the global-hotkey X11 event thread (Linux).
    // The bring-to-front composition crosses blocking getters that wait on
    // the main thread (the cold-start gate's `is_visible`), while a
    // concurrent register/unregister blocks the main thread on that same
    // hotkey thread — calling it from here would be a circular wait. The
    // trigger therefore dispatches the whole composition to the main thread
    // (tauri executes `run_on_main_thread` synchronously when already on it,
    // so the macOS/Windows path is unchanged).
    let app = app.clone();
    let dispatch = app.run_on_main_thread({
        let app = app.clone();
        move || {
            platform::bring_main_window_to_front(&app);
        }
    });
    if let Err(error) = dispatch {
        log::warn!("global shortcut trigger dispatch failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_accelerator_parses_and_round_trips() {
        // CmdOrCtrl is accepted by the plugin's parser (uppercased to the
        // CMDORCTRL alias): the default must always be registrable.
        let shortcut =
            parse_accelerator(crate::platform::prefs::DEFAULT_GLOBAL_SHORTCUT_ACCELERATOR)
                .expect("default accelerator must parse");
        // Canonical form is OS-dependent (control vs cmd); pin it via a
        // round-trip instead of a literal.
        let canonical = shortcut.to_string();
        assert_eq!(
            parse_accelerator(&canonical).expect("canonical form must reparse"),
            shortcut,
            "canonical display form must parse back to the same shortcut"
        );
        assert!(
            canonical.contains("alt"),
            "canonical form keeps Alt: {canonical}"
        );
        assert!(
            canonical.ends_with("KeyP"),
            "canonical form ends with the physical key: {canonical}"
        );
    }

    #[test]
    fn ctrl_alt_p_normalizes_to_the_canonical_display_form() {
        // Ctrl maps to CONTROL on every platform, so this expectation is
        // OS-independent (unlike CmdOrCtrl).
        assert_eq!(
            normalize_accelerator("Ctrl+Alt+P").as_deref(),
            Ok("control+alt+KeyP")
        );
    }

    #[test]
    fn normalization_folds_alias_casing_and_spacing() {
        let first = normalize_accelerator("  cmdorctrl + alt + p  ").expect("alias parses");
        let second =
            normalize_accelerator(crate::platform::prefs::DEFAULT_GLOBAL_SHORTCUT_ACCELERATOR)
                .expect("default parses");
        assert_eq!(
            first, second,
            "alias spellings fold onto one canonical form"
        );
    }

    #[test]
    fn empty_and_unparsable_accelerators_are_rejected() {
        assert!(normalize_accelerator("").is_err());
        assert!(normalize_accelerator("   ").is_err());
        assert!(
            normalize_accelerator("CmdOrCtrl+Alt").is_err(),
            "no main key"
        );
        assert!(normalize_accelerator("Ctrl+Alt+NotAKey").is_err());
        assert!(
            normalize_accelerator("Ctrl+P+Alt").is_err(),
            "key before modifier"
        );
    }

    #[test]
    fn plan_apply_covers_all_state_transitions() {
        // Enable from scratch.
        assert_eq!(
            plan_apply(true, "control+alt+KeyP", None),
            ApplyPlan::Replace {
                previous: None,
                accelerator: "control+alt+KeyP".to_string()
            }
        );
        // Enable with the same accelerator already registered: no-op.
        assert_eq!(
            plan_apply(true, "control+alt+KeyP", Some("control+alt+KeyP")),
            ApplyPlan::Noop
        );
        // Switch accelerators: old goes first.
        assert_eq!(
            plan_apply(true, "control+alt+KeyF9", Some("control+alt+KeyP")),
            ApplyPlan::Replace {
                previous: Some("control+alt+KeyP".to_string()),
                accelerator: "control+alt+KeyF9".to_string()
            }
        );
        // Disable while registered: unregister exactly the old one.
        assert_eq!(
            plan_apply(false, "control+alt+KeyP", Some("control+alt+KeyP")),
            ApplyPlan::Unregister {
                previous: "control+alt+KeyP".to_string()
            }
        );
        // Disable while nothing is registered: no-op.
        assert_eq!(plan_apply(false, "control+alt+KeyP", None), ApplyPlan::Noop);
    }

    #[test]
    fn status_strings_match_the_wire_contract() {
        assert_eq!(ApplyStatus::Registered.as_str(), "registered");
        assert_eq!(ApplyStatus::Disabled.as_str(), "disabled");
        assert_eq!(ApplyStatus::Invalid.as_str(), "invalid");
        assert_eq!(ApplyStatus::Conflict.as_str(), "conflict");
    }
}
