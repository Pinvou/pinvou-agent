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

export function createPetState() {
  return {
    sessions: new Map(),
    titles: new Map(),
    // 已在主窗口看到完成结果的会话。保留到下一次真实 turn_start，避免
    // chat:done / session_viewed / activity_snapshot 跨窗口乱序时复活卡片。
    viewedSessions: new Set(),
    lastSnapshotSequence: 0,
  };
}

function sessionId(payload) {
  const value = payload && (payload.session_id || payload.sessionId);
  return value == null ? '' : String(value).trim();
}

function normalizeConversationText(text) {
  return String(text || '')
    .replace(/\r\n?/g, '\n')
    .replace(/[\t ]+/g, ' ')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

function updateActivity(state, sid, status, now, changes = {}) {
  if (!sid) return false;
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
  if (typeof error === 'string') return error;
  if (error && typeof error.message === 'string') return error.message;
  return '';
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
    if (!next.has(sid)) changed = state.sessions.delete(sid) || changed;
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
  return state.sessions.delete(String(sid || '').trim());
}

function pruneExpired(state, now) {
  for (const [sid, activity] of state.sessions) {
    const ttl = ACTIVITY_TTL_MS[activity.status];
    if (!ttl || now - activity.updatedAt > ttl) state.sessions.delete(sid);
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
  return state.sessions.delete(key);
}

/**
 * 对齐一次带序号的权威活动快照(调用方需已滤掉定时任务会话)。
 * 旧快照直接丢弃；working:false 立即清除 running，不依赖宽限期或第二次快照。
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
      if (!card && !(state.viewedSessions && state.viewedSessions.has(key))) {
        changed = applyEvent(state, 'pet:turn_start', { session_id: key }, now, copy) || changed;
      }
    } else if (card && card.status === 'running') {
      changed = state.sessions.delete(key) || changed;
    }
  }
  return changed;
}
