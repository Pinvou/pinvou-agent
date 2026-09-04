import { isImeComposing } from '../../shared/ime-guard.mjs';

function normalizeVoiceShortcutMode(mode) {
  if (mode === 'edit' || mode === 'voice_edit' || mode === 'draft_edit') return 'edit';
  return mode === 'task' ? 'task' : 'dictation';
}

function isPlainAltKey(event) {
  return event
    && event.key === 'Alt'
    // Right Alt matches the Rust hook's policy (VK_RMENU classified as Other): reserved for
    // AltGr/IME, does not trigger the voice shortcut. location 2 = DOM_KEY_LOCATION_RIGHT;
    // fall back to code in environments where location is missing.
    && event.location !== 2
    && event.code !== 'AltRight'
    && !event.ctrlKey
    && !event.shiftKey
    && !event.metaKey;
}

function isAltSpaceKey(event) {
  return event
    && event.code === 'Space'
    && event.altKey
    && !event.ctrlKey
    && !event.shiftKey
    && !event.metaKey;
}

function shouldIgnoreVoiceShortcutEvent(event) {
  // Keys during IME composition (including the delayed keyCode 229 Enter/Esc dispatched by
  // WKWebView) belong to the IME candidate window and must not trigger the voice shortcut;
  // auto-repeat from holding a key is filtered for the same reason.
  return !event || event.repeat || Boolean(event.defaultPrevented) || isImeComposing(event);
}

function isActiveVoiceShortcutStatus(status) {
  return status === 'requesting_permission'
    || status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing';
}

// Windows combo passthrough injects a synthetic Alt down within the same event batch as the
// combo keydown (sent synchronously via SendInput), so leave some scheduling slack; a human
// "completing a bare Alt tap within 50ms of pressing a letter" is physically unreachable,
// so the window only needs to cover scheduling jitter.
const INJECTED_COMBO_WINDOW_MS = 50;

function voiceShortcutActionForKeyDown(event, current) {
  const state = current || {};
  const status = state.status || 'idle';

  if (isAltSpaceKey(event)) {
    return { type: 'clear_pending' };
  }

  if (isPlainAltKey(event)) {
    // When an Alt down is immediately followed by a non-Alt keydown (same injected batch),
    // it is the synthetic Alt down re-sent by combo passthrough: treat the whole gesture as
    // injected, so the real Alt up clears it instead of triggering (preventing a ghost
    // dictation from the "release Alt before the combo key" order).
    const injected = typeof state.lastNonAltKeyDownAt === 'number'
      && typeof state.now === 'number'
      && state.now - state.lastNonAltKeyDownAt >= 0
      && state.now - state.lastNonAltKeyDownAt <= INJECTED_COMBO_WINDOW_MS;
    return injected ? { type: 'pending_alt', injected: true } : { type: 'pending_alt' };
  }

  // While an Alt gesture is pending, any other key (including Esc) is a combo member:
  // clear pending, neither trigger nor cancel. Otherwise the Esc inside the passthrough
  // batch of Alt+Esc (system window cycling) would cancel an active recording too,
  // inconsistent with Alt+Tab (Other keys only clear pending).
  if (state.pendingAlt) return { type: 'clear_pending' };

  if (event && event.key === 'Escape') {
    return isActiveVoiceShortcutStatus(status) ? { type: 'cancel' } : { type: 'none' };
  }

  return { type: 'none' };
}

function voiceShortcutActionForKeyUp(event, current) {
  const state = current || {};
  if (!state.pendingAlt) return { type: 'none' };
  if (!isPlainAltKey(event)) {
    // Windows combo passthrough injects a synthetic Alt down after the bare
    // combo keydown, so the page sees [combo down, Alt down (injected),
    // combo up, real Alt up]. A human tap never has another key's keyup
    // between Alt down and Alt up, so a non-Alt keyup while pending means
    // the injected sequence is in flight — drop it instead of firing a
    // ghost dictation start (or stopping an active recording).
    return { type: 'clear_pending' };
  }
  if (state.pendingInjected) {
    // Reverse release order [combo down, Alt down (injected), real Alt up, combo up]:
    // Alt up arrives first, but this pending was marked as an injected sequence — clear
    // it without triggering.
    return { type: 'clear_pending' };
  }
  const status = state.status || 'idle';
  // Releasing Alt while a permission request is pending matches the native path: cancel the
  // pending voice start instead of triggering it again.
  if (status === 'requesting_permission') return { type: 'cancel' };
  const mode = normalizeVoiceShortcutMode(state.mode);
  if (status === 'recording') return { type: 'trigger', mode };
  return { type: 'trigger', mode: 'dictation' };
}

// While the shortcut intro modal is open, Esc belongs to the modal itself (same as clicking
// X to close); the router must not cancel a pending voice start at the same time. The modal
// component maintains this flag on mount/unmount.
let shortcutIntroOpen = false;

function setVoiceShortcutIntroOpen(open) {
  shortcutIntroOpen = !!open;
}

function isVoiceShortcutIntroOpen() {
  return shortcutIntroOpen;
}

export {
  isAltSpaceKey,
  isPlainAltKey,
  isActiveVoiceShortcutStatus,
  isVoiceShortcutIntroOpen,
  normalizeVoiceShortcutMode,
  setVoiceShortcutIntroOpen,
  shouldIgnoreVoiceShortcutEvent,
  voiceShortcutActionForKeyDown,
  voiceShortcutActionForKeyUp,
};
