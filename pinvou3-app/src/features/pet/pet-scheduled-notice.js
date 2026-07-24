export const SCHEDULED_NOTICE_ACK_KEY = 'pinvou3-pet-scheduled-notice-ack-v1';

function sessionId(payload) {
  return String((payload && (payload.session_id || payload.sessionId || payload.id)) || '').trim();
}

export function isScheduledSessionPayload(payload) {
  return sessionId(payload).startsWith('sched-');
}

function normalizedNotice(task, run) {
  const status = String((run && run.status) || '').toLowerCase();
  const runId = String((run && (run.id || run.runId || run.run_id)) || '').trim();
  const session = sessionId(run);
  const taskName = String((task && task.name) || '').trim();
  const endedAt = String((run && (run.endedAt || run.ended_at)) || '').trim();
  const endedAtMs = Date.parse(endedAt);
  if (status !== 'completed' || !run.unread || !runId || !session || !taskName || !Number.isFinite(endedAtMs)) {
    return null;
  }
  return {
    automationId: String(task.id),
    runId,
    sessionId: session,
    taskName,
    endedAt,
    endedAtMs,
  };
}

export function selectLatestScheduledNotice(tasks, runsByTask, acknowledgedAt = 0) {
  let latest = null;
  for (const task of Array.isArray(tasks) ? tasks : []) {
    const runs = runsByTask && runsByTask[task.id];
    for (const run of Array.isArray(runs) ? runs : []) {
      const notice = normalizedNotice(task, run);
      if (!notice || notice.endedAtMs <= acknowledgedAt) continue;
      if (!latest || notice.endedAtMs > latest.endedAtMs) latest = notice;
    }
  }
  return latest;
}

export function formatScheduledNoticeBody(notice, locale = 'zh-CN', completedLabel = '已完成') {
  const time = new Intl.DateTimeFormat(locale, {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(new Date(notice.endedAtMs));
  return `${time}「${notice.taskName}」${completedLabel}`;
}

export function readScheduledNoticeAcknowledgedAt(storage = window.localStorage) {
  try {
    const value = Number(storage.getItem(SCHEDULED_NOTICE_ACK_KEY));
    return Number.isFinite(value) && value > 0 ? value : 0;
  } catch (_) {
    return 0;
  }
}

export function acknowledgeScheduledNotice(notice, storage = window.localStorage) {
  const current = readScheduledNoticeAcknowledgedAt(storage);
  const next = Math.max(current, Number(notice && notice.endedAtMs) || 0);
  try {
    storage.setItem(SCHEDULED_NOTICE_ACK_KEY, String(next));
  } catch (_) {
    // A denied storage write should not block dismissing the in-memory notice.
  }
  return next;
}
