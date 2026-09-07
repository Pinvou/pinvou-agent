import { dict } from '../../shared/i18n.js';

export const ACTIVITY_PRIORITY = Object.freeze({
  waiting: 0,
  failed: 1,
  review: 2,
  running: 3,
});

export const ACTIVITY_TTL_MS = Object.freeze({
  running: 3 * 60 * 1000,
  failed: 60 * 60 * 1000,
  waiting: 24 * 60 * 60 * 1000,
  review: 7 * 24 * 60 * 60 * 1000,
});

// 快照说会话"不再工作"后，给事件流(chat:done / turn_end)留出的收尾窗口。
// 多会话并发时主窗口的 busy 状态与桌宠窗口的事件流不同步：快照可能先到
// （主窗口已清 busy）而终态事件还在路上。立即删卡会让窗口收起又展开，
// 多会话下高频反复即"闪现"。宽限期结束后仍未收到任何事件才真正删卡。
export const SNAPSHOT_REMOVAL_GRACE_MS = 2500;

export function createPetState() {
  return {
    sessions: new Map(),
    titles: new Map(),
    // 已在主窗口看到完成结果的会话。保留到下一次真实 turn_start，避免
    // chat:done / session_viewed / activity_snapshot 跨窗口乱序时复活卡片。
    viewedSessions: new Set(),
    lastSnapshotSequence: 0,
    // working:false 快照标记的待删 running 卡: sid -> 标记时间戳。
    // 事件流证明会话仍在活动时清除；超过宽限期仍无事件才删除。
    pendingRemoval: new Map(),
  };
}

function sessionId(payload) {
  const value = payload && (payload.session_id || payload.sessionId);
  return value == null ? '' : String(value).trim();
}

function normalizeConversationText(text) {
  return String(text || '')
    .replaceAll(/\r\n?/g, '\n')
    .replaceAll(/[\t ]+/g, ' ')
    .replaceAll(/\n{3,}/g, '\n\n')
    .trim();
}

function updateActivity(state, sid, status, now, changes = {}) {
  if (!sid) return false;
  // 任何真实事件都证明会话仍在活动(或已由事件流收尾)，取消快照的待删标记，
  // 避免 running 卡被迟到的 working:false 快照误删(多会话并发时高频闪现)。
  if (state.pendingRemoval) state.pendingRemoval.delete(sid);
  const previous = state.sessions.get(sid) || {};
  state.sessions.set(sid, {
    sessionId: sid,
    status,
    updatedAt: now,
    body: previous.body || '',
    latestReply: previous.latestReply || '',
    currentTurnText: previous.currentTurnText || '',
    tool: previous.tool || '',
    ...changes,
  });
  return true;
}

function errorText(payload) {
  const error = payload && payload.error;
  const raw = typeof error === 'string'
    ? error
    : (error && typeof error.message === 'string' ? error.message : '');
  if (!raw) return '';
  // The pet window also receives raw model-service failure bodies
  // (potentially credential-bearing); redact before display. If the helper
  // is missing (pet.html without the shared script), return raw and keep
  // the previous behavior.
  const helper = typeof globalThis !== 'undefined' && globalThis.PinvouModelServiceErrors;
  if (!helper || typeof helper.redactTechnicalDetail !== 'function') return raw;
  return helper.redactTechnicalDetail(raw);
}

// 活动卡正文与标题兜底默认中文；UI 边界(PetWindow)按当前语言从 i18n 的
// uiPet 命名空间注入同形 copy，键名与词条一一对应。
const DEFAULT_ACTIVITY_COPY = Object.freeze({
  activityThinking: dict.zh.uiPet.activityThinking,
  activityProcessing: dict.zh.uiPet.activityProcessing,
  activityUsingTool: dict.zh.uiPet.activityUsingTool,
  activityCallingTool: dict.zh.uiPet.activityCallingTool,
  activityContinuing: dict.zh.uiPet.activityContinuing,
  activityInputNeeded: dict.zh.uiPet.activityInputNeeded,
  activityTaskFailed: dict.zh.uiPet.activityTaskFailed,
  activityTaskDone: dict.zh.uiPet.activityTaskDone,
  activityStartFailed: dict.zh.uiPet.activityStartFailed,
  activityTitleFallback: dict.zh.uiPet.activityTitleFallback,
});

/** Apply a broadcast chat/pet event to the lightweight per-session activity model. */
export function applyEvent(state, name, payload, now = Date.now(), copy = DEFAULT_ACTIVITY_COPY) {
  const sid = sessionId(payload);
  if (!sid) return false;

  switch (name) {
    case 'pet:turn_start': {
      if (state.viewedSessions) state.viewedSessions.delete(sid);
      return updateActivity(state, sid, 'running', now, {
        body: copy.activityThinking,
        latestReply: '',
        currentTurnText: '',
        tool: '',
      });
    }

    case 'chat:delta': {
      const previous = state.sessions.get(sid) || {};
      const chunk = payload && (payload.text || payload.delta || '');
      const currentTurnText = `${previous.currentTurnText || ''}${chunk}`;
      const latestReply = normalizeConversationText(currentTurnText) || previous.latestReply || '';
      return updateActivity(state, sid, 'running', now, {
        currentTurnText,
        latestReply,
        body: latestReply || copy.activityProcessing,
      });
    }

    case 'chat:tool_start': {
      const tool = String((payload && (payload.name || payload.tool_name)) || '').trim();
      return updateActivity(state, sid, 'running', now, {
        tool,
        body: tool ? copy.activityUsingTool(tool) : copy.activityCallingTool,
      });
    }

    case 'chat:tool_end': {
      return updateActivity(state, sid, 'running', now, {
        tool: '',
        body: copy.activityContinuing,
      });
    }

    case 'chat:user_input_required': {
      const prompt = payload && (payload.prompt || payload.message || payload.text);
      const latestReply = normalizeConversationText(prompt);
      return updateActivity(state, sid, 'waiting', now, {
        body: latestReply || copy.activityInputNeeded,
        latestReply,
        currentTurnText: '',
        tool: '',
      });
    }

    case 'chat:done': {
      const previous = state.sessions.get(sid) || {};
      const status = String((payload && payload.status) || '');
      if (/cancel|interrupt/i.test(status)) {
        if (state.pendingRemoval) state.pendingRemoval.delete(sid);
        return state.sessions.delete(sid);
      }
      // 主窗口已经在看这个会话时，session_viewed 可能先于 chat:done
      // 抵达公仔窗口。完成事件此时只负责收尾，不再创建完成卡。
      if (state.viewedSessions && state.viewedSessions.has(sid)) {
        return state.sessions.delete(sid);
      }
      const error = normalizeConversationText(errorText(payload));
      const failed = Boolean(error) || /fail|error/i.test(status);
      const body = error || previous.latestReply || (failed ? copy.activityTaskFailed : copy.activityTaskDone);
      return updateActivity(state, sid, failed ? 'failed' : 'review', now, {
        body,
        latestReply: error || previous.latestReply || '',
        currentTurnText: '',
        tool: '',
      });
    }

    case 'pet:turn_end':
      // tauri-bridge emits this only from invoke(...).catch(), so there will be
      // no chat:done to replace the optimistic Running activity.
      return updateActivity(state, sid, 'failed', now, {
        body: copy.activityStartFailed,
        latestReply: copy.activityStartFailed,
        currentTurnText: '',
        tool: '',
      });

    default:
      return false;
  }
}

export function syncSessionTitles(state, sessions) {
  if (!Array.isArray(sessions)) return false;
  const next = new Map();
  for (const session of sessions) {
    const sid = session && (session.id || session.session_id || session.sessionId);
    if (!sid) continue;
    next.set(String(sid), String(session.title || session.name || '').trim());
  }
  let changed = false;
  for (const sid of state.sessions.keys()) {
    if (next.has(sid)) {
      continue;
    }

    changed = state.sessions.delete(sid) || changed;
    if (state.pendingRemoval) state.pendingRemoval.delete(sid);
  }
  if (state.viewedSessions) {
    for (const sid of state.viewedSessions) {
      if (!next.has(sid)) state.viewedSessions.delete(sid);
    }
  }
  state.titles = next;
  return changed;
}

export function removeSessionActivity(state, sid) {
  const key = String(sid || '').trim();
  if (state.pendingRemoval) state.pendingRemoval.delete(key);
  return state.sessions.delete(key);
}

function pruneExpired(state, now) {
  for (const [sid, activity] of state.sessions) {
    const ttl = ACTIVITY_TTL_MS[activity.status];
    if (!ttl || now - activity.updatedAt > ttl) {
      state.sessions.delete(sid);
      if (state.pendingRemoval) state.pendingRemoval.delete(sid);
    }
  }
  // 快照宽限期收尾：标记后一直没有任何事件(会话真消失)才真正删卡。
  if (state.pendingRemoval && state.pendingRemoval.size) {
    for (const [sid, markedAt] of state.pendingRemoval) {
      if (now - markedAt < SNAPSHOT_REMOVAL_GRACE_MS) {
        continue;
      }

      state.sessions.delete(sid);
      state.pendingRemoval.delete(sid);
    }
  }
}

export function deriveActivities(state, now = Date.now(), copy = DEFAULT_ACTIVITY_COPY) {
  pruneExpired(state, now);
  return [...state.sessions.values()]
    .map((activity) => ({
      ...activity,
      title: state.titles.get(activity.sessionId) || copy.activityTitleFallback,
    }))
    .sort((a, b) => (
      (ACTIVITY_PRIORITY[a.status] - ACTIVITY_PRIORITY[b.status])
      || (b.updatedAt - a.updatedAt)
      || a.sessionId.localeCompare(b.sessionId)
    ));
}

/**
 * tests/pet_state_logic.test.mjs copies this file to a temp directory and
 * dynamically imports this export via a computed URL; knip cannot build an
 * edge for that channel, so the `@public` tag keeps it from being removed as a
 * dead export.
 * @public
 */
export function deriveAnimation(state, now = Date.now()) {
  const first = deriveActivities(state, now)[0];
  return first ? first.status : null;
}

/** Ready/Blocked are read-like notices; active and waiting work stays visible. */
export function markSessionViewed(state, sid, { completed = false } = {}) {
  const key = String(sid || '').trim();
  if (!key) return false;
  const activity = state.sessions.get(key);
  const isCompletedCard = activity?.status === 'review' || activity?.status === 'failed';
  // 打开运行中的会话不等于看过未来的完成结果。只有完成事件确认主窗口此刻
  // 正在显示该会话，或用户实际打开完成卡时，才留下防乱序的已读标记。
  if (!completed && !isCompletedCard) return false;
  if (state.viewedSessions) state.viewedSessions.add(key);
  if (!isCompletedCard) return false;
  if (state.pendingRemoval) state.pendingRemoval.delete(key);
  return state.sessions.delete(key);
}

/**
 * 对齐一次带序号的权威活动快照(调用方需已滤掉定时任务会话)。
 * 旧快照直接丢弃；working:false 的 running 卡先进入宽限期待删标记，
 * 事件流(chat:done / chat:delta 等)到达即取消，宽限期后仍无事件才真正删除。
 */
export function applyActivitySnapshot(state, sessions, sequence, now = Date.now(), copy = DEFAULT_ACTIVITY_COPY) {
  if (!Array.isArray(sessions)) return false;
  const snapshotSequence = Number(sequence);
  if (Number.isFinite(snapshotSequence) && snapshotSequence > 0) {
    if (snapshotSequence <= (state.lastSnapshotSequence || 0)) return false;
    state.lastSnapshotSequence = snapshotSequence;
  }
  // 标题、会话删除与 working 状态必须属于同一张已验序快照；否则旧快照
  // 即使被工作态拒绝，仍可能先一步删掉新会话的活动卡。
  let changed = syncSessionTitles(state, sessions);
  for (const session of sessions) {
    const sid = session && (session.id || session.session_id || session.sessionId);
    if (!sid) continue;
    const key = String(sid);
    const card = state.sessions.get(key);
    const working = !!session.working;
    if (working) {
      if (state.pendingRemoval) state.pendingRemoval.delete(key);
      if (!card && !(state.viewedSessions && state.viewedSessions.has(key))) {
        changed = applyEvent(state, 'pet:turn_start', { session_id: key }, now, copy) || changed;
      }
    } else // 不立即删卡：快照的 working 来自主窗口 busy 状态，可能早于终态事件
      // 到达桌宠窗口。立即删除会让 running 卡消失 → 窗口收起，随后迟到的
      // chat:done/chat:delta 又把卡建回 → 窗口展开。多会话并发时这种
      // 收起/展开反复发生，就是用户看到的"闪现"。改为宽限期标记，事件流
      // 优先收尾；宽限期后仍无事件(会话确实消失)才真正删除。
      if (card && card.status === 'running' && !(state.pendingRemoval && state.pendingRemoval.has(key))) {
        if (!state.pendingRemoval) state.pendingRemoval = new Map();
        state.pendingRemoval.set(key, now);
        changed = true;
      }
  }
  return changed;
}
