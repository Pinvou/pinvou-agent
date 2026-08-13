use serde::Serialize;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutKey {
    Alt,
    Space,
    Escape,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VoiceShortcutEvent {
    TriggerDictation,
    Cancel,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutState {
    alt_down: bool,
    alt_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VoiceShortcutDecision {
    event: Option<VoiceShortcutEvent>,
    suppress: bool,
}

impl VoiceShortcutDecision {
    const fn pass() -> Self {
        Self {
            event: None,
            suppress: false,
        }
    }

    const fn suppress(event: Option<VoiceShortcutEvent>) -> Self {
        Self {
            event,
            suppress: true,
        }
    }
}

fn handle_voice_shortcut_key(
    state: &mut VoiceShortcutState,
    key: VoiceShortcutKey,
    key_down: bool,
    foreground: bool,
) -> VoiceShortcutDecision {
    if !foreground {
        if !key_down {
            match key {
                VoiceShortcutKey::Alt => {
                    state.alt_down = false;
                    state.alt_pending = false;
                }
                _ => {}
            }
        }
        return VoiceShortcutDecision::pass();
    }

    match (key, key_down) {
        (VoiceShortcutKey::Alt, true) => {
            state.alt_down = true;
            state.alt_pending = true;
            VoiceShortcutDecision::suppress(None)
        }
        (VoiceShortcutKey::Alt, false) => {
            let should_trigger = state.alt_pending;
            state.alt_down = false;
            state.alt_pending = false;
            VoiceShortcutDecision::suppress(
                should_trigger.then_some(VoiceShortcutEvent::TriggerDictation),
            )
        }
        (VoiceShortcutKey::Space, true) if state.alt_down => {
            state.alt_pending = false;
            VoiceShortcutDecision::suppress(None)
        }
        (VoiceShortcutKey::Escape, true) => VoiceShortcutDecision {
            event: Some(VoiceShortcutEvent::Cancel),
            suppress: false,
        },
        _ => VoiceShortcutDecision::pass(),
    }
}

#[derive(Clone, Serialize)]
struct VoiceShortcutTriggerPayload {
    mode: &'static str,
    source: &'static str,
}

#[derive(Clone, Serialize)]
struct VoiceShortcutCancelPayload {
    source: &'static str,
}

mod platform;

pub(crate) fn install(app: AppHandle) {
    platform::install(app);
}

fn emit_shortcut_event(app: &AppHandle, event: VoiceShortcutEvent) {
    match event {
        VoiceShortcutEvent::TriggerDictation => {
            let result = app.emit(
                "voice-shortcut:trigger",
                VoiceShortcutTriggerPayload {
                    mode: "dictation",
                    source: "native",
                },
            );
            eprintln!(
                "[pinvou3-app] voice shortcut emitted event=TriggerDictation ok={}",
                result.is_ok()
            );
        }
        VoiceShortcutEvent::Cancel => {
            let result = app.emit(
                "voice-shortcut:cancel",
                VoiceShortcutCancelPayload { source: "native" },
            );
            eprintln!(
                "[pinvou3-app] voice shortcut emitted event=Cancel ok={}",
                result.is_ok()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_key_up_triggers_dictation() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true);
        assert_eq!(down.event, None);
        assert!(down.suppress);

        let up = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
    }

    #[test]
    fn alt_space_does_not_trigger_task_or_followup_dictation() {
        let mut state = VoiceShortcutState::default();
        handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true);

        let space = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true);
        assert_eq!(space.event, None);
        assert!(space.suppress);

        let up = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true);
        assert_eq!(up.event, None);
        assert!(up.suppress);
    }

    #[test]
    fn space_then_alt_uses_plain_alt_only() {
        let mut state = VoiceShortcutState::default();
        let space = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Space, true, true);
        assert_eq!(space, VoiceShortcutDecision::pass());

        let alt = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, true);
        assert_eq!(alt.event, None);
        assert!(alt.suppress);

        let up = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, false, true);
        assert_eq!(up.event, Some(VoiceShortcutEvent::TriggerDictation));
        assert!(up.suppress);
    }

    #[test]
    fn shortcuts_are_ignored_outside_app_foreground() {
        let mut state = VoiceShortcutState::default();
        let down = handle_voice_shortcut_key(&mut state, VoiceShortcutKey::Alt, true, false);
        assert_eq!(down, VoiceShortcutDecision::pass());
    }
}
