import { useEffect, useRef } from 'react';
import { isWeb } from '../../shared/platform.js';
import { listenTauri, tryGetCurrentTauriWindow, tryGetTauriBridge } from '../../platform/tauri/client.js';
import {
  isPlainAltKey,
  isVoiceShortcutIntroOpen,
  shouldIgnoreVoiceShortcutEvent,
  voiceShortcutActionForKeyDown,
  voiceShortcutActionForKeyUp,
} from '../chat/voice-shortcut-state.mjs';
import {
  VOICE_SHORTCUT_ENABLED_KEY,
  VOICE_SHORTCUT_SETTINGS_EVENT,
  voiceShortcutEnabled,
} from '../chat/voice-shortcut-settings.mjs';
import { getActiveVoiceTarget } from './voice-target-registry.mjs';

// Native shortcut events must be handled only by the target window: the Rust payload carries
// window_label (aimed at the focused window), so consume it only after verifying it matches
// here; legacy events without a label pass through for compatibility. Also pass through when
// this window's label cannot be determined (Rust has already targeted the focused window;
// the sender side bears the misdelivery risk).
function isVoiceShortcutEventForThisWindow(payload) {
  const eventLabel = payload && typeof payload.window_label === 'string' ? payload.window_label : '';
  if (!eventLabel) return true;
  const ownLabel = (tryGetCurrentTauriWindow() || {}).label || '';
  return !ownLabel || ownLabel === eventLabel;
}

function VoiceShortcutRouter({ enabled = true }) {
  const pendingRef = useRef(null);
  // Timestamp of the most recent non-plain-Alt keydown: Windows combo passthrough injects a
  // synthetic Alt down in the same event batch as the combo keydown, and the state machine
  // uses this to mark that pending as an injected sequence (see voice-shortcut-state's
  // INJECTED_COMBO_WINDOW_MS).
  const lastNonAltKeyDownAtRef = useRef(null);

  useEffect(() => {
    if (!enabled || isWeb) return;
    function syncNativeShortcutSetting() {
      const bridge = tryGetTauriBridge();
      if (!bridge || !bridge.available || !bridge.voice
        || typeof bridge.voice.setVoiceShortcutEnabled !== 'function') return;
      bridge.voice.setVoiceShortcutEnabled(voiceShortcutEnabled());
    }
    // The authoritative state (settings.json) is replayed into the native
    // layer by Rust at startup; mounting no longer pushes the localStorage
    // mirror back into the native layer and settings.json — after WebView
    // storage is cleared the mirror defaults to false, and pushing it back
    // would overwrite an authoritative true with false. Only follow the
    // user's explicit changes here: same-window writes arrive via
    // CustomEvent, cross-window writes via storage (voice-shortcut key
    // only). A null event.key means localStorage.clear(), which wipes the
    // mirror but says nothing about the authoritative setting, so it must
    // be ignored just like any unrelated key.
    function handleShortcutStorageEvent(event) {
      if (!event || event.key !== VOICE_SHORTCUT_ENABLED_KEY) return;
      syncNativeShortcutSetting();
    }
    window.addEventListener(VOICE_SHORTCUT_SETTINGS_EVENT, syncNativeShortcutSetting);
    window.addEventListener('storage', handleShortcutStorageEvent);
    return () => {
      window.removeEventListener(VOICE_SHORTCUT_SETTINGS_EVENT, syncNativeShortcutSetting);
      window.removeEventListener('storage', handleShortcutStorageEvent);
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabled) return;
    function setPendingShortcutFlag(flag) {
      pendingRef.current = {
        ...pendingRef.current,
        [flag]: true,
      };
    }
    function clearPendingShortcut() {
      pendingRef.current = null;
    }
    function currentTargetState() {
      const target = getActiveVoiceTarget();
      const voiceInput = target && typeof target.getVoiceInput === 'function'
        ? target.getVoiceInput()
        : { status: 'idle' };
      return {
        target,
        status: (voiceInput && voiceInput.status) || 'idle',
        mode: (voiceInput && voiceInput.mode) || 'dictation',
      };
    }
    function triggerVoiceShortcutTarget(target, actionMode, status, activeMode) {
      if (!target || typeof target.trigger !== 'function') return;
      if (status === 'recording') {
        target.trigger(activeMode || 'dictation', { source: 'shortcut-stop', preserveMode: true });
        return;
      }
      target.trigger(actionMode || 'dictation');
    }
    function handleVoiceShortcutKeyDown(event) {
      if (shouldIgnoreVoiceShortcutEvent(event)) return;
      const shortcutEnabled = voiceShortcutEnabled();
      const { target, status, mode } = currentTargetState();
      if (!target) return;
      const recording = status === 'recording';
      if (!shortcutEnabled && !(event && (event.key === 'Escape' || (event.key === 'Alt' && recording)))) return;
      if (!isPlainAltKey(event)) lastNonAltKeyDownAtRef.current = Date.now();
      const action = voiceShortcutActionForKeyDown(event, {
        status,
        mode,
        pendingAlt: Boolean(pendingRef.current && pendingRef.current.alt),
        pendingInjected: Boolean(pendingRef.current && pendingRef.current.injected),
        now: Date.now(),
        lastNonAltKeyDownAt: lastNonAltKeyDownAtRef.current,
      });
      if (action.type === 'none') return;
      // While the intro modal is open, Esc yields to the modal's own close handling
      // (no preventDefault, no voice-start cancel).
      if (action.type === 'cancel' && isVoiceShortcutIntroOpen()) return;
      event.preventDefault();
      event.stopPropagation();
      if (action.type === 'clear_pending') {
        clearPendingShortcut();
        return;
      }
      if (action.type === 'cancel') {
        clearPendingShortcut();
        if (typeof target.cancel === 'function') target.cancel();
        return;
      }
      if (action.type === 'trigger') {
        clearPendingShortcut();
        triggerVoiceShortcutTarget(target, action.mode, status, mode);
        return;
      }
      if (action.type === 'pending_alt') {
        setPendingShortcutFlag('alt');
        if (action.injected) setPendingShortcutFlag('injected');
      }
    }
    function handleVoiceShortcutKeyUp(event) {
      const { target, status, mode } = currentTargetState();
      if (!target) return;
      const recording = status === 'recording';
      if (!voiceShortcutEnabled() && !(event && event.key === 'Alt' && recording)) {
        clearPendingShortcut();
        return;
      }
      const action = voiceShortcutActionForKeyUp(event, {
        status,
        mode,
        pendingAlt: Boolean(pendingRef.current && pendingRef.current.alt),
        pendingInjected: Boolean(pendingRef.current && pendingRef.current.injected),
      });
      if (action.type === 'none') return;
      if (action.type === 'clear_pending') {
        // Tail keyup of a passthrough combo while an injected Alt pair is in
        // flight (see voice-shortcut-state): drop the gesture, but without
        // preventDefault so the combo key's own keyup handlers keep working.
        clearPendingShortcut();
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      clearPendingShortcut();
      if (action.type === 'trigger') {
        triggerVoiceShortcutTarget(target, action.mode, status, mode);
        return;
      }
      if (action.type === 'cancel' && typeof target.cancel === 'function') {
        target.cancel();
      }
    }
    window.addEventListener('keydown', handleVoiceShortcutKeyDown, true);
    window.addEventListener('keyup', handleVoiceShortcutKeyUp, true);
    return () => {
      window.removeEventListener('keydown', handleVoiceShortcutKeyDown, true);
      window.removeEventListener('keyup', handleVoiceShortcutKeyUp, true);
      clearPendingShortcut();
    };
  }, [enabled]);

  useEffect(() => {
    if (!enabled || isWeb) return;
    let disposed = false;
    const unlisteners = [];
    function rememberUnlisten(unlisten) {
      if (disposed) {
        try { unlisten(); } catch { /* router already disposed */ }
        return;
      }
      unlisteners.push(unlisten);
    }
    listenTauri('voice-shortcut:trigger', (event) => {
      const payload = event && event.payload;
      if (!isVoiceShortcutEventForThisWindow(payload)) return;
      pendingRef.current = null;
      const target = getActiveVoiceTarget();
      if (!target || typeof target.trigger !== 'function') return;
      const voiceInput = typeof target.getVoiceInput === 'function'
        ? target.getVoiceInput()
        : { status: 'idle' };
      const status = (voiceInput && voiceInput.status) || 'idle';
      const mode = (voiceInput && voiceInput.mode) || 'dictation';
      const recording = status === 'recording';
      // Native routed this window as "the recording window" but no recording is active here:
      // usually a WebView reload/restore rebuilt the JS session while the native registration
      // stayed (native clears it only on window destroy). Clear the stale registration and
      // drop this gesture; the next press routes by focused window again. Never ghost-open
      // the mic in the background.
      if (payload && payload.route === 'recording' && !recording) {
        const bridge = tryGetTauriBridge();
        if (bridge && bridge.available
          && typeof bridge.voice.syncVoiceShortcutRecording === 'function') {
          bridge.voice.syncVoiceShortcutRecording(null);
        }
        return;
      }
      // Native events are only emitted when the Rust-side switch is on (the hook entry
      // short-circuits on !shortcut_enabled()), so authoritative gating already happened in
      // the native layer; do not stack a localStorage mirror check here — after WebView
      // storage is cleared the mirror defaults to false, and stacking the check would let a
      // natively enabled shortcut be swallowed by native and then dropped by the frontend,
      // failing silently. The mirror only serves the in-window key gesture channel above.
      if (recording) {
        target.trigger(mode, { source: 'shortcut-stop', preserveMode: true });
        return;
      }
      target.trigger('dictation');
    }).then(rememberUnlisten).catch(() => {});
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => {
        try { unlisten(); } catch { /* listener already gone */ }
      });
    };
  }, [enabled]);

  return null;
}

export { VoiceShortcutRouter };
