/**
 * Interaction-shell contract for the "Rebind project folder" confirm dialog.
 * The dialog shipped with no test coverage at all, which is how a
 * browser-surface-suspend omission survived nine review rounds (review #463
 * round-10 T1): every other modal in main.jsx publishes an intent and waits for
 * the publication barrier, and this one did neither. No React-render suite
 * exercises this layer, so — following move_dialog_contract.test.mjs — this file
 * pins the source-level invariants that a refactor must not silently drop:
 * dock suspension, close-time focus restore to a surviving target, busy gating,
 * Escape tiering, the two-half backdrop guard and the per-id list rendering.
 * Static source assertions by design.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const DIALOG = read('src', 'features', 'projects', 'RebindFolderDialog.jsx');
const MAIN = read('src', 'app', 'main.jsx');
const BRIDGE = read('src', 'platform', 'tauri', 'bridge', 'projects.js');

test('rebind dialog participates in the browser-surface suspend protocol', () => {
  // The native webview dock must be suspended before the modal mounts, or the
  // backdrop has no authority over the dock region (occlusion + click-through).
  // In the partial state this dialog is the only retry entry, so a dock that
  // keeps swallowing clicks makes the retry unreachable (round-10 T1).
  assert.match(
    MAIN,
    /rebindDraft \? 'rebind' : '',/,
    'rebind intent missing from browserOverlayIntent',
  );
  assert.match(
    MAIN,
    /\{rebindDraft && browserOverlayPublicationReady && \(\r?\n/,
    'rebind render is not gated on browserOverlayPublicationReady',
  );
});

test('close-time focus restore targets a node that survives the refresh', () => {
  // The badge that opens the dialog is unmounted by the operation it starts
  // (its root reads available again via the `available` wire field — the
  // backend's per-root is_dir() stat, review #463 round-14 B1), so the
  // hook's default restore target is detached and focus fell to <body>; the
  // project header row survives (round-10 T7).
  assert.match(
    DIALOG,
    /useDialogFocusRestore\(dialogRef, confirmButtonRef, restoreTargetRef\)/,
    'the dialog must pass the container-supplied restore resolver',
  );
  assert.match(
    MAIN,
    /rebindRestoreRef\.current = headerEl\n?\s*\? \(\) => headerEl\.querySelector\('button'\)/,
    'main.jsx must resolve the surviving project header at close time',
  );
  assert.match(
    MAIN,
    /restoreTargetRef=\{rebindRestoreRef\}/,
    'the resolver must reach the dialog',
  );
  // The badge is the only caller, and it must hand the header element over.
  assert.match(
    read('src', 'features', 'projects', 'ProjectGroupHeader.jsx'),
    /onRebind\(rootPath, e\.currentTarget\.parentElement\)/,
    'the badge must pass its header row to onRebind',
  );
});

test('status id lists are rendered one per line', () => {
  // `join('\n')` only reads as a list when the container preserves whitespace;
  // `break-all` alone collapses the newlines into spaces, which defeats the
  // "copy one id for manual follow-up" intent (round-10 T8).
  const listContainers = DIALOG.match(/font-mono text-\[11px\][^"]*"/g) || [];
  assert.equal(listContainers.length, 2, 'expected the failed and busy id lists');
  for (const className of listContainers) {
    assert.match(className, /whitespace-pre-wrap/, `missing whitespace-pre-wrap: ${className}`);
  }
});

test('the dialog is busy-gated on every dismissal path', () => {
  assert.match(DIALOG, /disabled=\{busy\}/, 'controls must be disabled while a run is in flight');
  assert.match(
    DIALOG,
    /if \(!busyRef\.current\) onCancelRef\.current\(\)/,
    'Escape must not dismiss a run in flight',
  );
});

test('backdrop close requires both halves of the press', () => {
  // A press on the break-all path text released on the backdrop synthesizes a
  // click on the common ancestor, so a click-only guard closes a dialog the
  // user never meant to close (round-8 Major 3).
  assert.match(DIALOG, /backdropPressRef\.current = e\.target === e\.currentTarget/);
  assert.match(
    DIALOG,
    /if \(backdropPressRef\.current && e\.target !== e\.currentTarget\) backdropPressRef\.current = false/,
  );
  assert.match(
    DIALOG,
    /if \(!backdropPressRef\.current \|\| e\.target !== e\.currentTarget\) return/,
  );
});

test('the retry feeds the previous post-busy ids back to the backend', () => {
  assert.match(
    MAIN,
    /const previousPostBusySessionIds = \(rebindDraft\.partial && rebindDraft\.partial\.postBusyIds\) \|\| \[\]/,
    'the dialog must carry its previous report into the next run',
  );
  assert.match(
    BRIDGE,
    /previousPostBusySessionIds: previousPostBusySessionIds \|\| \[\]/,
    'the bridge must forward the fed-back list',
  );
});

test('an out-of-budget reclaim is reported instead of claimed as success', () => {
  // The reclaim tail shares one budget (round-10 T13); candidates it does not
  // reach must fall into the same post-busy reporting gate as a refused
  // eviction, never into the success path.
  assert.match(
    read('src-tauri', 'src', 'app', 'commands', 'projects.rs'),
    /let \(acp_idle, engine_idle\) = if tokio::time::Instant::now\(\) < tail_deadline \{\r?\n\s*\(\r?\n\s*acp_pool\.evict_if_idle_for_rebind\(&session_id\)\.await,\r?\n\s*engines\.evict_if_idle_for_rebind\(&session_id\)\.await,\r?\n\s*\)\r?\n\s*\} else \{\r?\n\s*\(false, false\)\r?\n\s*\}/,
    'the tail must not attempt (or claim) eviction past its budget',
  );
});

test('the carryover-refused-only report does not deny its own busy line', () => {
  // round-10 minor 6: the backend state failed=0, rebound=0, postBusy>0 (tail
  // budget exhaustion / refused carryover) used to render "nothing needed
  // rebinding" directly above "sessions are busy after the rebind" — the first
  // line denying what the second asserts. The summary line must render only
  // when it agrees with postBusy.
  assert.match(
    DIALOG,
    /\{\(partial\.failed > 0 \|\| partial\.rebound > 0 \|\| partial\.postBusy === 0\) && \(/,
    'the summary line must be suppressed in the carryover-refused-only state',
  );
});

test('the folder-picker window is re-entry guarded', () => {
  // round-10 minor 7: `rebindDraft` stays null until the picker resolves, so
  // the existing guard cannot see a second badge click in that window — two
  // pickers would race, the second overwriting the focus-restore target, and
  // a successful A-rebind could restore focus to B's header or open B's
  // draft. A pickingRef must gate entry and release on every settle path.
  assert.match(
    MAIN,
    /const rebindPickingRef = useRef\(false\)/,
    'the picking guard must exist',
  );
  assert.match(
    MAIN,
    /projectOpsBusy \|\| rebindDraft \|\| rebindPickingRef\.current\) return/,
    'entry must be refused while a pick is already in flight',
  );
  assert.match(
    MAIN,
    /finally \{\r?\n\s*rebindPickingRef\.current = false;\r?\n\s*\}/,
    'the guard must release on pick, empty pick AND failure',
  );
});

test('the strong-confirm flag and the stay-open gate survive refactors', () => {
  // Review #463 round-14 should-fix 4: the two highest-risk dialog gates.
  // The confirm button must forward the strong-confirmation state exactly —
  // onConfirm(true) would silently bypass the old-root-exists warning on
  // every later attempt.
  assert.match(
    DIALOG,
    /onClick=\{\(\) => onConfirm\(!{0,2}warnExisting\)\}/,
    'the confirm button must forward the strong-confirm flag, not a constant',
  );
  assert.doesNotMatch(
    DIALOG,
    /onClick=\{\(\) => onConfirm\(true\)\}/,
    'the strong confirm must never degrade to an unconditional true',
  );
  // The dialog must stay open whenever the report still carries work — failed
  // sessions OR post-busy sessions whose runtime the idle gate refused.
  // Dropping `postBusy > 0` closes the only retry entry while an old-cwd
  // runtime stays resident (round-8 MAJOR-2).
  assert.match(
    MAIN,
    /if \(failed > 0 \|\| postBusy > 0\) \{/,
    'the stay-open gate must cover the post-busy state, not just failures',
  );
});
