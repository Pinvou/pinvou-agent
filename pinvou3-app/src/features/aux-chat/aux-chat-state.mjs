import { projectDeepSeekConversation } from '../conversation/deepseek-conversation.js';

/**
 * 辅助对话面板的纯逻辑层：把 bridge.auxChat.snapshot(auxId) 的同步快照
 * 归一化、判定 busy/空态，并投影成 ConversationTimeline 需要的 turns。
 * 投影直接复用主会话的 projectDeepSeekConversation——它是纯函数（所有
 * 输入经参数传入，不读取 active 会话全局态），辅助会话的 chatItems 又由
 * 同一事件管线写入，结构一致；仅 thinking/tokens/timelineEvents 是后台
 * 快照拿不到的主会话专属增强，辅助对话不投影这些。
 */

const EMPTY_AUX_SNAPSHOT = Object.freeze({ chatItems: [], busy: false, queued: [] });

export function normalizeAuxSnapshot(raw) {
  if (!raw || typeof raw !== 'object') return EMPTY_AUX_SNAPSHOT;
  return {
    chatItems: Array.isArray(raw.chatItems) ? raw.chatItems : [],
    busy: !!raw.busy,
    queued: Array.isArray(raw.queued) ? raw.queued : [],
  };
}

// chat 域 notify 也会携主会话的流式 tick 进来；辅助会话的快照没变时跳过
// setSnapshot，避免每个 token 都触发面板重渲染与 turns 重投影。逐条浅比较
// 条目字段（不能只看引用：流式 delta 是原地修改条目的 text/streaming 字段）。
function auxItemsEqual(left, right) {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  const leftKeys = Object.keys(left);
  if (leftKeys.length !== Object.keys(right).length) return false;
  return leftKeys.every((key) => Object.is(left[key], right[key]));
}

export function auxSnapshotsEqual(prev, next) {
  if (prev === next) return true;
  const a = normalizeAuxSnapshot(prev);
  const b = normalizeAuxSnapshot(next);
  if (a.busy !== b.busy) return false;
  if (a.chatItems.length !== b.chatItems.length || a.queued.length !== b.queued.length) return false;
  for (let i = 0; i < a.chatItems.length; i += 1) {
    if (!auxItemsEqual(a.chatItems[i], b.chatItems[i])) return false;
  }
  for (let i = 0; i < a.queued.length; i += 1) {
    if (!auxItemsEqual(a.queued[i], b.queued[i])) return false;
  }
  return true;
}

// 与 bridge send 的拒绝口径一致：busy 或仍有排队消息时都视为不可发送。
export function auxChatBusy(snapshot) {
  const snap = normalizeAuxSnapshot(snapshot);
  return snap.busy || snap.queued.length > 0;
}

// 落地说明条只在还没有任何问答内容时展示（system/工具类条目不算内容）。
export function auxChatHasContent(snapshot) {
  return normalizeAuxSnapshot(snapshot).chatItems.some((item) => (
    item && (item.type === 'user' || item.type === 'assistant')
  ));
}

export function projectAuxChatTurns(snapshot, auxId) {
  const snap = normalizeAuxSnapshot(snapshot);
  return projectDeepSeekConversation({
    chatItems: snap.chatItems,
    busy: snap.busy,
    sessionId: auxId,
  }).turns;
}
