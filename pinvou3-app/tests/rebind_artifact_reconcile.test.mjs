// Frontend half of the artifact-path rebase contract (review #463 round-10
// Major 2 + round-B Major 1): after a folder rebind the persisted
// artifacts[].storage_path entries keep the vanished absolute root, and two
// gated mechanisms keep the frontend from fighting the backend lane:
// 1. the switch-session reconcile rebases a stale absolute entry onto the
//    same-basename workspace file — ONLY for a session carrying a fresh
//    workspace_rebound mark (an unconditional arm would repoint a live
//    absolute entry outside the workspace onto an unrelated same-basename
//    workspace file and persist the damage);
// 2. every wholesale artifact save (turn end / session switch / reconcile)
//    rebases from→to while the mark exists — a chat turn's buffer save must
//    not durably revert the backend lane's rebase of SavedSession.artifacts.
// The mark {at, from, to} is stamped by the sessions.js listener from the
// session:list_changed payload; the backend emits it for rebound, failed AND
// post-busy ids with the rebind geometry (app/commands/projects.rs
// emit_workspace_rebound_events). The backend rebase itself is pinned in
// features/sessions/tests.rs.
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
// way rebind_dialog_contract.test.mjs pins main.jsx wiring. The web host is a
// parallel implementation with its own save sites and listener — the rebind
// events are forwarded to WebUI clients, so its transform must exist too
// (round-C Major 1).
const sessionsSource = read('platform/tauri/bridge/sessions.js');
assert.match(
  sessionsSource,
  /payload\.action === "workspace_rebound" && payload\.id && payload\.from && payload\.to/,
  'the session:list_changed listener must require the rebind geometry',
);
assert.match(
  sessionsSource,
  /last\.to === payload\.from/,
  'chained rebinds must APPEND a segment so every buffer vintage resolves',
);
assert.match(
  sessionsSource,
  /chain: \[\{ from: payload\.from, to: payload\.to \}\]/,
  'the mark must carry the timestamp and the segment chain',
);
const webSource = read('platform/web/bridge.js');
assert.match(
  webSource,
  /payload\.action === "workspace_rebound" && payload\.id && payload\.from && payload\.to/,
  'the web listener must stamp the mark from the forwarded payload',
);
assert.match(
  webSource,
  /function rebaseArtifactPathsForRebind\(sid, paths\)/,
  'the web host must carry the save transform',
);
const webSaveSites = webSource.match(/save_session_artifacts", \{ id: sid, paths: rebaseArtifactPathsForRebind\(/g) || [];
assert.equal(
  webSaveSites.length,
  2,
  'both web wholesale saves (persistMessagesFor + reconcile) must transform their paths',
);
// The backend emit site must carry the geometry, cover all three lists, and
// also fire on the roots-commit error path (round-C minor 1) — otherwise the
// documented retry, which cannot re-admit metadata-synced sessions, leaves
// resident buffers unmarked.
const projectsRs = read('../src-tauri/src/app/commands/projects.rs').replace(/\r\n/g, '\n');
assert.match(
  projectsRs,
  /emit_workspace_rebound_events\(\n\s*&app,\n\s*rebound_session_ids\n\s*\.iter\(\)\n\s*\.chain\(&failed_session_ids\)\n\s*\.chain\(&post_busy_session_ids\),/,
  'the events must cover rebound, failed and post-busy sessions',
);
assert.match(
  projectsRs,
  /"from": from\.display\(\)\.to_string\(\),\n\s*"to": to\.display\(\)\.to_string\(\),/,
  'the event payload must carry the rebind geometry',
);
assert.match(
  projectsRs,
  /if let Err\(error\) = &roots_result \{\n\s*emit_workspace_rebound_events\(/,
  'the roots-commit error path must mark the already-rebased sessions too',
);
// The view-heal window must not prune the mark: the save transform shares it
// and its whole-process-lifetime contract owns the lifetime (round-C Major 2).
assert.doesNotMatch(
  read('platform/tauri/bridge/artifact-tracker.js'),
  /delete marks\[sid\]/,
  'an expired window must not delete the mark the save transform depends on',
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

const MARK = { at: Date.now(), chain: [{ from: '/old/root', to: '/new/root' }] };

// 1. Reconcile: a stale absolute entry from the vanished root rebases onto
//    the scanned workspace file when the session carries a fresh mark, and
//    the rebased (already re-transformed) list is persisted.
{
  const state = {
    activeSessionId: 's1',
    reboundSessionIds: { s1: { ...MARK } },
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

// 2. Reconcile WITHOUT the mark must stay untouched: a live absolute entry
//    outside the workspace is not a dead one, and repointing it onto an
//    unrelated same-basename workspace file would durably open a different
//    file than the one produced (round-A review minor).
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

// 3. Reconcile with an expired mark no longer authorizes the VIEW rebase
//    (stale marks must not misfire on a later, unrelated basename collision)
//    — but the mark must SURVIVE for the save transform (case 7).
{
  const state = {
    activeSessionId: 's4',
    reboundSessionIds: { s4: { ...MARK, at: Date.now() - 11 * 60 * 1000 } },
    artifacts: [{ path: '/old/root/report.html', basename: 'report.html' }],
  };
  const { tracker } = makeTracker(state, ['/new/root/report.html']);
  await tracker.reconcileArtifacts('s4');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/old/root/report.html'],
    'an expired rebind mark must not authorize the view rebase',
  );
  assert.ok(
    state.reboundSessionIds.s4,
    'the expired mark must survive — the save transform still needs it',
  );
}

// 4. Reconcile: an entry whose absolute path the scan reproduces verbatim
//    stays put, and a scanned file with no tracked entry is added as before.
{
  const state = {
    activeSessionId: 's2',
    reboundSessionIds: { s2: { ...MARK } },
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

// 5. The save transform (round-B Major 1): with a mark, absolute paths under
//    the old root map onto the new root with suffix and casing preserved —
//    this is what keeps a post-rebind chat turn's wholesale buffer save from
//    reverting the backend lane's persisted rebase. Unrelated absolute paths
//    and relative paths pass through untouched.
{
  const state = { reboundSessionIds: { s5: { ...MARK } } };
  const { tracker } = makeTracker(state, []);
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s5', [
      '/old/root/report.html',
      '/old/root/sub/deep/a.png',
      '/OLD/ROOT/CASED.Docx',
      '/elsewhere/live/file.md',
      'relative/output.md',
      '/old/rootx/sibling-prefix.md',
    ]),
    [
      '/new/root/report.html',
      '/new/root/sub/deep/a.png',
      '/new/root/CASED.Docx',
      '/elsewhere/live/file.md',
      'relative/output.md',
      '/old/rootx/sibling-prefix.md',
    ],
    'from-prefix absolute paths follow the new root; everything else is untouched',
  );
}

// 6. Without a mark the transform is an identity (the ordinary save path of
//    every never-rebound session).
{
  const state = { reboundSessionIds: {} };
  const { tracker } = makeTracker(state, []);
  const paths = ['/old/root/report.html', 'relative/x.md'];
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s6', paths),
    paths,
    'no mark: the save path must be untouched',
  );
}

// 7. The transform OUTLIVES the window (round-C Major 2): an expired mark no
//    longer authorizes the view heal but still protects the save — this is
//    exactly the state a session re-visited >10 minutes after its rebind is
//    in, with a still-stale cached buffer.
{
  const state = {
    reboundSessionIds: { s7: { ...MARK, at: Date.now() - 11 * 60 * 1000 } },
  };
  const { tracker } = makeTracker(state, []);
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s7', ['/old/root/sub/report.html']),
    ['/new/root/sub/report.html'],
    'an expired mark must still drive the save transform',
  );
}

// 8. The segment chain resolves EVERY buffer vintage in order (round-D
//    Major 1): after chained rebinds A→B→C, an A-era path maps A→B→C, a
//    buffer re-vintaged from the durable JSON between the two rebinds
//    (B-era) maps B→C, and a C-era path stays put. A composed single
//    segment {A→C} would strand the B-era vintage.
{
  const state = {
    reboundSessionIds: {
      s8: {
        at: Date.now(),
        chain: [
          { from: '/a/root', to: '/b/root' },
          { from: '/b/root', to: '/c/root' },
        ],
      },
    },
  };
  const { tracker } = makeTracker(state, []);
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s8', [
      '/a/root/report.html',
      '/b/root/report.html',
      '/c/root/report.html',
    ]),
    [
      '/c/root/report.html',
      '/c/root/report.html',
      '/c/root/report.html',
    ],
    'each vintage resolves onto the final target through the ordered chain',
  );
}

console.log('rebind artifact reconcile contract passed');
