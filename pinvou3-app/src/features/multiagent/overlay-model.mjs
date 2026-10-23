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

/**
 * Terminal matches the authority chain exactly: the foundation considers a
 * blocked worker Completed (`is_terminal`), and transcripts.rs projects it as
 * done=true. Blocked is a display-layer attribute (`statusPresentation`), not
 * a liveness fact — keeping it out of `isTerminal` would pin the entry into
 * the active bucket forever and lock the ledger poll at the active cadence.
 */
export function isTerminal(entry) {
  return !!entry && !!entry.done;
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
 * Terminal endings that are not dispatch failures even though the ledger folds
 * every non-completed terminal into failed=true: a swarm-off cancellation or a
 * session interruption keeps the neutral presentation (own copy + stopped dot,
 * or the detail badge's amber bucket). Single source for the token set shared
 * by the overlay status mapping, the transcript panel dot, and the transcript
 * projection's status label.
 */
export function isNeutralEndingStatus(status) {
  const token = String(status || '').toLowerCase();
  return token === 'cancelled' || token === 'interrupted';
}

/**
 * Ending tokens of the worker-ledger vocabulary (the foundation's
 * agent_worker_status_name): completed/failed plus the two neutral endings.
 * A progress event carrying one is the engine's end-of-run announcement,
 * emitted just before the ordered completion event — not a live state.
 * Overriding the label with it would render raw untranslated English
 * ("completed"/"failed") on a not-yet-done row, so endings resolve through
 * done/blocked/failed plus the completion event and the ledger instead.
 */
export function isWorkerEndingToken(status) {
  const token = String(status || '').toLowerCase();
  return token === 'completed' || token === 'failed' || isNeutralEndingStatus(token);
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
 * - Spawn-time identity (role/agentType) only arrives from the ledger: a
 *   non-ledger detail's nulls restore the previous values instead of wiping
 *   them, while ledger rows stay authoritative and still set or clear the
 *   fields.
 * - A truthy detail.workerStatus (the ledger's token vocabulary) outranks
 *   the free-text phrase in detail.status — except ending tokens
 *   (isWorkerEndingToken): those are the engine's end-of-run announcement
 *   ahead of the ordered completion event, not a live state, so endings
 *   resolve through done plus the completion event/ledger instead.
 * @param {object|null} previous the entry's current cache (null = first observation)
 * @param {object} detail the new reading (with done/blocked/source, etc.)
 * @param {string} sessionIdIn session the entry belongs to
 * @param {number} now clock (injected by tests)
 */
export function mergeOverlayEntry(previous, detail, sessionIdIn, now) {
  if (previous && previous.done && !detail.done && detail.source !== 'ledger') return null;
  const next = { ...previous, ...detail, sessionId: sessionIdIn };
  // Spawn-time identity (role/agent type) only ever arrives from the ledger:
  // real-time progress/completion events carry neither field, and letting
  // their nulls through the spread would wipe the ledger's values, flipping
  // the row label to the generic role on every event and back on every poll.
  // The ledger stays authoritative — its rows still set or clear the fields.
  if (detail.source !== 'ledger') {
    if (detail.role == null && previous && previous.role != null) next.role = previous.role;
    if (detail.agentType == null && previous && previous.agentType != null) next.agentType = previous.agentType;
  }
  // A structured worker-status token (same vocabulary as the ledger's status
  // column) outranks the free-text progress phrase: with only the phrase, the
  // ledger token and the event phrase overwrote each other and the status
  // label flickered between "queued" and "working" for a steadily working
  // agent. Ending tokens are excluded: the engine ends every run with a
  // progress event that carries the terminal token milliseconds ahead of the
  // ordered completion event, and stamping it onto a not-yet-done row would
  // flash raw English ("completed") with a live dot — the phrase (a
  // multi-word line falls back to the working copy) holds the line until the
  // completion event or the ledger resolves the ending.
  if (detail.workerStatus && !isWorkerEndingToken(detail.workerStatus)) next.status = detail.workerStatus;
  // A real-time completion carries no status token (the bridge sends
  // status: null because the engine's completion event carries no status
  // token) and reports blocked: false, so the spread alone would whiten three neutral
  // endings until the next ledger read corrects them: a cancelled/interrupted
  // token into a green "completed", and a blocked entry into a plain
  // completion (losing the "waiting on the user" surface). Keep the ledger's
  // distinguishing fields instead.
  const previousToken = String(previous && previous.status || '').toLowerCase();
  const previousBlocked = !!(previous && previous.done && previous.blocked);
  if (previous && previous.done && detail.status == null
    && (isNeutralEndingStatus(previousToken) || previousBlocked)) {
    next.status = previous.status;
    next.failed = previous.failed;
    if (previousBlocked) next.blocked = true;
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
 * copy (queued/pending/starting, plus the parked token waiting_for_user →
 * pending; running, the executing-tools token running_tool and the
 * between-steps token model_wait → working). Model wait is a live activity —
 * the foundation partitions ModelWait/RunningTool into OwnerState::Running
 * versus Queued/WaitingForUser into OwnerState::Waiting, and its own sidebar
 * lists "model wait" among the live activities — so bucketing it as queued
 * made a working agent read as "queued" and flip against the working label.
 * A multi-word
 * non-terminal phrase is a real-time progress line (e.g. "🔧 Edit (step 3)")
 * and falls back to working; a single unknown token is shown verbatim.
 */
export function statusPresentation(entry, copy) {
  const statusToken = String(entry && entry.status || '').toLowerCase();
  if (entry && entry.done && entry.failed) {
    if (isNeutralEndingStatus(statusToken)) return { text: statusToken === 'cancelled' ? copy.agentCard.cancelled : copy.agentCard.interrupted, dot: 'stopped' };
    return { text: copy.agentCard.failed, dot: 'failed' };
  }
  if (entry && entry.done && entry.blocked) return { text: copy.blockedTag, dot: 'blocked' };
  if (entry && entry.done) return { text: copy.agentCard.completed, dot: 'done' };
  if (['queued', 'pending', 'starting', 'waiting_for_user'].includes(statusToken)) return { text: copy.pendingTag, dot: 'running' };
  if (['running', 'running_tool', 'model_wait'].includes(statusToken)) return { text: copy.agentCard.working, dot: 'running' };
  return { text: entry && entry.status && !/\s/.test(String(entry.status)) ? entry.status : copy.agentCard.working, dot: 'running' };
}

/**
 * Overlay visibility: some entry is running, or a just-finished terminal
 * entry is still inside its success-state window. A done-but-blocked entry
 * stays in the active list on purpose — it is the "waiting on the user"
 * surface — but it must not drive the poll cadence: `hasActive` in the
 * component is computed with `isTerminal`, which blocked entries satisfy.
 * @param {Array} entries the current session's entries
 * @param {number} now clock (injected by tests)
 */
export function overlayVisibleEntries(entries, now) {
  const active = [];
  const recent = [];
  for (const entry of entries || []) {
    if (!entry) continue;
    if (!entry.done || entry.blocked) active.push(entry);
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
