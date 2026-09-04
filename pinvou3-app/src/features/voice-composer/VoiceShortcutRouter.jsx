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

// 原生快捷键事件只应被目标窗口处理:Rust 侧 payload 携带 window_label(定向
// 发给聚焦窗口),这里校验一致才消费;无 label 的旧事件保持兼容放行。无法确定
// 本窗口 label 时放行(此时 Rust 已按聚焦窗口定向,错投风险由发送侧承担)。
function isVoiceShortcutEventForThisWindow(payload) {
  const eventLabel = payload && typeof payload.window_label === 'string' ? payload.window_label : '';
  if (!eventLabel) return true;
  const ownLabel = (tryGetCurrentTauriWindow() || {}).label || '';
  return !ownLabel || ownLabel === eventLabel;
}

function VoiceShortcutRouter({ enabled = true }) {
  const pendingRef = useRef(null);
  // 最近一次非纯 Alt keydown 的时刻:Windows 组合键透传会在组合键 keydown
  // 的同一事件批内注入合成 Alt down,状态机据此把该 pending 标记为注入
  // 序列(见 voice-shortcut-state 的 INJECTED_COMBO_WINDOW_MS)。
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
      // 引导弹窗打开时,Esc 让路给弹窗自身的关闭处理(不 preventDefault、不取消语音启动)。
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
      // 原生按「录音窗」路由到本窗,但本窗已无活跃录音:多数是 WebView 重载/
      // 恢复后 JS 会话重建而原生登记未清(原生只在窗口销毁时清)。清掉陈旧登记
      // 并丢弃本次手势,下一按恢复按聚焦窗路由,绝不后台幽灵开麦。
      if (payload && payload.route === 'recording' && !recording) {
        const bridge = tryGetTauriBridge();
        if (bridge && bridge.available
          && typeof bridge.voice.syncVoiceShortcutRecording === 'function') {
          bridge.voice.syncVoiceShortcutRecording(null);
        }
        return;
      }
      // 原生事件只在 Rust 侧开关开启时才会发出(hook 入口 !shortcut_enabled()
      // 短路),权威门控已在原生层完成,这里不叠加 localStorage 镜像检查——
      // WebView 存储被清后镜像缺省 false,叠加检查会让权威开启的快捷键被原生
      // 吞键后又被前端丢弃,静默失灵。镜像只服务上方窗口内键手势通道。
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
