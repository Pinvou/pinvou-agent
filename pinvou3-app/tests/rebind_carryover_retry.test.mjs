// Frontend half of the rebind carryover feed-back contract (review #463
// F-Major): the dialog keeps its previous report's post-busy ids and its
// retry feeds them back as `previousPostBusySessionIds`, so the backend can
// tell a carryover post-busy session (moved by an earlier run, old-cwd
// runtime still resident — must be reported post-busy again when the
// eviction refuses) apart from a healthy to-lane session (stays unreported).
// The Rust half pins the intersection logic in
// app/commands/projects.rs::tests::carryover_candidates_honor_only_the_to_lane_retry_population.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const read = relative => fs.readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

// 1. The tauri projects bridge forwards the feed-back list (defaulting to
//    []) in the rebind_workspace_root invoke payload.
const windowObject = {};
const context = vm.createContext({ window: windowObject, console });
vm.runInContext(read('platform/tauri/bridge/projects.js'), context, { filename: 'bridge/projects.js' });
const registerProjects = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.projects;
assert.equal(typeof registerProjects, 'function', 'projects bridge feature must register');

const invokes = [];
const bridge = registerProjects({
  state: {},
  notify: () => {},
  invoke: async (command, payload) => {
    // The payload was built inside the vm realm; a JSON round-trip gives it
    // this realm's Object.prototype so deepStrictEqual can compare it.
    invokes.push([command, JSON.parse(JSON.stringify(payload))]);
    return { rebound_session_ids: [], failed_session_ids: [], affected_project_ids: [], post_busy_session_ids: [] };
  },
  listen: async () => () => {},
});

await bridge.rebindWorkspaceRoot('/from', '/to', false);
const first = invokes.find(([command]) => command === 'rebind_workspace_root');
assert.deepEqual(
  first && first[1],
  { from: '/from', to: '/to', confirmExisting: false, previousPostBusySessionIds: [] },
  'the first attempt feeds an empty carryover list',
);

await bridge.rebindWorkspaceRoot('/from', '/to', true, ['s1', 's2']);
const retry = invokes.filter(([command]) => command === 'rebind_workspace_root').pop();
assert.deepEqual(
  retry && retry[1],
  { from: '/from', to: '/to', confirmExisting: true, previousPostBusySessionIds: ['s1', 's2'] },
  'the retry feeds the previous report post-busy ids back',
);

// 2. main.jsx wiring: the dialog state keeps the ids and confirmRebindWorkspace
//    passes them to the bridge.
const mainSource = read('app/main.jsx');
assert.match(
  mainSource,
  /postBusyIds: \(report && report\.post_busy_session_ids\) \|\| \[\]/,
  'the partial draft must keep the post-busy ids for the next retry',
);
assert.match(
  mainSource,
  /previousPostBusySessionIds = \(rebindDraft\.partial && rebindDraft\.partial\.postBusyIds\) \|\| \[\]/,
  'the retry must read the carryover ids from the draft',
);
assert.match(
  mainSource,
  /rebindDraft\.from, rebindDraft\.to, confirmExisting, previousPostBusySessionIds\)/,
  'the retry must pass the carryover ids to the bridge',
);

// 3. Round-15 Major 1: the roots-error carryover UNIONS with the previous
//    post-busy list instead of replacing it. The three-run shape: run 1
//    reports x post-busy; run 2 feeds [x] back but fails at the roots commit
//    with suffix ids [y]; if the merge replaced, run 3 would feed only [y]
//    and — x being a converged to-lane session that can never re-enter
//    rebound_session_ids — close "up to date" while x's old-cwd runtime
//    stays resident. Union + dedupe keeps both; the backend narrows by
//    intersection, so the union cannot widen the eviction set.
{
  const { mergeRebindCarryoverIds } = await import('../src/features/projects/rebindErrors.js');
  assert.deepEqual(
    mergeRebindCarryoverIds(['x'], ['y']),
    ['x', 'y'],
    'the error carryover must union with the previous post-busy list',
  );
  assert.deepEqual(mergeRebindCarryoverIds(['x'], ['x']), ['x'], 'dedupe');
  assert.deepEqual(mergeRebindCarryoverIds(undefined, ['y']), ['y'], 'no previous list');
  // The dialog must route all three error arms through the union helper.
  assert.match(
    mainSource,
    /mergeRebindCarryoverIds\(/,
    'main.jsx must build the error carryover via the union helper',
  );
  assert.match(
    mainSource,
    /mergeRebindCarryoverIds\([\s\S]{0,120}prev\.partial\.postBusyIds[\s\S]{0,80}classified\.reboundIds/,
    'the union must merge the previous carryover with the error suffix ids',
  );
}

console.log('rebind carryover feed-back contract passed');
