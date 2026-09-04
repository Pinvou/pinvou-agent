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
