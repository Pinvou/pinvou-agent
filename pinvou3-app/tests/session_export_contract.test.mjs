import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// One-click session archive export: source-contract assertions (style of
// code_mode_exit_contract.test.mjs — main.jsx has no DOM harness). Pins the
// lane exclusion, the in-flight guard, the save-dialog default-name hygiene,
// and the menu wiring that the data-testid stands for.

const main = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');
const nav = readFileSync(new URL('../src/components/layout/NavigationComponents.jsx', import.meta.url), 'utf8');
const searchView = readFileSync(new URL('../src/features/search/SearchView.jsx', import.meta.url), 'utf8');

// 1. Handler: bridge guard, in-flight early return, code-point-safe title
// truncation, `pinvou-` default stem, and the save path surfaced in the
// success toast. The handler is a stable useCallback (RecentItem is
// memoized) reading the task list and the in-flight set through latest
// refs, so the assertions pin the ref reads rather than state closure.
{
  const at = main.indexOf('const handleExportSessionArchive = useCallback(async (id) => {');
  assert.notStrictEqual(at, -1, 'handleExportSessionArchive not found');
  const body = main.slice(at, main.indexOf('const handleToggleSessionPinned = useCallback', at));
  assert.ok(body.includes('if (exportingSessionIdsRef.current.has(id)) return;'), 'handler must early-return while an export is in flight');
  assert.ok(body.includes("[...title].slice(0, 30).join('')"), 'title truncation must be code-point aware (no surrogate-pair split)');
  assert.ok(body.includes('`pinvou-session-${stem}-'), 'default file name stem must be `pinvou-session-`');
  assert.ok(body.includes('setSettingsToast(t.exportSessionDone(result.path))'), 'success toast must surface the archive path');
  assert.ok(body.includes('} finally {') && body.includes('next.delete(id);'), 'in-flight marker must be released on cancel/failure too');
}

// 2. Sidebar menu: codex sessions excluded, in-flight session's item hidden.
assert.match(
  main,
  /onExportArchive=\{chat\.taskKind !== 'codex' && !exportingSessionIds\.has\(chat\.id\) && bridge\.sessions\.exportSessionArchive \? handleExportSessionArchive : undefined\}/,
  'sidebar must hide the export item for codex sessions and while exporting',
);

// 3. Chat-manager search view: codex route excluded.
assert.match(
  searchView,
  /onExportArchive=\{route === 'codex' \? undefined : onExportArchive\}/,
  'search view must not offer export on the codex route',
);

// 4. Menu item: rendered from onExportArchive with the contract testid.
assert.match(
  nav,
  /\{onExportArchive && \(\s*<button[^>]*data-testid="session-export-archive"/,
  'export menu item must be gated on onExportArchive and keep its data-testid',
);

// 5. Flip-up placement estimate covers the 7-item menu
// (6 items × h-9 36px + divider 9px + vertical padding 8px).
assert.ok(nav.includes('const height = 233;'), 'menu flip-up height estimate must match the current menu item count');

console.log('session export contract tests passed');
