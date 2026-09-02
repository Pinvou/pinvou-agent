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
  windowFrom('const publishSession = ({ isCurrent }) => {', 900),
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

// 3. Scheduled-run shortcut: the unavailable-bridge fallback and the
// successful openScheduledRunChat branch each land on the scheduled view
// showing a regular conversation, so each branch clears the mode right
// before navigating. The entry must NOT clear unconditionally: when the open
// fails the user is still on the active code session, and an entry-side clear
// would leave it behind the standard sidebar.
{
  const head = windowFrom('async function handleOpenScheduledRunShortcut(run)', 700);
  const fallbackAt = head.indexOf('if (!bridge.available || !bridge.scheduled.openScheduledRunChat)');
  assert.ok(fallbackAt > -1, 'handleOpenScheduledRunShortcut: fallback branch marker not found');
  assert.ok(
    !head.slice(0, fallbackAt).includes('setCodeModeOn(false)'),
    'handleOpenScheduledRunShortcut: entry must not clear code mode — the failed-open path stays on the active code session',
  );
}
assertClearBefore(
  windowFrom('if (!bridge.available || !bridge.scheduled.openScheduledRunChat)'),
  'scheduled',
  'scheduled-run shortcut fallback branch',
);
assertClearBefore(
  windowFrom('const opened = await bridge.scheduled.openScheduledRunChat(run, task);'),
  'scheduled',
  'scheduled-run shortcut open branch',
);

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

// 7. New chat: the non-codex branch of handleNewChat lands on a normal chat
// draft. This covers the tool-intent path (tool store "new chat with this
// tool") that force-skips the codex draft even while code mode is on, plus
// forceMode='chat' call sites — the highest-traffic exit path of the mode.
assertClearBefore(
  windowFrom("// Every landing here is a normal chat (tool intent, forceMode='chat',", 600),
  'chat',
  'handleNewChat normal-chat branch',
);

// 8. Session list: opening a normal chat session is the baseline exit path.
assertClearBefore(
  windowFrom('const handleSwitchSession = useCallback(async (id) => {', 500),
  'chat',
  'handleSwitchSession',
);

// 9. navigateFromScheduledRun('chat') serves the collapsed-rail "current chat"
// and the mobile bottom tab — the round-2 P1 fix site. The clear precedes the
// navigation variable, matching the clear-before-nav order of the other sites.
assert.match(
  main,
  /if \(nextView === 'chat'\) setCodeModeOn\(false\);\s*setCurrentView\(nextView\);/,
  "navigateFromScheduledRun('chat'): navigating back to chat must exit code mode",
);

console.log('code mode exit contract tests passed');
