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
  // 该文件会被契约测试以 vm 切片加载,裸引用 crypto 在无注入的上下文里会
  // ReferenceError;经 globalThis 读取以获得 Web Crypto 真随机,缺失时才降级。
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

// 任务直发未被接受(守卫拦截或抛错)时抛给桥接层:bridge finishVoiceInput 的
// catch 会把它归一化成失败通知,而不是继续显示「语音任务已发送」的假成功。
// message 留空并覆写 toString,让 normalizeVoiceError 落到现有三语通用文案
// voiceInputFailed;后续可由 bridge 为 send_failed 类别映射专用文案。
function createVoiceTaskSendError() {
  const error = new Error('');
  error.category = 'send_failed';
  error.stage = 'writeback';
  error.toString = () => '';
  return error;
}

// 任务发送收尾:透传 sendTask 的真实结果。契约上 sendTask 应 resolve 布尔值;
// 显式 false 或抛错视为未接受(失败经 onTaskBlocked('send') 通知),历史实现
// 不发回结论(undefined)时保持既有「已受理」行为,避免把成功误报成失败。
// await 窗口内草稿被用户改动时跳过 onTaskAccepted,避免无条件清空吞掉新输入。
// 在途去重与失败呈现由调用方负责。
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

  // bridge 的 cancelVoiceInput/clearVoiceInput 在有进行中会话时语义分叉:cancel 只把
  // 会话收尾成 cancelled(不可见残留态),clear 才会复位 idle。这里统一成「取消 + 复位」,
  // 保证取消/关闭后都不留 cancelled 残留。
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
    // 发送在途时的重复触发(双击、全局 Enter 与 textarea Enter 同时命中)直接忽略,防止双发。
    if (options.send && taskSendInFlightRef.current) return false;
    // 预览以录音开始时的草稿快照(original)为改写基线,而预览挂起期间输入框
    // 仍可手编。草稿已偏离 original 时,确认会拿基于旧原文的改写整段覆盖草稿、
    // 静默丢弃用户手打内容(与新语音会话废弃过期预览同族)——在应用前最后
    // 一道拦下:废弃预览,草稿保持原样。
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

    // 与 dictation 分支一致,以最新草稿为基座函数式合并:录音/转写期间用户补打
    // 的字不能被旧的 draftBeforeStart 快照覆盖。adapter 每次渲染都会刷新
    // adapterRef,这里的 getDraft() 拿到的是写回时刻的最新输入。
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
      // send 的守卫静默 return 时也要让用户看到失败:草稿保留在输入框,并抛给
      // bridge 把「任务已发送」的假成功通知翻成失败通知。
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
      // 智能整理关闭时 edit 车道没有 LLM 可用(postprocess_disabled 必然以
      // 失败告终),dictation→edit 的自动升级只会把"必失败的编辑"强加给
      // 听写手势;保持听写、回写规则纠错文本,与 web 车道 asr_only 降级同
      // 口径。显式请求的 edit(非升级)不受此门约束,失败语义照旧。
      if (resolved === 'edit' && nextMode === 'dictation' && !voicePostprocessEnabled()) {
        nextMode = 'dictation';
      } else {
        nextMode = resolved;
      }
    } else if (!preserveActiveMode && nextMode === 'dictation' && options.source !== 'button'
      && trimDraft(typeof current.getDraft === 'function' ? current.getDraft() : '')) {
      // 智能整理关闭时保持听写(理由同上:postprocess_disabled 必失败)。
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

  // 会话/工作区/目标身份变化时,遗留的语音改写预览属于旧上下文:把它的 next
  // 应用进新会话草稿、甚至经 sendTask 发进新会话都是跨上下文数据污染,身份
  // 变化即自动取消(用户显式的 apply/cancel 不受影响)。首帧(previous 为
  // null)只登记身份,不触发取消。
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
