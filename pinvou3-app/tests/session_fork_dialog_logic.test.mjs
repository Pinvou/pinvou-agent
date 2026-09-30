/**
 * Fork-session dialog logic (docs/fork-session-plan.md §6.2-3): pure-logic
 * tests for the plan state in features/sessions/forkDialogState.js plus the
 * trilingual copy contract for the dialog (defaults, per-root overrides, the
 * one-time ownership notice, and static source invariants of the dialog
 * shell, following the session-management / move-dialog contract patterns).
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  forkConfirmEnabled,
  forkCopyPathPreview,
  forkDialogRoots,
  initialIsolationByRoot,
  selectedIsolateRoots,
} from '../src/features/sessions/forkDialogState.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const ROOTS = ['/home/u/projects/bugfix', '/home/u/docs/api-spec', '/home/u/projects/shared-lib'];

test('default toggles isolate the primary root and share the attached roots', () => {
  // D4: primary is the agent's write surface → isolated; attached roots are
  // usually reference material → shared.
  assert.deepEqual(initialIsolationByRoot(ROOTS), {
    '/home/u/projects/bugfix': true,
    '/home/u/docs/api-spec': false,
    '/home/u/projects/shared-lib': false,
  });
  // Degenerate shapes: empty / missing list → empty toggles (unbound session
  // keeps the isolate option disabled).
  assert.deepEqual(initialIsolationByRoot([]), {});
  assert.deepEqual(initialIsolationByRoot(undefined), {});
  assert.deepEqual(forkDialogRoots(null), []);
});

test('per-root overrides drive the selected isolation list in keychain order', () => {
  const byRoot = initialIsolationByRoot(ROOTS);
  byRoot['/home/u/docs/api-spec'] = true; // 用户把附加根也勾上
  byRoot['/home/u/projects/bugfix'] = false; // 主根改回共享
  assert.deepEqual(selectedIsolateRoots(ROOTS, byRoot), ['/home/u/docs/api-spec']);
  assert.deepEqual(selectedIsolateRoots(ROOTS, initialIsolationByRoot(ROOTS)), ['/home/u/projects/bugfix']);
});

test('confirm stays disabled for an isolation plan with zero selected roots', () => {
  const byRoot = initialIsolationByRoot(ROOTS);
  byRoot['/home/u/projects/bugfix'] = false;
  assert.equal(forkConfirmEnabled('isolate', ROOTS, byRoot), false);
  assert.equal(forkConfirmEnabled('isolate', ROOTS, initialIsolationByRoot(ROOTS)), true);
  assert.equal(forkConfirmEnabled('share', ROOTS, byRoot), true);
});

test('copy path preview follows the backend naming convention with an unknown id suffix', () => {
  // The backend mints `<name>-fork-<new-session-id-first-4>`; the id only
  // exists after creation, so the preview pins the parent + prefix and
  // marks the unknown tail.
  assert.equal(forkCopyPathPreview('/home/u/projects/bugfix'), '/home/u/projects/bugfix-fork-xxxx');
  assert.equal(forkCopyPathPreview('C:\\work\\repo'), 'C:/work/repo-fork-xxxx');
  assert.equal(forkCopyPathPreview(''), '');
  assert.equal(forkCopyPathPreview(undefined), '');
});

test('trilingual copy: every language carries the full uiForkSession set with the no-cleanup notice', async () => {
  const zh = (await import('../src/shared/i18n/zh.js')).dictZh;
  const en = (await import('../src/shared/i18n/en.js')).dictEn;
  const ja = (await import('../src/shared/i18n/ja.js')).dictJa;
  const expectedKeys = [
    'title', 'scopeLabel', 'scopeFull', 'workspaceLabel', 'shareAll', 'isolate',
    'isolateHint', 'copiesNotice', 'copiesNoCleanup', 'create', 'busy', 'success', 'failed',
  ].sort();
  for (const dict of [zh, en, ja]) {
    const copy = dict.uiForkSession;
    assert.ok(copy, 'uiForkSession must exist');
    assert.deepEqual(Object.keys(copy).sort(), expectedKeys);
    assert.equal(typeof copy.success, 'function', 'success is parameterized by the new title');
    assert.equal(typeof copy.copiesNotice, 'function', 'copiesNotice is parameterized by the count');
  }
  // The one-time ownership declaration (D6) is the load-bearing sentence.
  assert.ok(zh.uiForkSession.copiesNoCleanup.includes('不会随会话删除'), 'zh notice must state the no-auto-cleanup contract');
  assert.ok(en.uiForkSession.copiesNoCleanup.toLowerCase().includes('not removed'), 'en notice must state the no-auto-cleanup contract');
  assert.ok(ja.uiForkSession.copiesNoCleanup.includes('自動的に削除されません'), 'ja notice must state the no-auto-cleanup contract');
  // Menu label exists in all three languages (RecentItem reads t.riFork).
  assert.equal(zh.riFork, '分叉会话');
  assert.equal(en.riFork, 'Fork session');
  assert.equal(ja.riFork, 'セッションをフォーク');
});

test('dialog shell source contract: notice, busy gating, testids', () => {
  const DIALOG = read('src', 'features', 'sessions', 'ForkSessionDialog.jsx');
  // The ownership notice renders inside the isolate branch only.
  assert.match(DIALOG, /copiesNoCleanup/, 'the no-cleanup declaration must render');
  assert.match(DIALOG, /copiesNotice\(/, 'the copy-path list must render');
  assert.match(DIALOG, /data-testid="fork-session-notice"/);
  assert.match(DIALOG, /data-testid="fork-session-roots"/);
  assert.match(DIALOG, /data-testid="fork-session-confirm"/);
  // Busy gating: confirm disabled while forking, close intercepted.
  assert.match(DIALOG, /disabled=\{!canConfirm\}/);
  assert.match(DIALOG, /busyRef\.current\) return;/, 'Escape/backdrop close must be intercepted while busy');
  // Plan state comes from the pure module (no inline plan arithmetic).
  assert.match(DIALOG, /from '\.\/forkDialogState\.js'/);
});

test('main wiring contract: dialog consumes chat.workspaceRoots and switches on success', () => {
  const MAIN = read('src', 'app', 'main.jsx');
  assert.match(MAIN, /workspaceRoots=\{forkDialogChat\.workspaceRoots\}/);
  assert.match(MAIN, /onConfirm=\{handleForkConfirm\}/);
  // D8: the fork switches to the new session immediately on success.
  assert.match(MAIN, /handleSwitchSession\(result\.sessionId\)/);
  // The bridge result's title lands in the success toast.
  assert.match(MAIN, /t\.uiForkSession\.success\(result\.title\)/);
  // Locale threads the UI language into the injected hint template.
  assert.match(MAIN, /forkSession\(chat\.id, null, isolateRoots, language\)/);
});
