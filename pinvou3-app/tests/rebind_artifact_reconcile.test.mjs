// Frontend half of the artifact-path rebase contract (review #463 round-10
// Major 2): after a folder rebind the persisted artifacts[].storage_path
// entries keep the vanished absolute root, and the switch-session reconcile
// pass must rebase a stale absolute entry onto the same-basename file the
// workspace scan surfaces — but ONLY for a session the rebind command just
// moved (the workspace_rebound mark): an unconditional arm would also repoint
// a live absolute entry outside the workspace onto an unrelated same-basename
// workspace file and persist the damage (round-A review minor). The backend
// half (the rebase during the rebind metadata pass) is pinned in
// features/sessions/tests.rs; the mark producer is pinned by the sessions.js
// listener reading session:list_changed's {id, action:"workspace_rebound"}.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const read = relative => fs.readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

const windowObject = {};
const context = vm.createContext({ window: windowObject, console });
vm.runInContext(read('platform/tauri/bridge/artifact-tracker.js'), context, {
  filename: 'bridge/artifact-tracker.js',
});
const factory = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['artifact-tracker'];
assert.equal(typeof factory, 'function', 'artifact-tracker feature must register');

// The listener half lives in bridge/sessions.js; pin it source-level the same
// way rebind_dialog_contract.test.mjs pins main.jsx wiring.
const sessionsSource = read('platform/tauri/bridge/sessions.js');
assert.match(
  sessionsSource,
  /payload\.action === "workspace_rebound" && payload\.id/,
  'the session:list_changed listener must stamp workspace_rebound ids',
);
assert.match(
  sessionsSource,
  /state\.reboundSessionIds\[payload\.id\] = Date\.now\(\)/,
  'the mark must carry a timestamp for the freshness window',
);

function makeTracker(state, workspaceFiles) {
  const invokes = [];
  const tracker = factory({
    state,
    notify: () => {},
    invoke: async (command, payload) => {
      invokes.push([command, payload]);
      if (command === 'list_workspace_files') return workspaceFiles;
      return undefined;
    },
    isScheduledRunSession: () => false,
  });
  return { tracker, invokes };
}

// 1. A stale absolute entry from the vanished root rebases onto the scanned
//    workspace file when the session carries a fresh workspace_rebound mark,
//    and the rebased list is persisted.
{
  const state = {
    activeSessionId: 's1',
    reboundSessionIds: { s1: Date.now() },
    artifacts: [{ path: '/old/root/sub/report.html', basename: 'report.html' }],
  };
  const { tracker, invokes } = makeTracker(state, ['/new/root/sub/report.html']);
  await tracker.reconcileArtifacts('s1');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/new/root/sub/report.html'],
    'the stale absolute entry must follow the moved root inside the rebind window',
  );
  const saved = invokes.find(([command]) => command === 'save_session_artifacts');
  assert.ok(saved, 'the rebased list must be persisted');
  assert.deepEqual(saved[1].paths, ['/new/root/sub/report.html']);
}

// 2. WITHOUT the rebind mark the same shape must stay untouched: a live
//    absolute entry outside the workspace is not a dead one, and repointing
//    it onto an unrelated same-basename workspace file would durably open a
//    different file than the one produced (round-A review minor).
{
  const state = {
    activeSessionId: 's3',
    artifacts: [{ path: '/external/live/report.html', basename: 'report.html' }],
  };
  const { tracker, invokes } = makeTracker(state, ['/new/root/sub/report.html']);
  await tracker.reconcileArtifacts('s3');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/external/live/report.html'],
    'an external live entry must never be repointed without a rebind mark',
  );
  assert.ok(
    !invokes.some(([command]) => command === 'save_session_artifacts'),
    'no damage must be persisted',
  );
}

// 3. An expired mark no longer authorizes the rebase (stale marks must not
//    misfire on a later, unrelated basename collision).
{
  const state = {
    activeSessionId: 's4',
    reboundSessionIds: { s4: Date.now() - 11 * 60 * 1000 },
    artifacts: [{ path: '/old/root/report.html', basename: 'report.html' }],
  };
  const { tracker } = makeTracker(state, ['/new/root/report.html']);
  await tracker.reconcileArtifacts('s4');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/old/root/report.html'],
    'an expired rebind mark must not authorize the rebase',
  );
}

// 4. An entry whose absolute path the scan reproduces verbatim stays put (no
//    false rebase of a live workspace file onto itself or a sibling), and a
//    scanned file with no tracked entry is added as before.
{
  const state = {
    activeSessionId: 's2',
    reboundSessionIds: { s2: Date.now() },
    artifacts: [{ path: '/live/root/a.html', basename: 'a.html' }],
  };
  const { tracker } = makeTracker(state, ['/live/root/a.html', '/live/root/b.png']);
  await tracker.reconcileArtifacts('s2');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/live/root/a.html', '/live/root/b.png'],
    'verbatim matches stay put; unknown deliverables are added',
  );
}

console.log('rebind artifact reconcile contract passed');
