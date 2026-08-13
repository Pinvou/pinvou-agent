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
    function clearPendingShortcutFlag(flag) {
      const current = pendingRef.current || {};
      const next = { ...current };
      delete next[flag];
      pendingRef.current = next.alt || next.space ? next : null;
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
    function handleVoiceShortcutKeyDown(event) {
      if (shouldIgnoreVoiceShortcutEvent(event)) return;
      const shortcutEnabled = voiceShortcutEnabled();
      if (!shortcutEnabled && !(event && event.key === 'Escape')) return;
      if (
        event
        && event.code === 'Space'
        && !event.altKey
        && !event.ctrlKey
        && !event.shiftKey
        && !event.metaKey
      ) {
        setPendingShortcutFlag('space');
        return;
      }
      const { target, status, mode } = currentTargetState();
      if (!target) return;
      const action = voiceShortcutActionForKeyDown(event, {
        status,
        mode,
        pendingSpace: Boolean(pendingRef.current && pendingRef.current.space),
      });
      if (action.type === 'none') return;
      event.preventDefault();
      event.stopPropagation();
      if (action.type === 'cancel') {
        clearPendingShortcut();
        if (typeof target.cancel === 'function') target.cancel();
        return;
      }
      if (action.type === 'trigger') {
        clearPendingShortcut();
        if (typeof target.trigger === 'function') target.trigger(action.mode);
        return;
      }
      if (action.type === 'pending_alt') setPendingShortcutFlag('alt');
    }
    function handleVoiceShortcutKeyUp(event) {
      if (!voiceShortcutEnabled()) {
        clearPendingShortcutFlag('space');
        return;
      }
      if (event && event.code === 'Space') clearPendingShortcutFlag('space');
      const { target, status, mode } = currentTargetState();
      if (!target) return;
      const action = voiceShortcutActionForKeyUp(event, {
        status,
        mode,
        pendingAlt: Boolean(pendingRef.current && pendingRef.current.alt),
      });
      if (action.type === 'none') return;
      event.preventDefault();
      event.stopPropagation();
      clearPendingShortcut();
      if (action.type === 'trigger' && typeof target.trigger === 'function') {
        target.trigger(action.mode);
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
    listenTauri('voice-shortcut:trigger', (event) => {
      pendingRef.current = null;
      if (!voiceShortcutEnabled()) return;
      const target = getActiveVoiceTarget();
      if (!target || typeof target.trigger !== 'function') return;
      const payload = event && Object.prototype.hasOwnProperty.call(event, 'payload')
        ? event.payload
        : event;
      target.trigger(payload && payload.mode === 'task' ? 'task' : 'dictation');
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
