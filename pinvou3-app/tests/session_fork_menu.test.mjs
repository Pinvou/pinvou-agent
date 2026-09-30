/**
 * Fork menu-entry contract (docs/fork-session-plan.md §6.2-4): the sidebar
 * RecentItem "more" menu gains a fork entry gated to plain chat sessions that
 * are not generating (D12: the busy gate hides the entry; the backend rejects
 * a forced command as the second line). Web does not expose `forkSession`, so
 * the entry must be hidden there via the bridge-capability check (matrix #22:
 * explicitly unsupported, never a silent failure). Static source assertions
 * following the move_dialog_contract.test.mjs pattern.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(here, '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const NAV = read('src', 'components', 'layout', 'NavigationComponents.jsx');
const MAIN = read('src', 'app', 'main.jsx');
const WEB_BRIDGE = read('src', 'platform', 'web', 'bridge.js');

test('RecentItem menu renders the fork entry behind the onFork prop', () => {
  assert.match(NAV, /onFork,/, 'onFork must be part of the RecentItem props');
  assert.match(NAV, /data-testid="session-fork"/);
  assert.match(NAV, /t\.riFork/, 'the label comes from the trilingual dictionary');
  assert.match(NAV, /onFork\(chat\)/, 'the callback receives the whole chat object (keychain access)');
  // The portal menu height covers the extra row (36px per entry).
  assert.match(NAV, /usePortalMenu\(\{ height: 341 \}\)/, 'menu height must cover the ninth entry');
});

test('main.jsx gates the fork entry to idle plain chat sessions', () => {
  // taskKind !== 'codex' excludes code sessions (their lifecycle is owned by
  // the codex store); working/waitingInput hide the entry while generating;
  // the bridge-capability check keeps web without the entry.
  assert.match(
    MAIN,
    /onFork=\{chat\.taskKind !== 'codex' && !chat\.working && !chat\.waitingInput && bridge\.sessions\.forkSession \? handleOpenForkDialog : undefined\}/,
    'the fork entry must be gated on kind + busy + capability',
  );
  assert.match(MAIN, /const handleOpenForkDialog = useCallback\(/, 'the entry callback must be memo-stable (RecentItem memo)');
});

test('web bridge does not expose forkSession (explicitly unsupported, matrix #22)', () => {
  assert.doesNotMatch(
    WEB_BRIDGE,
    /forkSession/,
    'the web lane must not gain a silent stub; the UI hides the entry by capability',
  );
});

test('tauri bridge exports forkSession with in-flight dedupe', () => {
  const TAURI_SESSIONS = read('src', 'platform', 'tauri', 'bridge', 'sessions.js');
  assert.match(TAURI_SESSIONS, /async function forkSession\(/);
  assert.match(TAURI_SESSIONS, /forkingSessionIds\.has\(id\)/, 'repeat calls while in flight must be deduped');
  assert.match(TAURI_SESSIONS, /exportSessionArchive,\s*\r?\n\s*forkSession/, 'forkSession must be exported by the sessions lane');
});
