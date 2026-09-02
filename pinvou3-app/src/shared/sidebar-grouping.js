// Sidebar code style: pure logic that groups code sessions by folder (workspace).
// Like date-utils.js, this is a pure-function module with no UI/i18n dependencies,
// so it can be unit-tested on the node side.

// Shared group key for temporary-workspace sessions: their workspace directory is
// derived from the session id, so bucketing by path would produce many one-off groups;
// merging them into a single bottom group matches the "temporary" semantics.
const TEMPORARY_GROUP_KEY = '__temporary__';

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// Input is code sessions only: [{ workspacePath, workspaceKind, updatedAt, ... }].
// Returns [{ key, rows, latestAt }]: project sessions bucket by workspacePath, temporary
// sessions merge into one group; rows sort by latest activity (updatedAt) descending,
// groups sort by their latest activity descending, and the temporary group always sinks
// to the bottom.
function groupSessionsByFolder(items) {
  const byFolder = new Map();
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    const key = item.workspaceKind === 'project' && item.workspacePath
      ? String(item.workspacePath)
      : TEMPORARY_GROUP_KEY;
    if (!byFolder.has(key)) byFolder.set(key, []);
    byFolder.get(key).push(item);
  });
  const groups = [];
  byFolder.forEach((rows, key) => {
    rows.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    groups.push({ key, rows, latestAt: itemTime(rows[0]) });
  });
  groups.sort((a, b) => {
    if (a.key === TEMPORARY_GROUP_KEY) return 1;
    if (b.key === TEMPORARY_GROUP_KEY) return -1;
    return b.latestAt.localeCompare(a.latestAt);
  });
  return groups;
}

export { TEMPORARY_GROUP_KEY, groupSessionsByFolder };
