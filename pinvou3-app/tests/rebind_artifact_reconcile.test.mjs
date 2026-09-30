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
// The mark {at, chain: [{from, to}...]} is stamped by the sessions.js
// listener from the
// session:list_changed payload; the backend emits it for rebound, failed AND
// post-busy ids with the rebind geometry (app/commands/projects.rs
// emit_workspace_rebound_events). The backend rebase itself is pinned in
// features/sessions/tests.rs.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const read = relative => fs.readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

const windowObject = {};
const context = vm.createContext({ window: windowObject, console });
// The shared-helper dedup relocated the tracker's function bodies into
// bridge-shared-helpers.js; the lane file's wrappers resolve them through
// window.PinvouBridgeShared, so the harness must load the shared base first
// (same order as the runtime: index.html loads the shared base before the
// bridges).
vm.runInContext(read('shared/bridge-shared-helpers.js'), context, {
  filename: 'shared/bridge-shared-helpers.js',
});
vm.runInContext(read('platform/tauri/bridge/artifact-tracker.js'), context, {
  filename: 'bridge/artifact-tracker.js',
});
const factory = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['artifact-tracker'];
assert.equal(typeof factory, 'function', 'artifact-tracker feature must register');

// The listener half lives in bridge/sessions.js; pin it source-level the same
// way rebind_dialog_contract.test.mjs pins main.jsx wiring. The web host is a
// parallel implementation with its own save sites and listener — the rebind
// events are forwarded to WebUI clients, so its transform must exist too
// (round-C Major 1). Since round-13 the stamp semantics live ONCE in the
// shared base (the block was byte-duplicated between the two listeners and
// the round-D chain fix had to land twice); both listeners delegate.
const sharedSource = read('shared/bridge-shared-helpers.js');
assert.match(
  sharedSource,
  /payload\.action !== "workspace_rebound" \|\| !payload\.id \|\| !payload\.from \|\| !payload\.to/,
  'the shared stamp must require the rebind geometry',
);
assert.match(
  sharedSource,
  /else if \(existing\)/,
  'chained and non-contiguous rebinds must APPEND so every buffer vintage resolves',
);
assert.match(
  sharedSource,
  /last\.from === payload\.from && last\.to === payload\.to/,
  'an identical retry must refresh the window without resetting the chain',
);
assert.match(
  sharedSource,
  /chain: \[\{ from: payload\.from, to: payload\.to \}\]/,
  'the mark must carry the timestamp and the segment chain',
);
const sessionsSource = read('platform/tauri/bridge/sessions.js');
const webSource = read('platform/web/bridge.js');
// Round-24 MAJOR 2: the previous bare /applyWorkspaceReboundMark\(payload\)/
// pins were satisfied by the module-level wrapper declarations themselves —
// deleting the listener's stamp call kept every suite green (the same
// phantom-pin shape rounds 18–21 kept re-banning). Pin the stamp CALL inside
// the session:list_changed listener span instead: between the rebind
// listener's registration and the next listener, exactly once per host.
function rebindListenerSpan(source, host) {
  const start = source.indexOf('listen("session:list_changed"');
  assert.ok(start >= 0, `${host} must register session:list_changed`);
  const next = source.indexOf('listen("session:model_changed"', start);
  assert.ok(
    next > start,
    `${host} must keep session:model_changed right after the rebind listener`,
  );
  return source.slice(start, next);
}
for (const [host, source] of [
  ['the tauri sessions bridge', sessionsSource],
  ['the web bridge', webSource],
]) {
  const span = rebindListenerSpan(source, host);
  const stamps = span.match(/applyWorkspaceReboundMark\(payload\);/g) || [];
  assert.equal(
    stamps.length,
    1,
    `${host} must stamp the rebind mark exactly once inside the session:list_changed listener`,
  );
}
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
  /"from": (?:event_)?from\.display\(\)\.to_string\(\),\n\s*"to": (?:event_)?to\.display\(\)\.to_string\(\),/,
  'the event payload must carry the rebind geometry',
);
assert.match(
  projectsRs,
  /Err\(error\) => \{\n\s*emit_workspace_rebound_events\(/,
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
      return null;
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

// 3b. Round-24 minor 3: INSIDE the freshness window the take-over still
//     requires the stored entry to be real stale rebind geometry. An
//     unrelated live absolute entry (not under any chain from-prefix) whose
//     basename collides with a workspace deliverable must stay put even
//     with a fresh mark — the mark alone used to authorize the take-over
//     and persist the wrong path. Red-verified by deleting the
//     pathIsRebindStale conjunct.
{
  const state = {
    activeSessionId: 's6',
    reboundSessionIds: { s6: { ...MARK } },
    artifacts: [{ path: '/external/live/report.html', basename: 'report.html' }],
  };
  const { tracker, invokes } = makeTracker(state, ['/new/root/sub/report.html']);
  await tracker.reconcileArtifacts('s6');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/external/live/report.html'],
    'a live non-rebind entry must never be taken over inside the window either',
  );
  assert.ok(
    !invokes.some(([command]) => command === 'save_session_artifacts'),
    'no damage must be persisted',
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

// 7b. UNC network-share paths are absolute for the transform (round-20
//    minor 6, round-21 R1 — the arm had shipped with no pin, so deleting it
//    kept every suite green while a UNC artifact path silently lost rebase
//    protection): both the STAMP and the REBASE must accept \\server\\share
//    spellings.
{
  const state = {
    reboundSessionIds: {
      's-unc': {
        at: Date.now(),
        chain: [
          { from: '\\\\server\\share\\old-root', to: '\\\\server\\share\\new-root' },
        ],
      },
    },
  };
  const { tracker } = makeTracker(state, []);
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s-unc', [
      '\\\\server\\share\\old-root\\report.html',
    ]),
    // The suffix cut joins with `/`, so the mapped spelling normalizes
    // separators — functionally the same UNC target on Windows, and the
    // point of the pin is that the path is PROTECTED (mapped at all)
    // rather than returned unmapped.
    ['//server/share/new-root/report.html'],
    'a UNC artifact path must rebase along the UNC segment',
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

// 9. Expanding case mappings (round-13): U+0130 "İ" lowercases to "i" +
//    U+0307 — two code units for one — so slicing the ORIGINAL path at the
//    LOWERCASED prefix's length eats a character of the saved suffix. The
//    match must be located in the original string; the fold is for the
//    comparison only. Without the fix this saved "/moved/İstanbul" + "a.txt"
//    (the separator eaten).
{
  const state = {
    reboundSessionIds: {
      s9: {
        at: Date.now(),
        chain: [{ from: '/data/İstanbul', to: '/moved/İstanbul' }],
      },
    },
  };
  const { tracker } = makeTracker(state, []);
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s9', [
      '/data/İstanbul/a.txt',
      '/data/İstanbul/sub/deep/b.png',
    ]),
    [
      '/moved/İstanbul/a.txt',
      '/moved/İstanbul/sub/deep/b.png',
    ],
    'an expanding case mapping in the matched prefix must not eat the suffix',
  );
  // The exact-match arm (path IS the root) and the non-matching arm still
  // behave: equality rebases, a sibling prefix does not.
  assert.deepEqual(
    tracker.rebaseArtifactPathsForRebind('s9', [
      '/data/İstanbul',
      '/data/İstanbulx/sibling.md',
    ]),
    [
      '/moved/İstanbul',
      '/data/İstanbulx/sibling.md',
    ],
    'equality rebases; a sibling prefix stays untouched',
  );
}

// 10. Round-14 R3: the mark chain is APPEND — driven behaviorally through the
//     shared base, not by source regex: two stamped payloads must keep both
//     segments resolvable (a `chain = [{...}]` replace — verbatim the round-E
//     regression the source pins name — would drop the older vintage while
//     every regex above stays green), and an identical retry refreshes the
//     window without resetting the chain.
{
  const state = {};
  const shared = windowObject.PinvouBridgeShared.create('web', { state });
  const stamp = (from, to) =>
    shared.applyWorkspaceReboundMark({ action: 'workspace_rebound', id: 's10', from, to });
  stamp('/old/root', '/new/root');
  stamp('/new/root', '/newer/root');
  // The chain is built inside the vm realm; compare a primitive
  // serialization (deepStrictEqual rejects cross-realm prototypes).
  assert.equal(
    state.reboundSessionIds.s10.chain.map((segment) => `${segment.from} -> ${segment.to}`).join(' | '),
    '/old/root -> /new/root | /new/root -> /newer/root',
    'a chained rebind must APPEND its segment, not replace the chain',
  );
  assert.deepEqual(
    shared.rebaseArtifactPathsForRebind('s10', ['/old/root/sub/a.md', '/new/root/b.md']),
    ['/newer/root/sub/a.md', '/newer/root/b.md'],
    'every buffer vintage resolves through the whole chain in order',
  );
  // Identical retry of the last segment: refresh the window only.
  const stampedAt = state.reboundSessionIds.s10.at;
  stamp('/new/root', '/newer/root');
  assert.equal(
    state.reboundSessionIds.s10.chain.length,
    2,
    'an identical retry must refresh the window without resetting the chain',
  );
  assert.ok(state.reboundSessionIds.s10.at >= stampedAt);
  assert.deepEqual(
    shared.rebaseArtifactPathsForRebind('s10', ['/old/root/sub/a.md']),
    ['/newer/root/sub/a.md'],
    'the chain still resolves after the retry refresh',
  );
}

console.log('rebind artifact reconcile contract passed');

// review #463 round-18 test-strength: the mark payload identity was pinned
// nowhere across the boundary — a backend rename of the action, the id key
// or the event name would silently disarm the whole mark/transform system
// with every behavioral test green. Read BOTH halves of the wire from
// source: the Rust emitter and the shared consumer must agree.
{
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const rust = fs.readFileSync(
    path.join(root, 'src-tauri', 'src', 'app', 'commands', 'projects.rs'),
    'utf8',
  );
  const shared = fs.readFileSync(
    path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'),
    'utf8',
  );
  const emitterStart = rust.indexOf('fn emit_workspace_rebound_events');
  const emitterEnd = rust.indexOf(
    '}',
    rust.indexOf('forward_app_event(app, "session:list_changed", payload);'),
  );
  const emitter = rust.slice(emitterStart, emitterEnd);
  assert.ok(emitter.includes('"workspace_rebound"'), 'the emitter must stamp action=workspace_rebound');
  assert.ok(emitter.includes('"id"'), 'the emitter must key the session as id');
  assert.ok(
    /"from": event_from\.display\(\)\.to_string\(\)/.test(emitter)
      && /"to": event_to\.display\(\)\.to_string\(\)/.test(emitter),
    'the emitter must carry the (possibly per-session) rebind geometry',
  );
  // round-20 R4: strand-repaired sessions converge onto an out-of-geometry
  // target, so the emitter must consult per-id geometry overrides with the
  // run-level pair only as the fallback.
  assert.ok(
    emitter.includes('per_id_geometry'),
    'the emitter must accept per-id geometry overrides',
  );
  assert.ok(
    emitter.includes('unwrap_or_else(|| (from.to_path_buf(), to.to_path_buf()))'),
    'non-repaired ids keep the run-level geometry as the fallback',
  );
  assert.ok(emitter.includes('"session:list_changed"'), 'the emitter must ride session:list_changed');
  assert.ok(
    shared.includes('payload.action !== "workspace_rebound"'),
    'the shared consumer must filter on the same action',
  );
  assert.ok(
    shared.includes('!payload.id')
      && shared.includes('!payload.from')
      && shared.includes('!payload.to'),
    'the shared consumer must require the same payload keys',
  );
  assert.ok(
    shared.includes('session:list_changed'),
    'the shared consumer must reference the same event',
  );
}
