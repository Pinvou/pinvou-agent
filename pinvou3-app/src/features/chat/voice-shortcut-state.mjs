function normalizeVoiceShortcutMode(mode) {
  return mode === 'task' ? 'task' : 'dictation';
}

function isPlainAltKey(event) {
  return event
    && event.key === 'Alt'
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
  return !event || event.repeat || Boolean(event.defaultPrevented);
}

function voiceShortcutActionForKeyDown(event, current) {
  const state = current || {};
  const status = state.status || 'idle';
  const mode = normalizeVoiceShortcutMode(state.mode);
  const recording = status === 'recording';

  if (event && event.key === 'Escape') {
    return status === 'idle' ? { type: 'none' } : { type: 'cancel' };
  }

  if (isAltSpaceKey(event)) {
    if (!recording) return { type: 'trigger', mode: 'task' };
    return mode === 'task' ? { type: 'trigger', mode: 'task' } : { type: 'none' };
  }

  if (isPlainAltKey(event)) {
    if (state.pendingSpace) {
      if (!recording) return { type: 'trigger', mode: 'task' };
      return mode === 'task' ? { type: 'trigger', mode: 'task' } : { type: 'none' };
    }
    if (!recording) return { type: 'pending_alt' };
    return mode === 'dictation' ? { type: 'pending_alt' } : { type: 'none' };
  }

  return { type: 'none' };
}

function voiceShortcutActionForKeyUp(event, current) {
  const state = current || {};
  if (!isPlainAltKey(event) || !state.pendingAlt) return { type: 'none' };
  const status = state.status || 'idle';
  const mode = normalizeVoiceShortcutMode(state.mode);
  if (status === 'recording' && mode !== 'dictation') return { type: 'none' };
  return { type: 'trigger', mode: 'dictation' };
}

export {
  isAltSpaceKey,
  isPlainAltKey,
  normalizeVoiceShortcutMode,
  shouldIgnoreVoiceShortcutEvent,
  voiceShortcutActionForKeyDown,
  voiceShortcutActionForKeyUp,
};
