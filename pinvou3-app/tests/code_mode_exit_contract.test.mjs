import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const main = readFileSync(new URL('../src/app/main.jsx', import.meta.url), 'utf8');

// Code mode is a mode, not a page: any path that materializes a normal chat or
// a scheduled-run conversation must clear codeModeOn first, otherwise the
// code-filtered sidebar and the codex-draft New chat leak onto a regular
// session (review round 2, both P1 findings). These are source-contract
// assertions in the style of pet_navigation_contract.test.mjs — main.jsx has
// no DOM harness, so the invariant is pinned on the transition sites.

function windowFrom(marker, length = 700) {
  const at = main.indexOf(marker);
  assert.notStrictEqual(at, -1, `marker not found in main.jsx: ${marker}`);
  return main.slice(at, at + length);
}

function assertClearBefore(window, view, label) {
  const clearAt = window.indexOf('setCodeModeOn(false)');
  const navAt = window.indexOf(`setCurrentView('${view}')`);
  assert.notStrictEqual(navAt, -1, `${label}: no navigation to '${view}' found`);
  assert.notStrictEqual(clearAt, -1, `${label}: navigation to '${view}' does not exit code mode`);
  assert.ok(clearAt < navAt, `${label}: code mode must be cleared before navigating to '${view}'`);
}

// 1. Bridge sync effect: an externally changed activeSessionId materializes a
// normal chat view (web remote control and friends).
assertClearBefore(
  windowFrom("if (bs.activeSessionId !== activeChat)"),
  'chat',
  'bridge activeSessionId sync',
);

// 2. Bridge sync effect: a new composerPrefill lands on the normal chat
// composer regardless of the current view, including the codex view.
assertClearBefore(
  windowFrom('composerPrefillSeenRef.current = bs.composerPrefill.id;'),
  'chat',
  'bridge composerPrefill',
);

// 3. Scheduled-run shortcut: both the unavailable-bridge fallback and the
// successful openScheduledRunChat branch land on the scheduled view showing a
// regular conversation, so the mode is cleared once at the entry.
{
  const body = windowFrom('async function handleOpenScheduledRunShortcut(run)', 1200);
  assertClearBefore(body, 'scheduled', 'handleOpenScheduledRunShortcut');
  const firstNav = body.indexOf("setCurrentView('scheduled')");
  assert.ok(
    body.indexOf("setCurrentView('scheduled')", firstNav + 1) > -1,
    'handleOpenScheduledRunShortcut: expected both branches covered by the shared entry-point clear',
  );
}

// 4. Pet scheduled notice: opening the run chat lands on the scheduled view.
assertClearBefore(
  windowFrom('opened = await bridge.scheduled.openScheduledRunChat({', 1000),
  'scheduled',
  'pet scheduled notice',
);

// 5. Pet navigation fallbacks without a resolvable session land on chat.
assertClearBefore(
  windowFrom('const sid = request.session_id || request.sessionId;'),
  'chat',
  'pet navigation without session id',
);
assertClearBefore(
  windowFrom("emitToPet('pet:session_unavailable', { session_id: sid })", 500),
  'chat',
  'pet navigation with unknown session',
);

// 6. HMR/legacy-state fallback off the retired scheduled entry lands on chat.
assertClearBefore(
  windowFrom("if (!SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled')"),
  'chat',
  'retired scheduled entry fallback',
);

console.log('code mode exit contract tests passed');
