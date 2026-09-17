/**
 * Contract for dragging sidebar sessions onto project groups. The producer
 * (RecentItem in NavigationComponents) and the consumer (ProjectGroupHeader)
 * must agree on the drag MIME and on the memo-stable prop contract, or the
 * gesture silently no-ops — no highlight, no preventDefault, no drop, no
 * console output, no failing test. This file pins the source-level invariants
 * so a rewrite cannot silently drop them (a previous branch rewrite already
 * did once). Static source assertions, following the
 * move_dialog_contract.test.mjs pattern.
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

const GROUPING = read('src', 'features', 'projects', 'projectGrouping.js');
const HEADER = read('src', 'features', 'projects', 'ProjectGroupHeader.jsx');
const NAV = read('src', 'components', 'layout', 'NavigationComponents.jsx');
const MAIN = read('src', 'app', 'main.jsx');
const CHAT = read('src', 'features', 'chat', 'ChatView.jsx');
const MOVE_DIALOG = read('src', 'features', 'projects', 'MoveToProjectDialog.jsx');

const MIME = 'application/x-pinvou-session';

test('drag MIME lives in exactly one shared definition', () => {
  assert.match(
    GROUPING,
    new RegExp(`const PROJECT_SESSION_DRAG_TYPE = '${MIME.replace('/', '\\/')}';`),
    'the literal must be defined once in the shared projects module',
  );
  assert.match(GROUPING, /export \{[^}]*PROJECT_SESSION_DRAG_TYPE/, 'the shared type must be exported');
  assert.ok(!HEADER.includes(`'${MIME}'`), 'consumer must import the shared type, not re-declare the literal');
  assert.ok(!NAV.includes(`'${MIME}'`), 'producer must import the shared type, not hardcode the literal');
  // The consumer may import the shared type alongside other symbols from the
  // same module (ProjectGroupHeader also pulls the badge-cap helper); the
  // invariant is "imported from the shared definition", not the import shape.
  assert.match(HEADER, /import \{[^}]*PROJECT_SESSION_DRAG_TYPE[^}]*\} from '\.\/projectGrouping\.js';/);
  assert.match(NAV, /import \{ PROJECT_SESSION_DRAG_TYPE \} from '\.\.\/\.\.\/features\/projects\/projectGrouping\.js';/);
});

test('producer and consumer agree on the shared type end to end', () => {
  assert.match(
    NAV,
    /e\.dataTransfer\.setData\(PROJECT_SESSION_DRAG_TYPE, dndPayload\.sessionId\)/,
    'the row must publish the session id under the shared type',
  );
  assert.match(
    HEADER,
    /e\.dataTransfer\.types\.includes\(PROJECT_SESSION_DRAG_TYPE\)/,
    'dragover highlight must gate on the shared type, else no ring and no preventDefault',
  );
  assert.match(
    HEADER,
    /const sessionId = e\.dataTransfer\.getData\(PROJECT_SESSION_DRAG_TYPE\);/,
    'drop must read the session id under the shared type',
  );
});

test('HTML5 drag starts only from the label button drag surface', () => {
  // Same gesture surface as the tear-off long-press (useLongPressDrag skips
  // presses on non-surface buttons): initiating a drag from the pin/more
  // action buttons must not pick the row up, and the action buttons' own
  // clicks must stay clickable (the swipe path also swallows them otherwise).
  const surface = NAV.indexOf('data-drag-surface');
  const draggable = NAV.indexOf('draggable=');
  assert.notStrictEqual(surface, -1, 'label button must carry the data-drag-surface marker');
  assert.notStrictEqual(draggable, -1, 'draggable must exist');
  assert.ok(surface < draggable, 'draggable must live on the label button, not the row container');
  const rowContainer = NAV.slice(0, surface);
  assert.ok(!rowContainer.includes('onDragStart='), 'row container must not start HTML5 drags');
});

test('dnd payload stays memo-stable and mirrors the menu-path availability gate', () => {
  assert.match(
    MAIN,
    /const projectMovesAvailable = \(chat\.taskKind === 'codex' \|\| !!chat\.workspacePath\) && bridge\.projects && !!sidebarProjectsData\?\.projects\?\.length;/,
    'drag and menu move must share one availability gate (incl. non-empty project list; bound plain sessions included, unify #464)',
  );
  assert.match(
    MAIN,
    /dndPayload=\{projectMovesAvailable && sidebarCodeListActive\s*\? cachedItemCallback\(sidebarDndPayloads, chat, \(c\) => \(\{ sessionId: c\.id \}\)\)/,
    'payload must come from the per-item cache, not a fresh object per render',
  );
  assert.doesNotMatch(
    MAIN,
    /\{ sessionId: chat\.id \}/,
    'inline payload object defeats the RecentItem memo (review round 5, major 1)',
  );
  assert.match(MAIN, /onDragEnd=\{clearDropTarget\}/, 'onDragEnd must be the stable callback');
  assert.match(
    MAIN,
    /const clearDropTarget = useCallback\(\(\) => setDropTargetGroupKey\(null\), \[\]\)/,
    'an inline arrow here re-renders every sidebar row on every App render',
  );
});

test('drop handler guards busy before opening the preset confirm', () => {
  const start = MAIN.indexOf('const handleDropSessionOnProject = (sessionId, projectId) => {');
  assert.notStrictEqual(start, -1);
  const body = MAIN.slice(start, start + 1600);
  const busyGuard = body.indexOf('if (projectOpsBusy) return;');
  const presetOpen = body.indexOf('setMoveToPresetProject(projectId);');
  assert.notStrictEqual(busyGuard, -1, 'a busy drop must be ignored, not mount an inert all-disabled confirm panel');
  assert.notStrictEqual(presetOpen, -1);
  assert.ok(busyGuard < presetOpen, 'the busy guard must run before the preset branch');
});

test('the bound workspace kind is one shared constant between emitter and consumer', () => {
  // review #464 round-5 item 5: if the producer (main.jsx) and the consumer
  // (projectGrouping.js) spell 'bound' independently, a rename on either side
  // silently drops bound sessions from the project view with every behavioral
  // test green (the grouping tests inject the string into the pure function).
  assert.match(GROUPING, /export const WORKSPACE_KIND_BOUND = 'bound'/, 'consumer must export the shared kind constant');
  assert.match(GROUPING, /WORKSPACE_KINDS_WITH_PROJECT_DIR = \['project', WORKSPACE_KIND_BOUND\]/, 'consumer list must use the constant');
  assert.match(MAIN, /workspaceKind: s\.workspace_binding \? WORKSPACE_KIND_BOUND : ''/, 'emitter must use the shared constant, not a string literal');
});

test('the project view pulls bound sessions in, gated to the desktop host', () => {
  // review #464 round-6 finding 9: this inclusion clause is what actually puts
  // a bound plain session into the project view. Deleting it drops them from
  // the view with every other test still green — the constant contract above
  // pins the emitter field, not this consumer.
  assert.match(
    MAIN,
    /const sidebarCodeTasks = useMemo\(\(\) => \(sidebarCodeListActive\n\s*\? sidebarTaskHistory\.filter\(chat => chat\.taskKind === 'codex'[\s\S]{0,600}?\|\| \(can\('desktopChrome'\) && chat\.taskKind === 'regular' && chat\.workspacePath\)\)/,
    'the bound-session inclusion clause must exist, and only under the desktop capability',
  );
  // finding 8b: on web the backend degrades workspace_binding to a leaf name,
  // so an unguarded clause buckets same-named directories together.
  assert.match(MOVE_DIALOG, /const workspacePath = hasProjectWorkspace\(session\) \?/,
    'the dialog must ask the shared predicate, not re-derive from workspaceKind');
});

test('the chat binding cache cannot be re-poisoned after a rebind invalidation', () => {
  // review #464 round-5 item 6 / round-6 finding 8a: the cache wipe runs on the
  // sessions-slice change, but a query issued just before it can resolve after
  // it. Both writes — the chip effect's and resolveBindingForGate's — must be
  // invalidated by the generation bump, or the pre-rebind directory is served
  // to every later reader (stale chip, and a null value skips the YOLO gate).
  assert.match(CHAT, /workspaceBindingEpochRef\.current \+= 1;/,
    'the cache wipe must bump the query generation');
  assert.match(CHAT, /if \(!cancelled && cacheStillValid\(\)\) \{\n\s*workspaceBindingCacheRef\.current\[sid\] = normalized;/,
    'the chip effect cache write must be cancelled- and generation-guarded');
  assert.match(CHAT, /if \(workspaceBindingEpochRef\.current === epoch\) \{\n\s*workspaceBindingCacheRef\.current\[sid\] = normalized;/,
    'the gate resolver cache write must be generation-guarded too (sid-keying alone is not enough)');
  // review #464 round-6 finding 8c: a same-session revalidation keeps the prior
  // value instead of dropping the chip to null first.
  assert.match(CHAT, /if \(bindingSidRef\.current !== sid\) \{\n\s*bindingSidRef\.current = sid;\n\s*setSessionWorkspaceBinding\(null\);/,
    'the synchronous null clear must be limited to a real session switch');
});
