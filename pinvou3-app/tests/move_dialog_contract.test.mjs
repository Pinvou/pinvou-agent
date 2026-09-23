/**
 * Interaction-shell contract for the "Move to project" picker. Review rounds
 * landed their fixes in exactly this layer (browser-surface suspend, focus
 * handoff and restore, shared Tab trap, Escape tiering, busy gating, backdrop
 * press guard, memo-stable entry) and no React-render suite exercises it —
 * this file pins the source-level invariants so a refactor cannot silently
 * drop them. Static source assertions, following the
 * settings_window_confirm.test.mjs pattern.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
// Core.autocrlf=true gives a CRLF working tree on Windows while the repo holds
// LF; normalizing here keeps every pattern below platform-deterministic
// (multi-line anchors would otherwise silently match nothing on one platform).
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8').replace(/\r\n/g, '\n');

const DIALOG = read('src', 'features', 'projects', 'MoveToProjectDialog.jsx');
const MAIN = read('src', 'app', 'main.jsx');
const NAV = read('src', 'components', 'layout', 'NavigationComponents.jsx');
const HOOK = read('src', 'hooks', 'useDialogFocusRestore.js');

test('picker participates in the browser-surface suspend protocol', () => {
  // The native webview dock must be suspended before the picker mounts, or
  // the backdrop has no authority over the dock region (occlusion +
  // click-through), so the picker goes through the same publication barrier
  // as every other overlay.
  assert.match(
    MAIN,
    /moveToProjectSession \? 'move-picker' : '',/,
    'move-picker intent missing from browserOverlayIntent',
  );
  assert.match(
    MAIN,
    /moveToProjectSession && browserOverlayPublicationReady && \(\n/,    'picker render is not gated on browserOverlayPublicationReady',
  );
});

test('success clears only the moved session picker slot', () => {
  assert.match(
    MAIN,
    /setMoveToProjectSession\(current => \(current && current\.id === sessionId\) \? null : current\)/,
    'slot must be cleared conditionally by session id',
  );
});

test('picker entry keeps the RecentItem memo stable', () => {
  assert.match(MAIN, /const openMovePicker = useCallback\(/, 'entry must be a stable callback');
  assert.doesNotMatch(
    MAIN,
    /\? \(target\) => setMoveToProjectSession\(target\)/,
    'inline arrow on onMoveToProject defeats the RecentItem memo',
  );
});

test('focus trap and restore come from the shared hooks', () => {
  assert.match(DIALOG, /useDialogFocusRestore\(dialogRef, searchInputRef, restoreTargetRef\)/);
  assert.match(DIALOG, /useDialogFocusTrap\(dialogRef\)/);
  assert.doesNotMatch(
    DIALOG,
    /e\.key === 'Tab'/,
    'Tab cycling must live in useDialogFocusTrap, not a local copy',
  );
});

test('escape tiers through busy and the derived confirm target', () => {
  const handler = DIALOG.slice(DIALOG.indexOf('const onKey = (e) => {'));
  const busyGate = handler.indexOf('if (busyRef.current) return;');
  const pendingGate = handler.indexOf('if (pendingProjectRef.current)');
  const close = handler.indexOf('onCloseRef.current();');
  assert.notStrictEqual(busyGate, -1, 'escape must be gated on busy');
  assert.notStrictEqual(pendingGate, -1, 'escape must gate on the derived confirm target');
  assert.ok(busyGate < pendingGate && pendingGate < close, 'escape ordering must be busy > pending > close');
});

test('confirm target derives from the live project list', () => {
  assert.match(
    DIALOG,
    /projectList\.find\(project => project\.id === pendingMove\.id\)/,
    'a target deleted mid-flight must fall back instead of looping a dead id',
  );
});

test('backdrop close requires press start and end on the backdrop', () => {
  assert.match(DIALOG, /onMouseDown=\{\(e\) => \{ backdropPressRef\.current = e\.target === e\.currentTarget; \}\}/);
  assert.match(
    DIALOG,
    /onMouseUp=\{\(e\) => \{ if \(backdropPressRef\.current && e\.target !== e\.currentTarget\) backdropPressRef\.current = false; \}\}/,
    'a drag ending inside the dialog must not close it',
  );
  assert.match(DIALOG, /if \(!backdropPressRef\.current \|\| e\.target !== e\.currentTarget\) return;/);
});

test('focus restore survives the success regroup', () => {
  assert.match(MAIN, /const movePickerRestoreRef = useRef\(null\)/);
  assert.match(
    MAIN,
    /movePickerRestoreRef\.current = \(\) => \{/,
    'the regroup commit lands asynchronously; the lookup must run at close time, not inline',
  );
  assert.match(MAIN, /data-session-key="\$\{CSS\.escape\(String\(sessionId\)\)\}"/);
  assert.match(
    MAIN,
    /row \? row\.querySelector\('button\[data-drag-surface\]'\) : null/,
    'the row container is role="presentation" and cannot take focus; restore must target its focusable label button',
  );
  assert.match(
    HOOK,
    /typeof override === 'function' \? override\(\) : override/,
    'the override may be a resolver; the hook must call it during cleanup',
  );
  assert.match(
    HOOK,
    /resolved : null\) \|\| previous;/,
    'a vanished override target must fall back to the original restore element',
  );
  assert.match(
    DIALOG,
    /useDialogFocusRestore\(dialogRef, searchInputRef, restoreTargetRef\)/,
    'success-path restore must resolve the moved row\'s new node',
  );
  assert.match(NAV, /data-session-key=\{chat\.id\}/);
});

test('a session deleted while the picker is open retires the picker at submit', () => {
  assert.match(
    MAIN,
    /\(allSidebarTasksRef\.current \|\| \[\]\)\.every\(task => task\.id !== sessionId\)/,
    'the picker holds the open-time snapshot; submit must re-check the live task list',
  );
});

test('move menu item hands focus to the always-rendered label button', () => {
  assert.match(
    NAV,
    /rowLabelRef\.current\?\.focus\(\); closeMenu\(\); onMoveToProject\(chat\)/,
    'the portal menu item unmounts with the menu; handoff must go to a persistent element',
  );
  assert.match(
    NAV,
    /hidden group-hover:flex group-focus-within:flex max-sm:flex/,
    'action container must reveal on focus-within so keyboard users can reach it',
  );
});
