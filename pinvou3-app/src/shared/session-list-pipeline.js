// Shared filter / sort / date-group pipeline for session lists. Two views
// ship the exact same semantics — the sidebar task list (main.jsx) and the
// conversation-management page (features/search/SearchView.jsx) — so the
// comparators and the 'unknown'-sink date grouping live here once.
//
// Sessions are plain objects carrying at least `pinned`, `pinnedAt`,
// `updatedAt` and (for the task-kind filter) `taskKind` ('codex' |
// 'scheduled' | 'regular'). Timestamps are compared as strings, matching the
// ISO-ish shapes the history slices already produce.

/**
 * Filter a session list by sidebar-tab id. Unknown / 'all' tabs keep every
 * session. Always returns a new array (safe to sort in place).
 */
export function filterSessionsByTab(sessions, tabId) {
  if (tabId === 'pinned') return sessions.filter((chat) => !!chat.pinned);
  if (tabId === 'code') return sessions.filter((chat) => chat.taskKind === 'codex');
  if (tabId === 'scheduled') return sessions.filter((chat) => chat.taskKind === 'scheduled');
  return [...sessions];
}

/**
 * Pinned sessions first (most recently pinned first), the rest by update
 * time. Shared by 'pinned_first' sort in both consumers.
 */
export function compareSessionsPinnedFirst(a, b) {
  if (!!a.pinned !== !!b.pinned) return a.pinned ? -1 : 1;
  const aTime = a.pinned ? (a.pinnedAt || a.updatedAt) : (a.updatedAt || a.pinnedAt);
  const bTime = b.pinned ? (b.pinnedAt || b.updatedAt) : (b.updatedAt || b.pinnedAt);
  return String(bTime || '').localeCompare(String(aTime || ''));
}

/** Plain most-recently-updated order ('recent' sort / archived panels). */
export function compareSessionsByRecentUpdate(a, b) {
  return String(b.updatedAt || b.pinnedAt || '').localeCompare(String(a.updatedAt || a.pinnedAt || ''));
}

/** Comparator for the shared sort modes: 'pinned_first' | 'recent'. */
export function sessionListComparator(sortMode) {
  return sortMode === 'pinned_first' ? compareSessionsPinnedFirst : compareSessionsByRecentUpdate;
}

/**
 * Group sessions by local calendar day. `dateKeyOf` maps a session to its
 * group key (callers pass localDateKey over their preferred timestamp, e.g.
 * `updatedAt || pinnedAt` vs plain `updatedAt`); keys must be same-width
 * strings so lexicographic order equals chronological order. Groups keep the
 * input row order (the list is pre-sorted), are ordered newest day first,
 * and sessions without a usable timestamp sink into the 'unknown' group at
 * the bottom.
 */
export function groupSessionsByLocalDate(sessions, dateKeyOf) {
  const groups = [];
  const byDate = new Map();
  sessions.forEach((chat) => {
    const key = dateKeyOf(chat);
    if (!byDate.has(key)) byDate.set(key, []);
    byDate.get(key).push(chat);
  });
  byDate.forEach((rows, key) => { groups.push({ key, rows }); });
  groups.sort((a, b) => {
    if (a.key === 'unknown') return 1;
    if (b.key === 'unknown') return -1;
    return b.key.localeCompare(a.key);
  });
  return groups;
}
