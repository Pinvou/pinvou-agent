/**
 * Pure display/filter model for the swarm running overlay (no React, no bridge
 * dependency, directly coverable by node:test). RunningAgentsOverlay is the
 * only production consumer.
 */

/** How long a terminal entry lingers in the list (success-state window). */
export const RECENT_TERMINAL_MS = 3500;

/**
 * Entry cap for the entries cache: beyond it, evict the oldest terminal
 * entries first. Swarm-mode per-tree admission reaches the foundation cap
 * (1024), so a long session would accumulate without bound otherwise.
 */
export const MAX_OVERLAY_ENTRIES = 200;

export function entryKey(sessionId, agentId) {
  return `${sessionId || ''}\u0000${agentId || ''}`;
}

export function isTerminal(entry) {
  return !!entry && !!entry.done && !entry.blocked;
}

/**
 * Single-entry merge (the pure logic behind the component's mergeEntry):
 * - Terminal ratchet: the persisted terminal state is authoritative; a late
 *   non-terminal real-time event must not flip an entry back to running (the
 *   persisted re-awaken path flips back through the ledger snapshot itself).
 *   Rejections return null so the caller can skip the state update.
 * - completedAt is granted only for a real non-terminal → terminal flip
 *   observed within this session. Cold-start snapshots (historical terminal
 *   entries read on mount or on the first poll after a session switch) get
 *   none: they were never shown as running in this session, and granting
 *   would pull the whole historical batch into the success-state window,
 *   flashing a fake "running 0" pill on open.
 * @param {object|null} previous the entry's current cache (null = first observation)
 * @param {object} detail the new reading (with done/blocked/source, etc.)
 * @param {string} sessionIdIn session the entry belongs to
 * @param {number} now clock (injected by tests)
 */
export function mergeOverlayEntry(previous, detail, sessionIdIn, now) {
  if (previous && previous.done && !detail.done && detail.source !== 'ledger') return null;
  const next = { ...previous, ...detail, sessionId: sessionIdIn };
  // A first observation (no previous) is not a flip: grant nothing whether the
  // reading is running or terminal.
  const wasLiveNonTerminal = !!previous && !isTerminal(previous);
  if (isTerminal(detail) && wasLiveNonTerminal) next.completedAt = now;
  if (!detail.done) delete next.completedAt;
  return next;
}

/**
 * Status presentation: terminal first; non-terminal entries map the ledger's
 * English status tokens to i18n copy (queued/pending/starting → pending,
 * running → working, same set as tool-renderers' LEDGER_STATUS_TOKENS);
 * anything else is treated as a real-time progress phrase and shown verbatim.
 */
export function statusPresentation(entry, copy) {
  const statusToken = String(entry && entry.status || '').toLowerCase();
  if (entry && entry.done && entry.failed) return { text: copy.agentCard.failed, dot: 'failed' };
  if (entry && entry.done && entry.blocked) return { text: copy.blockedTag, dot: 'blocked' };
  if (entry && entry.done) return { text: copy.agentCard.completed, dot: 'done' };
  if (['queued', 'pending', 'starting'].includes(statusToken)) return { text: copy.pendingTag, dot: 'running' };
  if (statusToken === 'running') return { text: copy.agentCard.working, dot: 'running' };
  return { text: entry && entry.status && !/\s/.test(String(entry.status)) ? entry.status : copy.agentCard.working, dot: 'running' };
}

/**
 * Overlay visibility: some entry is non-terminal, or a just-finished terminal
 * entry is still inside its success-state window.
 * @param {Array} entries the current session's entries
 * @param {number} now clock (injected by tests)
 */
export function overlayVisibleEntries(entries, now) {
  const active = [];
  const recent = [];
  for (const entry of entries || []) {
    if (!isTerminal(entry)) active.push(entry);
    else if (entry.completedAt && now - entry.completedAt < RECENT_TERMINAL_MS) recent.push(entry);
  }
  return { active, recent };
}

/**
 * Entries cache eviction: past the cap, evict the oldest terminal entries by
 * completedAt ascending (missing completedAt counts as oldest). Returns null
 * when no change is needed (under the cap, or not enough terminal entries to
 * evict — non-terminal entries are naturally bounded by the live agent count).
 */
export function pruneOverlayEntries(entries, max = MAX_OVERLAY_ENTRIES) {
  const keys = Object.keys(entries);
  if (keys.length <= max) return null;
  const overflow = keys.length - max;
  const terminal = keys
    .filter(key => isTerminal(entries[key]))
    .sort((left, right) => (entries[left].completedAt || 0) - (entries[right].completedAt || 0));
  if (terminal.length <= overflow) return null;
  const evict = new Set(terminal.slice(0, overflow));
  const next = {};
  for (const key of keys) {
    if (!evict.has(key)) next[key] = entries[key];
  }
  return next;
}
