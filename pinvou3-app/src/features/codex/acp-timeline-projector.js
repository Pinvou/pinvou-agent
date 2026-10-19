import { projectAcpTimeline } from './acp-state.js';

// Only immutable, strictly ordered, single-session logs qualify for reuse.
// Snapshots, duplicates and irregular inputs retain the full projector's semantics.
function orderedTail(events, start, sessionId = null, lastSeq = 0) {
  for (let index = start; index < events.length; index++) {
    const envelope = events[index];
    if (!envelope || typeof envelope.sessionId !== 'string' || !envelope.sessionId
      || !Number.isSafeInteger(envelope.seq) || envelope.seq <= lastSeq
      || (envelope.turnId && typeof envelope.turnId !== 'string')) return null;
    // The authoritative reducer's plain-object index treats inherited names
    // as global events. Preserve that legacy behavior through full replay.
    if (envelope.turnId && Object.hasOwn(Object.prototype, envelope.turnId)) return null;
    if (sessionId !== null && sessionId !== envelope.sessionId) return null;
    sessionId = envelope.sessionId;
    lastSeq = envelope.seq;
  }
  return { sessionId, lastSeq };
}

function groupTurns(events) {
  const groups = new Map();
  for (const envelope of events) {
    if (!envelope.turnId) continue;
    if (!groups.has(envelope.turnId)) groups.set(envelope.turnId, []);
    groups.get(envelope.turnId).push(envelope);
  }
  return groups;
}

/**
 * One latest projection per owner. Inputs and returned timelines are read-only.
 * Reuse requires the previous event references to remain an exact prefix; no
 * completed turn is assumed immutable when a later event actually targets it.
 */
export function createAcpTimelineProjector() {
  let cache = null;
  return function project(input, options = {}) {
    const events = input || [];
    const language = options && options.language;
    const errorHelper = globalThis.PinvouModelServiceErrors;
    const redactor = errorHelper && errorHelper.redactTechnicalDetail;
    const extendsCache = cache && Array.isArray(events)
      && language === cache.language && errorHelper === cache.errorHelper && redactor === cache.redactor
      && events.length >= cache.events.length
      && cache.events.every((envelope, index) => events[index] === envelope);
    if (extendsCache && events.length === cache.events.length) return cache.projection;
    const ordered = extendsCache && (cache.ordered || orderedTail(cache.events, 0));
    const tail = ordered
      ? orderedTail(events, cache.events.length, ordered.sessionId, ordered.lastSeq)
      : null;

    if (!tail) {
      const projection = projectAcpTimeline(events, options);
      // Avoid a second history traversal on initial hydration. Eligibility and
      // turn grouping are needed only if a later append can reuse this result.
      cache = Array.isArray(events) ? {
        events, projection, groups: null, ordered: null, language, errorHelper, redactor,
      } : null;
      return projection;
    }

    const additions = groupTurns(events.slice(cache.events.length));
    const previousGroups = cache.groups || groupTurns(cache.events);
    const groups = new Map(previousGroups);
    const replacements = new Map();
    for (const [id, added] of additions) {
      const history = [...(groups.get(id) || []), ...added];
      replacements.set(id, projectAcpTimeline(history, options).turns[0]);
      groups.set(id, history);
    }
    const turns = cache.projection.turns.map(turn => replacements.get(turn.id) || turn);
    for (const [id, turn] of replacements) {
      if (!previousGroups.has(id)) turns.push(turn);
    }
    const globals = events.slice(cache.events.length).filter(envelope => !envelope.turnId);
    const projection = {
      turns,
      global: globals.length ? [...cache.projection.global, ...globals] : cache.projection.global,
    };
    // Publish the new cache only after every affected turn projected successfully.
    cache = { events, projection, groups, ordered: tail, language, errorHelper, redactor };
    return projection;
  };
}
