function normalizeVoiceShortcutMode(mode) {
  if (mode === 'edit' || mode === 'voice_edit' || mode === 'draft_edit') return 'edit';
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
    return { type: 'clear_pending' };
  }

  if (isPlainAltKey(event)) {
    if (!recording) return { type: 'pending_alt' };
    return { type: 'pending_alt' };
  }

  return { type: 'none' };
}

function voiceShortcutActionForKeyUp(event, current) {
  const state = current || {};
  if (!isPlainAltKey(event) || !state.pendingAlt) return { type: 'none' };
  const status = state.status || 'idle';
  const mode = normalizeVoiceShortcutMode(state.mode);
  if (status === 'recording') return { type: 'trigger', mode };
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
