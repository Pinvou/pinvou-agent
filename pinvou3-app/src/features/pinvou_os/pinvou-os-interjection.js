const TURN_FEEDBACK_BOUNDARIES_MS = Object.freeze([3_000, 15_000, 30_000]);

function normalizedText(value) {
  return String(value == null ? '' : value).trim().replace(/\s+/g, ' ');
}

export function latestOpenTurnStart(turnTimeline) {
  const events = Array.isArray(turnTimeline) ? turnTimeline : [];
  const completedTurnIds = new Set();

  for (const event of events) {
    if (event && event.event === 'assistant_done' && event.turn_id) {
      completedTurnIds.add(String(event.turn_id));
    }
  }

  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (!event || event.event !== 'user_start' || !event.turn_id) continue;
    if (completedTurnIds.has(String(event.turn_id))) continue;
    const timestamp = Number(event.timestamp);
    if (Number.isFinite(timestamp) && timestamp > 0) return { ...event, timestamp };
  }

  return null;
}

export function getTurnFeedback(turnTimeline, nowMs) {
  const openTurn = latestOpenTurnStart(turnTimeline);
  const now = Number(nowMs);
  if (!openTurn || !Number.isFinite(now)) return null;

  const elapsedMs = Math.max(0, now - openTurn.timestamp);
  const thresholdMs = TURN_FEEDBACK_BOUNDARIES_MS
    .filter(boundary => elapsedMs >= boundary)
    .at(-1);
  if (!thresholdMs) return null;

  const phase = thresholdMs === 30_000
    ? 'extended'
    : thresholdMs === 15_000
      ? 'long'
      : 'ready';

  return {
    elapsedMs,
    phase,
    thresholdMs,
    turnId: String(openTurn.turn_id),
  };
}

export function getNextTurnFeedbackDelay(turnTimeline, nowMs) {
  const openTurn = latestOpenTurnStart(turnTimeline);
  const now = Number(nowMs);
  if (!openTurn || !Number.isFinite(now)) return null;

  const elapsedMs = Math.max(0, now - openTurn.timestamp);
  const nextBoundary = TURN_FEEDBACK_BOUNDARIES_MS.find(boundary => boundary > elapsedMs);
  return nextBoundary == null ? null : Math.max(0, nextBoundary - elapsedMs);
}

export function queuedMessageText(item) {
  if (!item || typeof item !== 'object') return '';
  return String(item.displayText == null ? item.text || '' : item.displayText).trim();
}

export function queuedMessagePresentations(queued) {
  return (Array.isArray(queued) ? queued : [])
    .map((item, index) => ({
      id: item && item.id != null ? item.id : `queued-${index}`,
      text: queuedMessageText(item),
    }))
    .filter(item => Boolean(item.text));
}

export function visibleUnqueuedUtterance(lastUtterance, queued) {
  const utterance = String(lastUtterance || '').trim();
  if (!utterance) return '';
  const normalizedUtterance = normalizedText(utterance);
  const duplicated = (Array.isArray(queued) ? queued : []).some(item => (
    normalizedText(queuedMessageText(item)) === normalizedUtterance
  ));
  return duplicated ? '' : utterance;
}
