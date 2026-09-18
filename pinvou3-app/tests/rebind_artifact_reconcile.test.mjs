// Frontend half of the artifact-path rebase contract (review #463 round-10
// Major 2): after a folder rebind the persisted artifacts[].storage_path
// entries keep the vanished absolute root, and the switch-session reconcile
// pass must rebase a stale absolute entry onto the same-basename file the
// workspace scan surfaces — the relative→absolute escape hatch never fires
// for an entry that is already absolute. The backend half (the rebase during
// the rebind metadata pass) is pinned in features/sessions/tests.rs.
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
//    workspace file with the same basename, and the rebased list is persisted.
{
  const state = {
    activeSessionId: 's1',
    artifacts: [{ path: '/old/root/sub/report.html', basename: 'report.html' }],
  };
  const { tracker, invokes } = makeTracker(state, ['/new/root/sub/report.html']);
  await tracker.reconcileArtifacts('s1');
  assert.deepEqual(
    state.artifacts.map(a => a.path),
    ['/new/root/sub/report.html'],
    'the stale absolute entry must follow the moved root',
  );
  const saved = invokes.find(([command]) => command === 'save_session_artifacts');
  assert.ok(saved, 'the rebased list must be persisted');
  assert.deepEqual(saved[1].paths, ['/new/root/sub/report.html']);
}

// 2. An entry whose absolute path the scan reproduces verbatim stays put (no
//    false rebase of a live workspace file onto itself or a sibling), and a
//    scanned file with no tracked entry is added as before.
{
  const state = {
    activeSessionId: 's2',
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
