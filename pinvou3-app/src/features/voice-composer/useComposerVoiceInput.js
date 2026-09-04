import { useCallback, useEffect, useRef, useState } from 'react';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { voicePostprocessEnabled } from '../chat/voice-shortcut-settings.mjs';
import {
  getActiveVoiceTarget,
  isActiveVoiceTarget,
  registerVoiceTarget,
} from './voice-target-registry.mjs';

function normalizeMode(mode) {
  if (mode === 'task') return 'task';
  if (['edit', 'voice_edit', 'draft_edit'].includes(mode)) return 'edit';
  return 'dictation';
}

function activeStatus(status) {
  return ['requesting_permission', 'recording', 'transcribing', 'postprocessing'].includes(status);
}

let fallbackVoiceSessionCounter = 0;

function createVoiceSessionRandomPart() {
  // Contract tests load this file in a vm slice; a bare crypto reference throws a
  // ReferenceError in a context without injections. Read it via globalThis to get Web Crypto
  // true randomness, falling back only when it is missing.
  const cryptoApi = (typeof globalThis !== 'undefined' && globalThis.crypto) || null;
  if (cryptoApi && typeof cryptoApi.randomUUID === 'function') {
    return cryptoApi.randomUUID();
  }
  if (cryptoApi && typeof cryptoApi.getRandomValues === 'function') {
    const values = new Uint32Array(2);
    cryptoApi.getRandomValues(values);
    return `${values[0].toString(36)}${values[1].toString(36)}`;
  }
  fallbackVoiceSessionCounter += 1;
  return `fallback-${Date.now().toString(36)}-${fallbackVoiceSessionCounter.toString(36)}`;
}

function createVoiceSessionId(targetId) {
  return `${targetId || 'voice'}:${Date.now().toString(36)}:${createVoiceSessionRandomPart()}`;
}

function trimDraft(value) {
  return String(value || '').trim();
}

// Thrown to the bridge layer when direct task send is not accepted (blocked by a guard or
// threw): the catch in bridge finishVoiceInput normalizes it into a failure notification
// instead of leaving the fake "voice task sent" success showing.
// message is left empty and toString overridden so normalizeVoiceError lands on the existing
// trilingual generic copy voiceInputFailed; the bridge can map dedicated copy for the
// send_failed category later.
function createVoiceTaskSendError() {
  const error = new Error('');
  error.category = 'send_failed';
  error.stage = 'writeback';
  error.toString = () => '';
  return error;
}

// Task-send teardown: pass through sendTask's real result. By contract sendTask resolves a
// boolean; an explicit false or a throw counts as not accepted (failure notified via
// onTaskBlocked('send')), while legacy implementations returning no verdict (undefined) keep
// the existing "accepted" behavior so a success is not misreported as a failure.
// Skip onTaskAccepted when the draft changed during the await window, so an unconditional
// clear cannot swallow the new input.
// In-flight dedup and failure rendering are the caller's responsibility.
async function deliverVoiceTask(current, text, context, inFlightRef) {
  inFlightRef.current = true;
  let accepted;
  try {
    accepted = await current.sendTask(text, context);
  } catch {
    accepted = false;
  } finally {
    inFlightRef.current = false;
  }
  if (accepted === false) {
    if (typeof current.onTaskBlocked === 'function') current.onTaskBlocked('send', text, context);
    return false;
  }
  const draftUntouched = typeof current.getDraft !== 'function'
    || trimDraft(current.getDraft()) === trimDraft(text);
  if (draftUntouched && typeof current.onTaskAccepted === 'function') {
    current.onTaskAccepted(text, context);
  }
  return true;
}

function useComposerVoiceInput(adapter) {
  const adapterRef = useRef(adapter);
  const [voiceSessionId, setVoiceSessionId] = useState(null);
  const [editPreview, setEditPreview] = useState(null);
  const voiceSessionIdRef = useRef(null);
  const editPreviewRef = useRef(null);
  const taskSendInFlightRef = useRef(false);
  // eslint-disable-next-line react-hooks/refs -- latest-adapter mirrors read from timers/events outside the render loop
  adapterRef.current = adapter;
  // eslint-disable-next-line react-hooks/refs -- latest edit preview mirror for the send path
  editPreviewRef.current = editPreview;

  // bridge cancelVoiceInput/clearVoiceInput diverge while a session is in flight: cancel only
  // ends the session as cancelled (an invisible residue state), while clear actually resets to
  // idle. Combine both here as "cancel + reset" so no cancelled residue remains after
  // cancel/close.
  const cancelVoice = useCallback(() => {
    const current = adapterRef.current || {};
    if (!current.bridge || !current.bridge.available) return;
    current.bridge.voice.cancelVoiceInput();
    current.bridge.voice.clearVoiceInput();
  }, []);

  const closeVoice = useCallback(() => {
    const current = adapterRef.current || {};
    if (!current.bridge || !current.bridge.available) return;
    current.bridge.voice.cancelVoiceInput();
    current.bridge.voice.clearVoiceInput();
  }, []);

  const cancelVoiceEditPreview = useCallback(() => {
    setEditPreview(null);
    closeVoice();
  }, [closeVoice]);

  const cancelVoiceOrPreview = useCallback(() => {
    if (editPreviewRef.current) {
      setEditPreview(null);
      closeVoice();
      return;
    }
    const current = adapterRef.current || {};
    if (!current.bridge || !current.bridge.available) return;
    current.bridge.voice.cancelVoiceInput();
    current.bridge.voice.clearVoiceInput();
  }, [closeVoice]);

  const applyVoiceEditPreview = useCallback(async (options = {}) => {
    const current = adapterRef.current || {};
    const preview = editPreview;
    if (!preview) return false;
    const next = trimDraft(preview.next);
    if (!next) {
      setEditPreview(null);
      return false;
    }
    // Ignore repeat triggers while a send is in flight (double click, or global Enter and
    // textarea Enter firing together) to prevent double sends.
    if (options.send && taskSendInFlightRef.current) return false;
    // The preview rewrites against the draft snapshot taken at recording start (original),
    // but the input box stays hand-editable while the preview is pending. If the draft has
    // drifted from original, confirming would overwrite the whole draft with a rewrite based
    // on the old text, silently discarding what the user typed (same family as a new voice
    // session discarding a stale preview) — this is the last gate before applying: discard
    // the preview and keep the draft as-is.
    if (typeof current.getDraft === 'function'
      && trimDraft(current.getDraft()) !== preview.original) {
      setEditPreview(null);
      return false;
    }
    current.setDraft(next);
    setEditPreview(null);
    closeVoice();
    if (!options.send) return true;
    if (typeof current.canSendTask === 'function' && !current.canSendTask(next, { mode: 'edit', preview })) {
      if (typeof current.onTaskBlocked === 'function') current.onTaskBlocked('gate', next, { mode: 'edit', preview });
      return false;
    }
    if (typeof current.sendTask !== 'function') return false;
    return deliverVoiceTask(current, next, { mode: 'edit', preview }, taskSendInFlightRef);
  }, [editPreview, closeVoice]);

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
      setEditPreview({
        original,
        next,
        instruction: trimDraft(context && context.rawText),
        context,
      });
      return;
    }

    const appendDraft = current.appendDraft
      || (current.bridge && current.bridge.voice && current.bridge.voice.appendVoiceText)
      || ((base, value) => `${String(base || '').trimEnd()}\n${String(value || '').trim()}`.trim());

    if (mode !== 'task') {
      current.setDraft(prev => appendDraft(prev, recognized));
      return;
    }

    // Same as the dictation branch: merge functionally on top of the latest draft. Characters
    // the user typed during recording/transcription must not be overwritten by the stale
    // draftBeforeStart snapshot. adapter refreshes adapterRef on every render, so getDraft()
    // here returns the latest input at writeback time.
    const baseDraft = typeof current.getDraft === 'function' ? current.getDraft() : (draftBeforeStart || '');
    const outgoing = appendDraft(baseDraft, recognized);
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
    if (typeof current.sendTask !== 'function' || taskSendInFlightRef.current) return;
    const accepted = await deliverVoiceTask(current, outgoing, context, taskSendInFlightRef);
    if (!accepted) {
      // When send's guard returns silently, the user still must see the failure: keep the
      // draft in the input box and throw to the bridge so the fake "task sent" success
      // notification is flipped into a failure notification.
      throw createVoiceTaskSendError();
    }
  }, [clearStaleVoiceState]);

  // Returns true only when a fresh voice session was started (the final
  // bridge.voice.startVoiceInput call). Every other branch — including the
  // cancel/stop side effects for requesting_permission and recording
  // statuses — returns false. Callers that stash a pending auto-start
  // intent before the first-use intro (see ChatView handleVoiceClick) use
  // the false return to drop that stale intent instead of letting a later
  // ASR install completion auto-start a recording nobody asked for.
  const triggerVoice = useCallback((mode = 'dictation', options = {}) => {
    const current = adapterRef.current || {};
    const bridge = current.bridge;
    const voiceInput = current.voiceInput || { status: 'idle' };
    const voiceBusy = !!current.voiceBusy;
    const preserveActiveMode = options.preserveMode && voiceInput.status === 'recording';
    let nextMode = preserveActiveMode ? normalizeMode(voiceInput.mode) : normalizeMode(mode);
    if (!preserveActiveMode && typeof current.resolveMode === 'function') {
      const resolved = normalizeMode(current.resolveMode(nextMode, {
        source: options.source || 'shortcut',
        draft: typeof current.getDraft === 'function' ? current.getDraft() : '',
        voiceInput,
      }));
      // With smart post-processing off, the edit lane has no LLM available (postprocess_disabled
      // always fails), so auto-upgrading dictation→edit would force a doomed edit onto the
      // dictation gesture; stay on dictation and write back the rule-corrected text, matching
      // the web lane's asr_only downgrade. An explicitly requested edit (not an upgrade) is
      // not subject to this gate and keeps its usual failure semantics.
      if (resolved === 'edit' && nextMode === 'dictation' && !voicePostprocessEnabled()) {
        nextMode = 'dictation';
      } else {
        nextMode = resolved;
      }
    } else if (!preserveActiveMode && nextMode === 'dictation' && options.source !== 'button'
      && trimDraft(typeof current.getDraft === 'function' ? current.getDraft() : '')) {
      // Stay on dictation when smart post-processing is off (same reason as above: postprocess_disabled always fails).
      nextMode = voicePostprocessEnabled() ? 'edit' : 'dictation';
    }
    if (!bridge || !bridge.available) return false;
    if (voiceInput.status === 'requesting_permission') {
      bridge.voice.cancelVoiceInput();
      bridge.voice.clearVoiceInput();
      return false;
    }
    if (voiceInput.status === 'recording') {
      const activeMode = normalizeMode(voiceInput.mode);
      bridge.voice.startVoiceInput(
        typeof current.getDraft === 'function' ? current.getDraft() : '',
        (text, draftBeforeStart, context) => handleVoiceResult(
          voiceSessionIdRef.current,
          text,
          draftBeforeStart,
          context,
        ),
        { mode: activeMode },
      );
      return false;
    }
    if (voiceBusy) return false;
    if (typeof current.canStart === 'function' && !current.canStart(nextMode)) return false;
    if (!options.skipBeforeStart && typeof current.onBeforeStart === 'function'
      && current.onBeforeStart(nextMode) === false) {
      return false;
    }

    // A pending edit preview is keyed to the old draft snapshot. Starting a
    // fresh session counts as abandoning it: a leftover preview would let the
    // global Enter handler later replace the draft wholesale with a rewrite
    // based on the stale original, silently discarding anything the new
    // session dictated into the draft.
    if (editPreviewRef.current) setEditPreview(null);

    const sessionId = createVoiceSessionId(current.targetId);
    voiceSessionIdRef.current = sessionId;
    setVoiceSessionId(sessionId);
    bridge.voice.startVoiceInput(
      typeof current.getDraft === 'function' ? current.getDraft() : '',
      (text, draftBeforeStart, context) => handleVoiceResult(sessionId, text, draftBeforeStart, context),
      {
        mode: nextMode,
        beforePermission: typeof current.beforePermission === 'function'
          ? current.beforePermission
          : undefined,
      },
    );
    return true;
  }, [handleVoiceResult]);

  useEffect(() => {
    const current = adapterRef.current || {};
    const status = current.voiceInput && current.voiceInput.status;
    if (!activeStatus(status)) {
      voiceSessionIdRef.current = null;
      setVoiceSessionId(null);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps -- re-arm only when the adapter's voice status identity changes
  }, [adapter.voiceInput && adapter.voiceInput.status]);

  // While an edit preview is pending, Esc/Enter must work regardless of
  // focus, so a capture-phase window listener is installed. Esc carries IME
  // composition guarding (the IME candidate window's Esc only dismisses the
  // candidates, it must not cancel the preview). Enter yields to native
  // activation on interactive elements (buttons, links, selects, text
  // inputs, contenteditable fields) so it never hijacks unrelated inline
  // forms; the main composer is a textarea and keeps the Enter-applies
  // behavior.
  useEffect(() => {
    if (!editPreview) return;
    function handleEditPreviewKeyDown(event) {
      if (!event || event.repeat || event.defaultPrevented || isImeComposing(event)) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        cancelVoiceEditPreview();
        return;
      }
      if (event.key !== 'Enter' || event.shiftKey) return;
      const target = event.target;
      if (target && typeof target.closest === 'function'
        && target.closest('button, a[href], select, input, [contenteditable="true"], [contenteditable=""], [contenteditable="plaintext-only"], [role="button"], [role="menuitem"], [role="option"]')) {
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      void applyVoiceEditPreview({ send: event.ctrlKey || event.metaKey });
    }
    window.addEventListener('keydown', handleEditPreviewKeyDown, true);
    return () => window.removeEventListener('keydown', handleEditPreviewKeyDown, true);
  }, [editPreview, cancelVoiceEditPreview, applyVoiceEditPreview]);

  // When the session/workspace/target identity changes, a leftover voice rewrite preview
  // belongs to the old context: applying its next into the new session's draft, or sending
  // it into the new session via sendTask, would be cross-context data pollution, so cancel
  // it automatically on identity change (explicit user apply/cancel is unaffected). On the
  // first frame (previous is null) only register the identity, without cancelling.
  const voiceContextIdentityRef = useRef(null);
  useEffect(() => {
    const identity = [
      adapter.targetId,
      adapter.ownerKind,
      adapter.workspaceId,
      adapter.sessionId,
    ].map(part => String(part || '')).join('\u0000');
    const previous = voiceContextIdentityRef.current;
    voiceContextIdentityRef.current = identity;
    if (previous === null || previous === identity) return;
    if (editPreviewRef.current) {
      setEditPreview(null);
      closeVoice();
    }
  }, [adapter.targetId, adapter.ownerKind, adapter.workspaceId, adapter.sessionId, closeVoice]);

  useEffect(() => {
    const current = adapterRef.current || {};
    if (!current.targetId) return;
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
