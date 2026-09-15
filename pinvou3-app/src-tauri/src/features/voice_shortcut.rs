use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

/// Which physical Alt key started the current gesture. The state machine
/// treats both sides identically; the platform layer only reads this when
/// replaying a swallowed combo, so the injected modifier keeps the identity of
/// the key the user actually held (right-Alt combos surface as AltGr rather
/// than being rewritten to left Alt — see `alt_side_vk`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum AltSide {
    #[default]
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutKey {
    Alt(AltSide),
    Space,
    Escape,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutEvent {
    TriggerDictation,
}

/// Gesture state for the global Alt voice shortcut (written only by the
/// Windows low-level hook).
///
/// Alt down is swallowed immediately (tap-hold): if a combo key follows, the
/// platform layer replays a synthetic Alt down (`inject_alt_down`) to restore
/// system/WebView Alt+combo behavior; if it was a bare tap, the up is
/// swallowed and dictation triggers. The WebView sees either a full down/up
/// pair or neither, and no "Alt still held" modifier state is left behind.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutState {
    alt_down: bool,
    alt_pending: bool,
    /// Which physical Alt key started the current gesture (meaningful only
    /// while `alt_down`); the platform layer reads it at combo replay time.
    /// If the other side is pressed while held, it is swallowed as a repeat
    /// and the recorded side stays the arming one, so a combo is replayed
    /// with the side that started the gesture, not the most recent press.
    alt_side: AltSide,
    /// The [Alt↓, combo↓] ordered replay has been issued for a combo key (the
    /// platform layer sets this whenever at least the Alt down was injected,
    /// complete or partial); the real Alt up must be let through to pair with
    /// the injected down, so no synthetic modifier state is left behind.
    alt_forwarded: bool,
    /// Space down was swallowed within this gesture; its up must be swallowed
    /// in pairs only, never the up of a Space that was already held before Alt
    /// went down (otherwise the system-level VK_SPACE gets stuck pressed).
    space_swallowed: bool,
    /// Foreground window handle at Alt down (0 = unknown); compared at Alt up
    /// so releasing Alt after alt-tabbing to another window does not misfire.
    alt_hwnd: isize,
    /// KBDLLHOOKSTRUCT.time of the previous event (millisecond tick; 0 = no
    /// event yet), used as the stale-gesture fallback (state stuck after
    /// UAC/lock-screen eats a keyup).
    last_event_ms: u32,
}

/// An inter-event gap above this threshold means the previous gesture is dead
/// (UAC/lock-screen swallowed the keyup) and the whole state resets. While
/// held normally, OS auto-repeat keeps refreshing the event stream and never
/// trips it; with accessibility settings that disable auto-repeat, a held
/// press longer than 2s no longer triggers (an accepted fallback cost).
const STALE_GESTURE_MS: u32 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutDecision {
    event: Option<VoiceShortcutEvent>,
    suppress: bool,
    /// The current combo down was swallowed: the platform layer must replay
    /// [Alt↓, combo↓] in order with a single SendInput (confirming
    /// alt_forwarded whenever at least the Alt down was injected — a partial
    /// injection still leaves a synthetic Alt down that the real Alt up must
    /// pair with), otherwise the system never sees the combo because Alt down
    /// was swallowed (Alt+Tab / Alt+F4 break).
    inject_alt_down: bool,
}

impl VoiceShortcutDecision {
    const fn pass() -> Self {
        Self {
            event: None,
            suppress: false,
            inject_alt_down: false,
        }
    }

    const fn suppress(event: Option<VoiceShortcutEvent>) -> Self {
        Self {
            event,
            suppress: true,
            inject_alt_down: false,
        }
    }

    /// Swallow the current combo down and let the platform layer replay it in
    /// order.
    const fn forward_combo() -> Self {
        Self {
            event: None,
            suppress: true,
            inject_alt_down: true,
        }
    }
}

/// `active` = this keystroke belongs to the shortcut gesture (switch on and
/// the foreground window is one of this process's target windows).
/// `foreground_hwnd` is the current foreground window handle (0 = none).
/// `time_ms` is KBDLLHOOKSTRUCT.time (millisecond tick, wrap-around allowed).
fn handle_voice_shortcut_key(
    state: &mut VoiceShortcutState,
    key: VoiceShortcutKey,
    key_down: bool,
    active: bool,
    foreground_hwnd: isize,
    time_ms: u32,
) -> VoiceShortcutDecision {
    // Stale-gesture fallback: a lost keyup (UAC/lock-screen etc.) can leave
    // alt_down stuck, so the first event of the next gesture would be
    // misread as a long-press repeat or ghost trigger. A gap above the
    // threshold resets everything and the event is then handled as fresh
    // (tick wrap-around handled with wrapping_sub).
    if state.last_event_ms != 0 && time_ms.wrapping_sub(state.last_event_ms) > STALE_GESTURE_MS {
        *state = VoiceShortcutState::default();
    }
    state.last_event_ms = time_ms;

    if !active {
        // Gesture landed on a non-target window: swallow nothing, only reset
        // on Alt up so state cannot leak into the next gesture.
        if matches!(key, VoiceShortcutKey::Alt(_)) && !key_down {
            *state = VoiceShortcutState::default();
        }
        return VoiceShortcutDecision::pass();
    }

    match (key, key_down) {
        (VoiceShortcutKey::Alt(side), true) => {
            if state.alt_down {
                // Long-press auto-repeat: keep swallowing when not forwarded,
                // let through when forwarded (consistent with the synthetic
                // down). Covers the other Alt side too: both downs are
                // swallowed and the first up resolves the gesture — releasing
                // the arming side fires dictation while the other side is
                // still held, whose auto-repeat re-arms and fires once more
                // on its own release. Holding both Alts is exotic enough that
                // this double trigger is accepted over per-side tracking.
                return if state.alt_forwarded {
                    VoiceShortcutDecision::pass()
                } else {
                    VoiceShortcutDecision::suppress(None)
                };
            }
            state.alt_down = true;
            state.alt_pending = true;
            state.alt_side = side;
            state.alt_forwarded = false;
            state.alt_hwnd = foreground_hwnd;
            VoiceShortcutDecision::suppress(None)
        }
        (VoiceShortcutKey::Alt(_), false) => {
            if !state.alt_down {
                return VoiceShortcutDecision::pass();
            }
            let hwnd_mismatch =
                state.alt_hwnd != 0 && foreground_hwnd != 0 && state.alt_hwnd != foreground_hwnd;
            let trigger = state.alt_pending && !hwnd_mismatch;
            let forwarded = state.alt_forwarded;
            *state = VoiceShortcutState::default();
            if forwarded {
                // Combo path: the real up is let through to pair with the
                // synthetic down; pending was cleared by the combo, so no
                // trigger.
                VoiceShortcutDecision::pass()
            } else {
                // Bare tap (or a combo of Space only): down was swallowed, so
                // swallow the up in pairs and trigger.
                VoiceShortcutDecision::suppress(
                    trigger.then_some(VoiceShortcutEvent::TriggerDictation),
                )
            }
        }
        (VoiceShortcutKey::Space, true) if state.alt_down => {
            // Alt+Space opens the window system menu: swallow down/up as a
            // pair and do not inject an Alt down for it.
            state.alt_pending = false;
            state.space_swallowed = true;
            VoiceShortcutDecision::suppress(None)
        }
        // Only swallow the Space up whose down was already swallowed; for a
        // Space held before Alt went down, its down was already let through,
        // so the up must be let through too or the system-level key sticks.
        (VoiceShortcutKey::Space, false) if state.alt_down && state.space_swallowed => {
            state.alt_pending = false;
            VoiceShortcutDecision::suppress(None)
        }
        // Alt+Esc (system window cycling) is treated the same as a regular
        // combo: without the replay a bare Esc leaks into the WebView, and
        // leftover pending would mis-trigger dictation at Alt up.
        (VoiceShortcutKey::Other | VoiceShortcutKey::Escape, true) if state.alt_down => {
            if state.alt_pending {
                state.alt_pending = false;
                if !state.alt_forwarded {
                    // alt_forwarded is confirmed by the platform layer whenever
                    // the replay injected at least the Alt down (partial
                    // included — the injected down must stay paired with the
                    // real Alt up); only a zero-injection replay failure leaves
                    // it unforwarded, the real Alt up is wrapped up along the
                    // unforwarded path, and no state is left behind.
                    return VoiceShortcutDecision::forward_combo();
                }
            }
            VoiceShortcutDecision::pass()
        }
        _ => VoiceShortcutDecision::pass(),
    }
}

#[derive(Clone, Serialize)]
struct VoiceShortcutTriggerPayload {
    mode: &'static str,
    source: &'static str,
    /// Target window label (a window mounting VoiceShortcutRouter); the
    /// frontend consumes it only after checking it matches its own window.
    window_label: String,
    /// Routing basis: "recording" (the targeted recording window, used for
    /// stop/mutual exclusion) or "focused" (the focused window, a normal
    /// trigger). The frontend uses this to spot a stale recording-window
    /// registration ("routed as the recording window but no longer has an
    /// active session" — the JS session is rebuilt after a WebView reload
    /// while the native registration was not cleared), clear it, and drop the
    /// event instead of ghost-opening the microphone in the background.
    route: &'static str,
}

/// Label of the window currently recording (synced by the frontend via a
/// command when recording starts/ends).
/// Used for cross-window recording mutual exclusion: while window A records,
/// window B's Alt gesture is routed to A (to stop it) and never opens a
/// second session.
static RECORDING_LABEL: Mutex<Option<String>> = Mutex::new(None);

/// Called by the `set_voice_shortcut_recording` command; the frontend syncs
/// its own window label when recording starts/ends/fails.
pub(crate) fn set_recording_label(label: Option<String>) {
    if let Ok(mut guard) = RECORDING_LABEL.lock() {
        *guard = label.filter(|value| !value.trim().is_empty());
    }
}

pub(crate) fn recording_label() -> Option<String> {
    RECORDING_LABEL.lock().ok().and_then(|guard| guard.clone())
}

fn clear_recording_label() {
    if let Ok(mut guard) = RECORDING_LABEL.lock() {
        *guard = None;
    }
}

/// Deregister proactively when a window is destroyed: if a recording window is
/// closed outright, the frontend never gets to run the finishVoiceInput
/// teardown. If the label is not cleared, the native hook keeps routing Alt
/// gestures into the destroyed window (emit does not error on a destroyed
/// window and the failure fallback never fires — effectively a global
/// swallow-keys black hole).
pub(crate) fn forget_recording_window(label: &str) {
    if recording_label().as_deref() == Some(label) {
        clear_recording_label();
    }
}

/// Only the main window and detached windows (DetachedShell) mount
/// VoiceShortcutRouter and can consume shortcut events; pet, code-reader,
/// artifact and similar windows are not whitelisted: no swallowing, no emit.
fn is_voice_shortcut_router_window(label: &str) -> bool {
    label == "main" || label.starts_with("detached-")
}

/// Routing for a bare Alt tap: the recording window wins (targeted stop,
/// cross-window mutual exclusion), otherwise the gesture targets the focused
/// whitelisted window; with neither, returns None (no swallowing, no emit).
/// The return value carries the routing basis so the frontend can tell apart a
/// stale recording-window registration (see payload.route).
fn resolve_trigger_target(
    recording_label: Option<&str>,
    focused_router_label: Option<&str>,
) -> Option<(String, &'static str)> {
    if let Some(label) = recording_label {
        return Some((label.to_string(), "recording"));
    }
    focused_router_label.map(|label| (label.to_string(), "focused"))
}

mod platform;

pub(crate) fn install(app: AppHandle) {
    platform::install(app);
}

pub(crate) fn set_enabled(enabled: bool) {
    platform::set_enabled(enabled);
}

/// Targeted emit: send only to the target window; silently dropped when there
/// is no focused/target window (no more broadcast to all windows).
fn emit_shortcut_event(
    app: &AppHandle,
    event: VoiceShortcutEvent,
    window_label: &str,
    route: &'static str,
) {
    match event {
        VoiceShortcutEvent::TriggerDictation => {
            let result = app.emit_to(
                window_label,
                "voice-shortcut:trigger",
                VoiceShortcutTriggerPayload {
                    mode: "dictation",
                    source: "native",
                    window_label: window_label.to_string(),
                    route,
                },
            );
            match result {
                Ok(()) => {
                    log::debug!(
                        "voice shortcut emitted event=TriggerDictation window={}",
                        window_label
                    );
                }
                Err(error) => {
                    log::warn!(
                        "voice shortcut emit failed event=TriggerDictation window={} error={}",
                        window_label,
                        error
                    );
                    // Target window already destroyed: if it is still recorded
                    // as the recording window, clear the stale label so later
                    // gestures are not black-holed.
                    if recording_label().as_deref() == Some(window_label) {
                        clear_recording_label();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HWND_A: isize = 100;
    const HWND_B: isize = 200;

    #[test]
    fn alt_tap_swallows_down_and_up_symmetrically_and_triggers() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert_eq!(down.event, None);
        assert!(down.suppress);
        assert!(!down.inject_alt_down);

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn alt_autorepeat_stays_swallowed_and_triggers_once() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        let repeat = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert_eq!(repeat, VoiceShortcutDecision::suppress(None));

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));

        let stray = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(stray, VoiceShortcutDecision::pass());
    }

    #[test]
    fn alt_combo_injects_alt_down_once_and_forwards_real_up() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );

        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        assert!(combo.suppress);

        // The platform layer confirms alt_forwarded after SendInput succeeds
        // (the [Alt↓, combo↓] replay is complete).
        state.alt_forwarded = true;

        // A second key during the same combo must not inject again.
        let combo2 =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo2, VoiceShortcutDecision::pass());

        // Alt auto-repeat down after a combo is let through, consistent with
        // the synthetic down.
        let repeat = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert_eq!(repeat, VoiceShortcutDecision::pass());

        // The real up is let through to pair with the synthetic down; no
        // dictation trigger.
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn alt_escape_combo_forwards_alt_down_once_and_never_triggers() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );

        // Alt+Esc is treated like a regular combo: swallow and replay
        // [Alt↓, Esc↓] in order, so Esc does not leak through bare.
        let escape =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Escape, true, true, HWND_A, 0);
        assert_eq!(escape, VoiceShortcutDecision::forward_combo());
        state.alt_forwarded = true;

        // The real Alt up is let through to pair with the synthetic down; no
        // dictation trigger.
        let alt_up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(alt_up, VoiceShortcutDecision::pass());
        assert!(!alt_up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn combo_replay_failure_leaves_no_residue() {
        // SendInput failure: alt_forwarded was never confirmed by the platform
        // layer and the combo is lost; the real Alt up is wrapped up along the
        // unforwarded path — no trigger, state reset, no "Alt still held"
        // residue.
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        // (the platform-layer confirmation is not simulated)

        let combo_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, false, true, HWND_A, 0);
        assert_eq!(combo_up, VoiceShortcutDecision::pass());

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::suppress(None));
        assert_eq!(up.event, None);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn partial_combo_replay_still_pairs_the_injected_alt_down_with_the_real_up() {
        // Partial SendInput injection (only the leading Alt↓ reached Windows):
        // the platform layer confirms alt_forwarded for it too, so the real
        // Alt up is let through to pair with the injected down — no stuck
        // modifier — and no dictation trigger fires. The combo keystroke
        // itself is lost, and no synthetic cleanup key is emitted.
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        // (the platform layer confirms the partial injection)
        state.alt_forwarded = true;

        // Alt auto-repeat while held is let through, consistent with the
        // injected down being held.
        let repeat = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert_eq!(repeat, VoiceShortcutDecision::pass());

        // The real up is let through to pair with the injected Alt down.
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::pass());
        assert!(!up.suppress);
        assert_eq!(up.event, None);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn stale_gesture_resets_after_lost_keyup() {
        // UAC/lock-screen swallowed the keyup: the gap to the previous event
        // far exceeds the threshold, so everything resets.
        // A later Alt up no longer ghost-triggers dictation, and the next
        // gesture works in full.
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            1000,
        );

        let ghost_up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            90000,
        );
        assert_eq!(ghost_up, VoiceShortcutDecision::pass());
        // Gesture state is reset (last_event_ms keeps the current tick for
        // later gap checks).
        assert!(!state.alt_down);
        assert!(!state.alt_pending);
        assert!(!state.alt_forwarded);

        // New gesture: down swallowed, up triggers — identical to the first
        // time.
        let down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            90500,
        );
        assert!(down.suppress);
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            90620,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn alt_repeat_within_gesture_does_not_trip_stale_reset() {
        // Held normally: OS auto-repeat keeps refreshing the event stream with
        // gaps far below the threshold, so no reset is triggered.
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            1000,
        );
        let repeat = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            1600,
        );
        assert_eq!(repeat, VoiceShortcutDecision::suppress(None));
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            1630,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn right_alt_tap_triggers_like_left_alt() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            true,
            true,
            HWND_A,
            0,
        );
        assert!(down.suppress);
        assert!(!down.inject_alt_down);

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn right_alt_combo_records_side_for_platform_replay() {
        // The state machine treats both sides identically, but the platform
        // layer must replay the swallowed combo with the VK of the Alt the
        // user actually held, so the recorded side must survive until the
        // combo down arrives.
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            true,
            true,
            HWND_A,
            0,
        );
        let combo =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, true, HWND_A, 0);
        assert_eq!(combo, VoiceShortcutDecision::forward_combo());
        assert_eq!(state.alt_side, AltSide::Right);
    }

    #[test]
    fn cross_side_hold_resolves_on_first_up_and_rearms_on_repeat() {
        // Documented cross-side hold: both downs are swallowed (the other side
        // is a repeat and the arming side sticks), the first up resolves the
        // gesture and fires once, and the still-held side's auto-repeat
        // re-arms, so its eventual release fires a second time. Per-side
        // tracking was deliberately rejected as not worth complicating the
        // main gesture path — this pins the accepted behavior.
        let mut state = VoiceShortcutState::default();
        let left_down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            1000,
        );
        assert!(left_down.suppress);

        let right_down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            true,
            true,
            HWND_A,
            1100,
        );
        assert_eq!(right_down, VoiceShortcutDecision::suppress(None));
        assert_eq!(state.alt_side, AltSide::Left);
        assert!(state.alt_pending);

        let left_up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            1200,
        );
        assert_eq!(left_up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert_eq!(state, VoiceShortcutState::default());

        let right_repeat = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            true,
            true,
            HWND_A,
            1300,
        );
        assert!(right_repeat.suppress);
        assert_eq!(state.alt_side, AltSide::Right);
        let right_up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Right),
            false,
            true,
            HWND_A,
            1400,
        );
        assert_eq!(right_up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn alt_space_suppresses_pair_and_never_triggers_or_injects() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );

        let space_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space_down, VoiceShortcutDecision::suppress(None));

        let space_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(space_up, VoiceShortcutDecision::suppress(None));

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::suppress(None));
        assert_eq!(up.event, None);
    }

    #[test]
    fn space_up_pressed_before_alt_is_not_swallowed() {
        // Space pressed before Alt: its down was already let through (the hook
        // only swallows Alt+Space), so while Alt is held the Space up must be
        // let through too, or the system-level VK_SPACE sticks pressed.
        let mut state = VoiceShortcutState::default();
        let space_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space_down, VoiceShortcutDecision::pass());

        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );

        let space_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(space_up, VoiceShortcutDecision::pass());

        // A genuine Alt+Space pressed later within the same gesture is still
        // swallowed as a pair.
        let pair_down =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(pair_down, VoiceShortcutDecision::suppress(None));
        let pair_up =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, false, true, HWND_A, 0);
        assert_eq!(pair_up, VoiceShortcutDecision::suppress(None));
    }

    #[test]
    fn space_then_alt_uses_plain_alt_only() {
        let mut state = VoiceShortcutState::default();
        let space =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true, HWND_A, 0);
        assert_eq!(space, VoiceShortcutDecision::pass());

        let alt = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert_eq!(alt, VoiceShortcutDecision::suppress(None));

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_A,
            0,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
    }

    #[test]
    fn alt_up_in_another_app_window_does_not_trigger() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        assert!(down.suppress);

        // Hold Alt, switch to another window of the same process, release: no
        // trigger; the down was swallowed, so the up is swallowed in pairs.
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            HWND_B,
            0,
        );
        assert_eq!(up.event, None);
        assert!(up.suppress);
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn unknown_foreground_hwnd_still_allows_trigger() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            0,
            0,
        );
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            true,
            0,
            0,
        );
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
    }

    #[test]
    fn shortcuts_are_ignored_when_no_target_window() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            false,
            HWND_A,
            0,
        );
        assert_eq!(down, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());

        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            false,
            HWND_A,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::pass());
    }

    #[test]
    fn gesture_state_resets_when_focus_leaves_target_mid_hold() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            true,
            true,
            HWND_A,
            0,
        );
        // Hold Alt while focus leaves the target window: keys in between are
        // all let through, and state resets at Alt up.
        let other =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Other, true, false, HWND_B, 0);
        assert_eq!(other, VoiceShortcutDecision::pass());
        let up = handle_voice_shortcut_key(
            &mut state,
            VoiceShortcutKey::Alt(AltSide::Left),
            false,
            false,
            HWND_B,
            0,
        );
        assert_eq!(up, VoiceShortcutDecision::pass());
        assert_eq!(state, VoiceShortcutState::default());
    }

    #[test]
    fn escape_is_not_emitted_without_frontend_state() {
        let mut state = VoiceShortcutState::default();
        let escape =
            handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Escape, true, true, HWND_A, 0);
        assert_eq!(escape, VoiceShortcutDecision::pass());
    }

    #[test]
    fn router_window_whitelist_only_covers_main_and_detached() {
        assert!(is_voice_shortcut_router_window("main"));
        assert!(is_voice_shortcut_router_window(
            "detached-session-0123456789abcdef"
        ));
        assert!(is_voice_shortcut_router_window(
            "detached-persona-fedcba9876543210"
        ));
        assert!(!is_voice_shortcut_router_window("pet"));
        assert!(!is_voice_shortcut_router_window("code-reader"));
        assert!(!is_voice_shortcut_router_window(
            "artifact-0123456789abcdef"
        ));
        assert!(!is_voice_shortcut_router_window(""));
    }

    #[test]
    fn recording_window_wins_over_focused_window() {
        assert_eq!(
            resolve_trigger_target(Some("main"), Some("detached-session-0123456789abcdef")),
            Some(("main".to_string(), "recording"))
        );
    }

    #[test]
    fn without_recording_routes_to_focused_router_window() {
        assert_eq!(
            resolve_trigger_target(None, Some("detached-session-0123456789abcdef")),
            Some(("detached-session-0123456789abcdef".to_string(), "focused"))
        );
    }

    #[test]
    fn no_recording_and_no_focused_router_window_means_no_emit() {
        assert_eq!(resolve_trigger_target(None, None), None);
    }
}
