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
//! Registration is driven from Rust only: the plugin is initialized in
//! `lib.rs` on sessions where it can work (the same availability flag gates
//! every apply below) with no plugin-level handler, and each registration
//! binds a per-shortcut handler via `on_shortcut` — so only a combo this
//! module registered can ever trigger the bring-to-front (a plugin-level
//! handler would fire for ANY shortcut registered on the shared plugin). This
//! module owns exactly one shortcut at a time; unregister never goes through
//! `unregister_all`, so shortcuts other features might register on the same
//! plugin stay untouched. The authoritative persistence is `settings.json`
//! (`global_shortcut_enabled` / `global_shortcut_accelerator`, written by the
//! `set_global_shortcut_config` command); lib.rs replays them at startup — a
//! parse failure or register conflict logs and continues, it never fails
//! startup.
//!
//! The trigger path dispatches to the main thread (`handle_triggered` →
//! `run_on_main_thread`): on Linux the plugin dispatches shortcut events on
//! the global-hotkey X11 event thread, and the present composition crosses
//! blocking getters that wait on the main thread, so running it on the hotkey
//! thread would risk a circular wait against a concurrent register/unregister
//! (on macOS/Windows events already arrive on the main thread, where the
//! dispatch is a synchronous same-thread call).
//!
//! Availability is gated by `platform::capabilities::global_shortcuts_available`
//! (Windows/macOS and X11 on Linux): on Wayland sessions the plugin's X11
//! backend has no working attach target — XWayland grabs would silently do
//! nothing, and on a pure Wayland session the backend cannot connect at all —
//! so `apply_config` refuses before touching the plugin (and lib.rs skips the
//! plugin registration entirely there).

use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

// The default accelerator string is single-sourced as
// `crate::platform::prefs::DEFAULT_GLOBAL_SHORTCUT_ACCELERATOR` (the layer
// its serde default fn lives in — platform must not depend on features);
// tests below pin that this module and the persisted default agree on it.

mod platform;

/// The shortcut this module currently has registered, in canonical display
/// form. Unregistering reparses this string — safe because the canonical
/// form round-trips (the `canonical_display_form_round_trips` test pins the
/// property over non-default keys, and the default-accelerator test pins the
/// specific value). Written only under [`apply_config`]'s critical section.
static REGISTERED: Mutex<Option<RegisteredShortcut>> = Mutex::new(None);

/// App handle captured by [`install`], used by the startup replay.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegisteredShortcut {
    /// Canonical display form (the comparison key against prefs).
    accelerator: String,
}

/// Parse an accelerator with the plugin's `Shortcut` parser, plus this
/// feature's own product policy: at least one modifier. The plugin parser
/// also accepts bare keys (`KeyP`, `F9`), but a system-wide grab of a lone
/// letter key would swallow that keystroke in every application — too
/// aggressive for a summon shortcut. Shared by normalization and
/// registration so both reject the same inputs (the frontend pre-validation
/// mirrors the policy, but the Rust layer does not trust it).
fn parse_accelerator(raw: &str) -> Result<Shortcut, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("accelerator is empty".to_string());
    }
    let shortcut = Shortcut::from_str(trimmed)
        .map_err(|error| format!("invalid accelerator {raw:?}: {error}"))?;
    if shortcut.mods.is_empty() {
        return Err(format!(
            "invalid accelerator {raw:?}: at least one modifier is required"
        ));
    }
    Ok(shortcut)
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
    // Per-shortcut handler (`on_shortcut`, not `register`): the handler is
    // bound at registration time, so only THIS combo can ever trigger the
    // bring-to-front — a plugin-level handler would fire for any shortcut
    // registered on the shared plugin, including ones other features might
    // add later. Triggering strictly on Pressed keeps Released a no-op (the
    // Windows backend emits it from a poll thread).
    //
    // Known plugin-level residual (global-hotkey 0.8): on X11 the OS register
    // round-trips through the manager's event thread, and if that thread died
    // at creation (no reachable X server despite the session gate) the reply
    // channel disconnects and the plugin reports Ok anyway — a "registered"
    // outcome is only as good as the X11 connection. The availability gate
    // (session type AND a DISPLAY socket) keeps this to narrow stale-env
    // windows; the combo then simply never fires until the next start.
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                handle_triggered(app);
            }
        })
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
/// command serialize on the module state): a Replace registers the new
/// shortcut before dropping the old one, so a failed register leaves the
/// previous combo working. An `Invalid` parse or a `Conflict` leaves the
/// registration and prefs untouched.
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
    // (Linux Wayland) the X11 backend cannot serve the feature — XWayland
    // grabs would silently do nothing, and without any X server the backend
    // has nothing to attach to — so a fake success must be avoided up front.
    // This is the same flag lib.rs gates the plugin registration on, so a
    // `true` here guarantees the plugin state exists. Disabling needs no
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
                    // The plugin may still hold and fire the combo, so the
                    // bookkeeping must keep pointing at it: a retry-disable
                    // then genuinely retries instead of planning a Noop that
                    // would report — and persist — `disabled` while the
                    // shortcut keeps firing. `Conflict` skips persistence, so
                    // prefs keep saying enabled=true, which matches the
                    // still-registered plugin state. (Near-unreachable:
                    // macOS unregister cannot fail; X11 can fail after a
                    // keyboard-mapping change; Windows `UnregisterHotKey`
                    // can also error.)
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
            // Register the NEW combo BEFORE dropping the old one (the plan
            // guarantees new ≠ old, or this would be a Noop): a failed new
            // register — the common conflict, another app owning the combo —
            // then changes nothing, the previous shortcut keeps working, and
            // the `Conflict` outcome plus untouched prefs stay truthful.
            match register_shortcut(app, shortcut) {
                Ok(()) => {
                    if let Some(previous) = &previous {
                        if let Err(error) = unregister_shortcut_display(app, previous) {
                            // Zombie residual: the old combo stays live at the
                            // plugin level and fires ALONGSIDE the new one
                            // while bookkeeping and prefs record only the new
                            // one — self-heals at the next startup replay.
                            // Near-unreachable: unregistering a registered
                            // combo cannot fail on macOS; on X11 it can fail
                            // after a keyboard-mapping change (the keycode is
                            // re-derived from a fresh `get_keyboard_mapping`
                            // round trip), and Windows `UnregisterHotKey` can
                            // also error.
                            log::warn!("global shortcut unregister of {previous} failed: {error}");
                        }
                    }
                    *registered = Some(RegisteredShortcut {
                        accelerator: accelerator.clone(),
                    });
                    Ok(ApplyOutcome::registered(accelerator))
                }
                Err(error) => {
                    log::warn!("global shortcut register of {accelerator} failed: {error}");
                    // Nothing changed: any previous combo is still registered
                    // and still working, bookkeeping untouched — the skipped
                    // persistence keeps describing the actually-active state.
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

/// Per-shortcut handler entry (bound by [`register_shortcut`]): the
/// registered accelerator was pressed. Only fires while our shortcut is
/// registered; a missing main window (mid-teardown) is logged and ignored,
/// never a panic.
pub(crate) fn handle_triggered(app: &AppHandle) {
    // The plugin handler runs on the global-hotkey X11 event thread (Linux).
    // The bring-to-front composition crosses blocking getters that wait on
    // the main thread (the cold-start gate's `is_visible`), while a
    // concurrent register/unregister blocks the main thread on that same
    // hotkey thread — calling it from here would be a circular wait. The
    // trigger therefore dispatches the whole composition to the main thread
    // (tauri executes `run_on_main_thread` synchronously when already on it,
    // so the macOS/Windows path is unchanged).
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
    fn single_key_without_modifier_is_rejected() {
        // Product policy, enforced at the authoritative layer (not just the
        // frontend pre-validation): the plugin parser accepts bare keys, but
        // a system-wide grab of a lone letter/function key would swallow
        // that keystroke in every application. A direct IPC caller must not
        // be able to opt into that.
        for raw in ["KeyP", "F9", "p", "MediaPlayPause", "0"] {
            assert!(
                normalize_accelerator(raw).is_err(),
                "bare key {raw:?} must be rejected: at least one modifier is required"
            );
        }
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
    fn disable_plan_ignores_the_input_accelerator() {
        // The lazy-parse contract both `apply_config` and the command rely
        // on: disabling tolerates unparsable input because the Unregister
        // arm binds only `previous` — the input never reaches a parse.
        assert_eq!(
            plan_apply(false, "not even parseable!!!", Some("control+alt+KeyP")),
            ApplyPlan::Unregister {
                previous: "control+alt+KeyP".to_string()
            }
        );
    }

    #[test]
    fn canonical_display_form_round_trips() {
        // Round-trip property the REGISTERED bookkeeping relies on (it
        // reparses the stored canonical string to unregister): Display output
        // must parse back to the very same shortcut, across modifier combos,
        // function keys, digit keys and numpad aliases.
        let raw_inputs = [
            "Ctrl+Alt+F9",
            "super+Shift+Digit0",
            "cmd+NumpadAdd",
            "control+alt+Numpad5",
            "shift+MediaPlayPause",
        ];
        for raw in raw_inputs {
            let shortcut = parse_accelerator(raw)
                .unwrap_or_else(|error| panic!("fixture {raw:?} must parse: {error}"));
            let canonical = shortcut.to_string();
            assert_eq!(
                parse_accelerator(&canonical).unwrap_or_else(|error| panic!(
                    "canonical {canonical:?} must reparse: {error}"
                )),
                shortcut,
                "canonical display form of {raw:?} must round-trip"
            );
        }
    }

    #[test]
    fn lib_rs_registers_the_plugin_conditionally_and_handler_less() {
        // Source pin on lib.rs (would have caught the review-fix commit that
        // deleted the registration entirely, leaving `app.global_shortcut()`
        // panicking on missing plugin state): the plugin MUST be registered,
        // handler-less (per-shortcut handlers bind via `on_shortcut`; a
        // plugin-level `with_handler` would fire for ANY combo), and gated on
        // the same availability flag the apply path checks.
        let lib = include_str!("../lib.rs");
        assert!(
            lib.contains("tauri_plugin_global_shortcut::Builder::new()"),
            "lib.rs must register the global-shortcut plugin (state() panics otherwise)"
        );
        assert!(
            !lib.contains("with_handler"),
            "the plugin must be registered WITHOUT a plugin-level handler"
        );
        assert!(
            lib.contains("global_shortcuts_available"),
            "plugin registration must be gated on session availability"
        );
    }

    #[test]
    fn status_strings_match_the_wire_contract() {
        assert_eq!(ApplyStatus::Registered.as_str(), "registered");
        assert_eq!(ApplyStatus::Disabled.as_str(), "disabled");
        assert_eq!(ApplyStatus::Invalid.as_str(), "invalid");
        assert_eq!(ApplyStatus::Conflict.as_str(), "conflict");
    }
}
