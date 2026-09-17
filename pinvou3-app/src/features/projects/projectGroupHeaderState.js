// ProjectGroupHeader's menu gating and inline-edit commit decisions,
// extracted into pure functions for node-side unit tests (review finding 42:
// both regressions — #16's silent convert failure and the web empty menu —
// happened in this previously untested logic).

// The "more" menu renders per actually available actions: web has no projects
// backend, so when callbacks like onConvert are absent the buttons are not
// rendered — no opening a zero-item menu.
function groupHeaderHasMenu(kind, handlers) {
  const { onConvert, onRename, onDelete } = handlers || {};
  return (kind === 'folder' && !!onConvert)
    || (kind === 'project' && (!!onRename || !!onDelete));
}

// Commit an inline edit: returns { action: 'convert' | 'rename', value } or
// null (cancel). "unchanged value = cancel" holds for rename only: convert
// prefills the directory name as the default project name, and pressing Enter
// directly must create with the prefilled value, otherwise the default path
// silently does nothing (review finding 16).
function resolveGroupHeaderEdit({ mode, value, label, busy }) {
  const trimmed = String(value || '').trim();
  if (!trimmed || busy) return null;
  if (mode === 'rename' && trimmed === label) return null;
  if (mode === 'convert') return { action: 'convert', value: trimmed };
  if (mode === 'rename') return { action: 'rename', value: trimmed };
  return null;
}

export { groupHeaderHasMenu, resolveGroupHeaderEdit };
