import React, { useEffect, useRef } from 'react';
import { isWeb } from '../../shared/platform.js';
import { listenTauri } from '../../platform/tauri/client.js';
import {
  shouldIgnoreVoiceShortcutEvent,
  voiceShortcutActionForKeyDown,
  voiceShortcutActionForKeyUp,
} from '../chat/voice-shortcut-state.mjs';
import { voiceShortcutEnabled } from '../chat/voice-shortcut-settings.mjs';
import { getActiveVoiceTarget } from './voice-target-registry.mjs';

function VoiceShortcutRouter({ enabled = true }) {
  const pendingRef = useRef(null);

  useEffect(() => {
    if (!enabled) return undefined;
    function setPendingShortcutFlag(flag) {
      pendingRef.current = {
        ...(pendingRef.current || {}),
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
      const action = voiceShortcutActionForKeyDown(event, {
        status,
        mode,
      });
      if (action.type === 'none') return;
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
      if (action.type === 'pending_alt') setPendingShortcutFlag('alt');
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
      });
      if (action.type === 'none') return;
      event.preventDefault();
      event.stopPropagation();
      clearPendingShortcut();
      if (action.type === 'trigger') {
        triggerVoiceShortcutTarget(target, action.mode, status, mode);
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
    if (!enabled || isWeb) return undefined;
    let disposed = false;
    const unlisteners = [];
    function rememberUnlisten(unlisten) {
      if (disposed) {
        try { unlisten(); } catch (_) {}
        return;
      }
      unlisteners.push(unlisten);
    }
    listenTauri('voice-shortcut:trigger', () => {
      pendingRef.current = null;
      const target = getActiveVoiceTarget();
      if (!target || typeof target.trigger !== 'function') return;
      const voiceInput = typeof target.getVoiceInput === 'function'
        ? target.getVoiceInput()
        : { status: 'idle' };
      const status = (voiceInput && voiceInput.status) || 'idle';
      const mode = (voiceInput && voiceInput.mode) || 'dictation';
      const recording = status === 'recording';
      if (!voiceShortcutEnabled() && !recording) return;
      if (recording) {
        target.trigger(mode, { source: 'shortcut-stop', preserveMode: true });
        return;
      }
      target.trigger('dictation');
    }).then(rememberUnlisten).catch(() => {});
    listenTauri('voice-shortcut:cancel', () => {
      pendingRef.current = null;
      const target = getActiveVoiceTarget();
      if (target && typeof target.cancel === 'function') target.cancel();
    }).then(rememberUnlisten).catch(() => {});
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => {
        try { unlisten(); } catch (_) {}
      });
    };
  }, [enabled]);

  return null;
}

export { VoiceShortcutRouter };
