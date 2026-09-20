// ProjectGroupHeader's menu gating and inline-edit submit decisions,
// extracted as pure functions so they can be unit-tested on the node side
// (review finding 42: both regressions — #16's silent convert failure and
// the empty web menu — happened in this previously untested logic).

// The "More" menu renders only the actions actually available: web has no
// projects backend, so when callbacks like onConvert are absent the buttons
// are not rendered, avoiding a menu that opens with zero items.
function groupHeaderHasMenu(kind, handlers) {
  const { onConvert, onRename, onDelete } = handlers || {};
  return (kind === 'folder' && !!onConvert)
    || (kind === 'project' && (!!onRename || !!onDelete));
}

// Submit an inline edit: returns { action: 'convert' | 'rename', value } or
// null (cancel). "Unchanged value = cancel" holds only for rename: convert
// prefills the directory name as the default project name, so pressing Enter
// directly must create with the prefilled value — otherwise the default path
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
