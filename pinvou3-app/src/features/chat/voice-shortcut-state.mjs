import { isImeComposing } from '../../shared/ime-guard.mjs';

function normalizeVoiceShortcutMode(mode) {
  if (mode === 'edit' || mode === 'voice_edit' || mode === 'draft_edit') return 'edit';
  return mode === 'task' ? 'task' : 'dictation';
}

function isPlainAltKey(event) {
  return event
    && event.key === 'Alt'
    // 右 Alt 与 Rust 钩子同口径(VK_RMENU 归 Other):留给 AltGr/输入法,
    // 不触发语音快捷键。location 2 = DOM_KEY_LOCATION_RIGHT;个别环境
    // location 缺失时以 code 兜底。
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
  // IME 合成期间的按键(含 WKWebView 延迟派发的 keyCode 229 回车/Esc)只属于
  // 输入法候选窗口,不得触发语音快捷键;按住不放产生的 repeat 同理过滤。
  return !event || event.repeat || Boolean(event.defaultPrevented) || isImeComposing(event);
}

function isActiveVoiceShortcutStatus(status) {
  return status === 'requesting_permission'
    || status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing';
}

// Windows 组合键透传在组合键 keydown 的同一事件批内注入合成 Alt down
//(SendInput 同步补发),留一点调度余量;人类「按字母后 50ms 内完成一次
// Alt 空按」在物理上不可达,故窗口只需覆盖调度抖动。
const INJECTED_COMBO_WINDOW_MS = 50;

function voiceShortcutActionForKeyDown(event, current) {
  const state = current || {};
  const status = state.status || 'idle';

  if (isAltSpaceKey(event)) {
    return { type: 'clear_pending' };
  }

  if (isPlainAltKey(event)) {
    // Alt down 紧跟着一个非 Alt keydown(同一注入批)时,它是组合键透传
    // 补发的合成 Alt down:整段手势按注入对待,真实 Alt up 到来时直接清除
    // 而不是触发(防「先松 Alt 后松组合键」顺序的 ghost 听写)。
    const injected = typeof state.lastNonAltKeyDownAt === 'number'
      && typeof state.now === 'number'
      && state.now - state.lastNonAltKeyDownAt >= 0
      && state.now - state.lastNonAltKeyDownAt <= INJECTED_COMBO_WINDOW_MS;
    return injected ? { type: 'pending_alt', injected: true } : { type: 'pending_alt' };
  }

  // 挂起的 Alt 手势期间,任何其他键(含 Esc)都是组合键成员:清 pending、
  // 不触发也不取消。否则 Alt+Esc(系统窗口循环切换)透传批内的 Esc 会在
  // 录音中把会话一并取消,与 Alt+Tab(Other 键只清 pending)口径不一。
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
    // 反向释放序 [combo down, Alt down (injected), real Alt up, combo up]:
    // Alt up 先到,但该 pending 已被标记为注入序列,清除而不触发。
    return { type: 'clear_pending' };
  }
  const status = state.status || 'idle';
  // 权限申请挂起期间松开 Alt 与原生路径一致:取消这次挂起的语音启动,而不是再触发一次。
  if (status === 'requesting_permission') return { type: 'cancel' };
  const mode = normalizeVoiceShortcutMode(state.mode);
  if (status === 'recording') return { type: 'trigger', mode };
  return { type: 'trigger', mode: 'dictation' };
}

// 快捷键引导弹窗打开期间,Esc 交给弹窗自身处理(等同于点 X 关闭),
// 路由层不应同时取消挂起的语音启动。弹窗挂载/卸载时由弹窗组件维护该标记。
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
