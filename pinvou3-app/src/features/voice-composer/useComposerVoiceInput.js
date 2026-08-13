import { useCallback, useEffect, useRef, useState } from 'react';
import {
  getActiveVoiceTarget,
  isActiveVoiceTarget,
  registerVoiceTarget,
} from './voice-target-registry.mjs';

function normalizeMode(mode) {
  if (mode === 'task') return 'task';
  if (mode === 'structured' || mode === 'list' || mode === 'structured_dictation') return 'structured';
  if (mode === 'edit' || mode === 'voice_edit' || mode === 'draft_edit') return 'edit';
  return 'dictation';
}

function activeStatus(status) {
  return status === 'requesting_permission'
    || status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing';
}

function createVoiceSessionId(targetId) {
  const random = Math.random().toString(36).slice(2, 8);
  return `${targetId || 'voice'}:${Date.now().toString(36)}:${random}`;
}

function trimDraft(value) {
  return String(value || '').trim();
}

function useComposerVoiceInput(adapter) {
  const adapterRef = useRef(adapter);
  const [voiceSessionId, setVoiceSessionId] = useState(null);
  const [editPreview, setEditPreview] = useState(null);
  const voiceSessionIdRef = useRef(null);
  const editPreviewRef = useRef(null);
  adapterRef.current = adapter;
  editPreviewRef.current = editPreview;

  const cancelVoice = useCallback(() => {
    const current = adapterRef.current || {};
    if (current.bridge && current.bridge.available) current.bridge.voice.cancelVoiceInput();
  }, []);

  const closeVoice = useCallback(() => {
    const current = adapterRef.current || {};
    if (current.bridge && current.bridge.available) current.bridge.voice.clearVoiceInput();
  }, []);

  const cancelVoiceEditPreview = useCallback(() => {
    setEditPreview(null);
  }, []);

  const cancelVoiceOrPreview = useCallback(() => {
    if (editPreviewRef.current) {
      setEditPreview(null);
      return;
    }
    const current = adapterRef.current || {};
    if (current.bridge && current.bridge.available) current.bridge.voice.cancelVoiceInput();
  }, []);

  const applyVoiceEditPreview = useCallback(async (options = {}) => {
    const current = adapterRef.current || {};
    const preview = editPreview;
    if (!preview) return false;
    const next = trimDraft(preview.next);
    if (!next) {
      setEditPreview(null);
      return false;
    }
    current.setDraft(next);
    setEditPreview(null);
    if (!options.send) return true;
    if (typeof current.canSendTask === 'function' && !current.canSendTask(next, { mode: 'edit', preview })) {
      if (typeof current.onTaskBlocked === 'function') current.onTaskBlocked('gate', next, { mode: 'edit', preview });
      return false;
    }
    if (typeof current.sendTask !== 'function') return false;
    const accepted = await current.sendTask(next, { mode: 'edit', preview });
    if (accepted && typeof current.onTaskAccepted === 'function') current.onTaskAccepted(next, { mode: 'edit', preview });
    return !!accepted;
  }, [editPreview]);

  const clearStaleVoiceState = useCallback((targetId, sessionId) => {
    const current = adapterRef.current || {};
    const active = getActiveVoiceTarget();
    if (active && active.targetId === targetId && active.voiceSessionId === sessionId
      && current.bridge && current.bridge.available) {
      current.bridge.voice.clearVoiceInput();
    }
  }, []);

  const handleVoiceResult = useCallback(async (sessionId, text, draftBeforeStart, context) => {
    const current = adapterRef.current || {};
    const targetId = current.targetId;
    if (!targetId || !isActiveVoiceTarget(targetId, sessionId)) {
      clearStaleVoiceState(targetId, sessionId);
      return;
    }
    if (typeof current.isStillActive === 'function' && !current.isStillActive()) {
      clearStaleVoiceState(targetId, sessionId);
      return;
    }

    const recognized = String(text || '').trim();
    if (!recognized) return;

    const mode = normalizeMode(context && context.mode);
    if (mode === 'edit') {
      const original = trimDraft(draftBeforeStart);
      const next = trimDraft(recognized);
      if (!original || !next || next === original) {
        if (next === original && typeof current.onEditUnchanged === 'function') {
          current.onEditUnchanged({ original, instruction: trimDraft(context && context.rawText), context });
        }
        return;
      }
      current.setDraft(next);
      setEditPreview(null);
      return;
    }

    const appendDraft = current.appendDraft
      || (current.bridge && current.bridge.voice && current.bridge.voice.appendVoiceText)
      || ((base, value) => `${String(base || '').trimEnd()}\n${String(value || '').trim()}`.trim());

    if (mode !== 'task') {
      current.setDraft(prev => appendDraft(prev, recognized));
      return;
    }

    const outgoing = appendDraft(draftBeforeStart || '', recognized);
    if (!String(outgoing || '').trim()) return;
    current.setDraft(outgoing);
    if (context && context.diagnostic && context.diagnostic.task_send_blocked) {
      if (typeof current.onTaskBlocked === 'function') current.onTaskBlocked('diagnostic', outgoing, context);
      return;
    }
    if (typeof current.canSendTask === 'function' && !current.canSendTask(outgoing, context)) {
      if (typeof current.onTaskBlocked === 'function') current.onTaskBlocked('gate', outgoing, context);
      return;
    }
    if (typeof current.sendTask !== 'function') return;
    const accepted = await current.sendTask(outgoing, context);
    if (accepted && typeof current.onTaskAccepted === 'function') current.onTaskAccepted(outgoing, context);
  }, [clearStaleVoiceState]);

  const triggerVoice = useCallback((mode = 'dictation', options = {}) => {
    const current = adapterRef.current || {};
    const bridge = current.bridge;
    const voiceInput = current.voiceInput || { status: 'idle' };
    const voiceBusy = !!current.voiceBusy;
    let nextMode = normalizeMode(mode);
    if (typeof current.resolveMode === 'function') {
      nextMode = normalizeMode(current.resolveMode(nextMode, {
        source: options.source || 'shortcut',
        draft: typeof current.getDraft === 'function' ? current.getDraft() : '',
        voiceInput,
      }));
    } else if (nextMode === 'dictation' && options.source !== 'button'
      && trimDraft(typeof current.getDraft === 'function' ? current.getDraft() : '')) {
      nextMode = 'edit';
    }
    if (!bridge || !bridge.available) return;
    if (voiceInput.status === 'requesting_permission') {
      bridge.voice.cancelVoiceInput();
      return;
    }
    if (voiceBusy) return;
    if (typeof current.canStart === 'function' && !current.canStart(nextMode)) return;
    if (!options.skipBeforeStart && typeof current.onBeforeStart === 'function'
      && current.onBeforeStart(nextMode) === false) {
      return;
    }

    if (voiceInput.status === 'recording') {
      if (normalizeMode(voiceInput.mode) !== nextMode) return;
      bridge.voice.startVoiceInput(
        typeof current.getDraft === 'function' ? current.getDraft() : '',
        (text, draftBeforeStart, context) => handleVoiceResult(
          voiceSessionIdRef.current,
          text,
          draftBeforeStart,
          context,
        ),
        { mode: nextMode },
      );
      return;
    }

    const sessionId = createVoiceSessionId(current.targetId);
    voiceSessionIdRef.current = sessionId;
    setVoiceSessionId(sessionId);
    bridge.voice.startVoiceInput(
      typeof current.getDraft === 'function' ? current.getDraft() : '',
      (text, draftBeforeStart, context) => handleVoiceResult(sessionId, text, draftBeforeStart, context),
      { mode: nextMode },
    );
  }, [handleVoiceResult]);

  useEffect(() => {
    const current = adapterRef.current || {};
    const status = current.voiceInput && current.voiceInput.status;
    if (!activeStatus(status)) {
      voiceSessionIdRef.current = null;
      setVoiceSessionId(null);
    }
  }, [adapter.voiceInput && adapter.voiceInput.status]);

  useEffect(() => {
    const current = adapterRef.current || {};
    if (!current.targetId) return undefined;
    return registerVoiceTarget({
      targetId: current.targetId,
      ownerKind: current.ownerKind,
      voiceSessionId,
      workspaceId: current.workspaceId,
      sessionId: current.sessionId,
      getVoiceInput: () => {
        const latest = adapterRef.current || {};
        return latest.voiceInput || { status: 'idle' };
      },
      isStillActive: () => {
        const latest = adapterRef.current || {};
        return typeof latest.isStillActive === 'function' ? latest.isStillActive() : true;
      },
      trigger: triggerVoice,
      cancel: cancelVoiceOrPreview,
      cancelPreview: cancelVoiceEditPreview,
    });
  }, [
    adapter.targetId,
    adapter.ownerKind,
    adapter.workspaceId,
    adapter.sessionId,
    voiceSessionId,
    triggerVoice,
    cancelVoiceOrPreview,
    cancelVoiceEditPreview,
  ]);

  return {
    voiceSessionId,
    editPreview,
    triggerVoice,
    cancelVoice,
    closeVoice,
    cancelVoiceEditPreview,
    applyVoiceEditPreview,
  };
}

export { useComposerVoiceInput };
