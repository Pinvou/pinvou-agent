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
 * A ledger summary with no status token that is not done: transcripts.rs
 * projects orphan transcripts (file exists, no worker-ledger record) this
 * way. The foundation prunes worker records past MAX_AGENT_WORKER_RECORDS
 * (256) while keeping their transcript files, so any swarm session that
 * spawns more than 256 children accumulates these rows. They are historical
 * leftovers, not live agents: merging them would pin eternal "working"
 * ghosts into the overlay, inflate the count badge, and lock the ledger
 * poll at the active cadence forever. Callers must skip them.
 */
export function isUnknownLedgerRow(summary) {
  return !!summary && !summary.done && summary.status == null;
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
  // A real-time completion carries no status token (the bridge sends
  // status: null because the engine event cannot distinguish endings).
  // Keep the ledger's distinguishing terminal token — and its failed flag —
  // instead of letting the spread whiten a cancelled/interrupted ending into
  // a green "completed" until the next ledger read corrects it.
  const previousToken = String(previous && previous.status || '').toLowerCase();
  if (previous && previous.done && detail.status == null
    && (previousToken === 'cancelled' || previousToken === 'interrupted')) {
    next.status = previous.status;
    next.failed = previous.failed;
  }
  // A first observation (no previous) is not a flip: grant nothing whether the
  // reading is running or terminal.
  const wasLiveNonTerminal = !!previous && !isTerminal(previous);
  if (isTerminal(detail) && wasLiveNonTerminal) next.completedAt = now;
  if (!detail.done) delete next.completedAt;
  return next;
}

/**
 * Status presentation: terminal first. Terminal failures keep a distinguishing
 * status token honest: the ledger folds every non-completed ending into
 * failed=true, but a swarm-off cancellation or a session interruption is not a
 * dispatch failure, so those two tokens map to their own copy and a neutral
 * dot. Non-terminal entries map the ledger's English status tokens to i18n
 * copy (queued/pending/starting, plus the waiting tokens waiting_for_user/
 * model_wait → pending; running and the executing-tools token running_tool →
 * working). A multi-word non-terminal phrase is a real-time progress line
 * (e.g. "🔧 Edit (step 3)") and falls back to working; a single unknown token
 * is shown verbatim.
 */
export function statusPresentation(entry, copy) {
  const statusToken = String(entry && entry.status || '').toLowerCase();
  if (entry && entry.done && entry.failed) {
    if (statusToken === 'cancelled') return { text: copy.agentCard.cancelled, dot: 'stopped' };
    if (statusToken === 'interrupted') return { text: copy.agentCard.interrupted, dot: 'stopped' };
    return { text: copy.agentCard.failed, dot: 'failed' };
  }
  if (entry && entry.done && entry.blocked) return { text: copy.blockedTag, dot: 'blocked' };
  if (entry && entry.done) return { text: copy.agentCard.completed, dot: 'done' };
  if (['queued', 'pending', 'starting', 'waiting_for_user', 'model_wait'].includes(statusToken)) return { text: copy.pendingTag, dot: 'running' };
  if (statusToken === 'running' || statusToken === 'running_tool') return { text: copy.agentCard.working, dot: 'running' };
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
  if (terminal.length < overflow) return null;
  const evict = new Set(terminal.slice(0, overflow));
  const next = {};
  for (const key of keys) {
    if (!evict.has(key)) next[key] = entries[key];
  }
  return next;
}
