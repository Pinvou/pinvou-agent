/** Deliverables index total-failure contract (round-7 review P2): the shared
 * tauriArtifacts cluster must propagate a rejected scan (unreadable sessions
 * root, transport down) as a rejection so the Knowledge panel can render an
 * error over its last successful table — flattening it to
 * { rows: [], skipped: [] } made an unreadable root read as a clean empty
 * index. The success shape (disk rows merged with memory rows, skipped
 * disclosure preserved) must stay intact. */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const sharedSource = fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8');

function loadShared() {
  const root = {};
  vm.runInNewContext(sharedSource, { window: root, globalThis: root });
  return root.PinvouBridgeShared;
}

function artifactsCluster({ invoke, state, hasCommand }) {
  return loadShared().create('tauriArtifacts', {
    invoke,
    addSystemItem() {},
    bt(key) { return key; },
    state,
    sessionStates: { value: {} },
    isDeliverable(p) { return /\.(html?|pdf|pptx?|docx?|xlsx?|png|jpe?g)$/i.test(String(p)); },
    basename(p) { return String(p).split(/[\\/]/).pop(); },
    artifactInfo() { return Promise.resolve({ modified: '2026-10-19T00:00:00Z', size: 10 }); },
    dialogOpen: null,
    addAttachmentByPath() {},
    hasCommand: hasCommand || (() => true),
  });
}

test('a rejected deliverables scan propagates instead of resolving to an empty index', async () => {
  const cluster = artifactsCluster({
    invoke() { return Promise.reject(new Error('cannot read the sessions root: EACCES')); },
    state: { sessions: [], activeSessionId: 's1', artifacts: [] },
  });
  await assert.rejects(cluster.listDeliverableIndex(), /sessions root/);
});

test('a successful scan still merges disk and memory rows and preserves skipped', async () => {
  const state = {
    sessions: [{ id: 's1', title: 'One' }],
    activeSessionId: 's1',
    artifacts: [{ path: 'report.html', sessionId: 's1' }],
  };
  const cluster = artifactsCluster({
    invoke(cmd, args) {
      if (cmd === 'list_deliverable_index_v2') {
        return Promise.resolve({ rows: [{ path: '/w/disk.pdf', mtime: 5 }], skipped: ['s-big'] });
      }
      assert.equal(cmd, 'list_workspace_files');
      assert.equal(cmd, 'list_workspace_files');
      assert.equal(args.sessionId, 's1');
      return Promise.resolve(['/w/report.html']);
    },
    state,
  });
  const res = await cluster.listDeliverableIndex();
  // res.rows / res.skipped are vm-realm arrays: rebuild them in this realm
  // before strict compares.
  const paths = Array.from(res.rows, (r) => String(r.path)).sort((a, b) => a.localeCompare(b));
  assert.deepEqual(paths, ['/w/disk.pdf', '/w/report.html']);
  assert.deepEqual(Array.from(res.skipped, String), ['s-big']);
});

// Round-9 review (mixed-version pairings): the web dist updates
// independently of the installed desktop, so a dist that knows v2 can meet
// a desktop whose advertised policy lacks it — the helper must then call
// the legacy command and still merge its rows instead of degrading a bare
// array to an empty index.
test('a legacy array from an old desktop merges as rows, not an empty index', async () => {
  const state = {
    sessions: [{ id: 's1', title: 'One' }],
    activeSessionId: 's1',
    artifacts: [],
  };
  const commands = [];
  const cluster = artifactsCluster({
    hasCommand: () => false,
    invoke(cmd) {
      commands.push(cmd);
      if (cmd === 'list_deliverable_index') {
        return Promise.resolve([{ path: '/w/legacy.pdf', mtime: 5 }]);
      }
      return Promise.resolve([]);
    },
    state,
  });
  const res = await cluster.listDeliverableIndex();
  assert.deepEqual(Array.from(commands), ['list_deliverable_index'],
    'the legacy command is used when the desktop does not advertise v2');
  assert.deepEqual(Array.from(res.rows, (r) => String(r.path)), ['/w/legacy.pdf']);
  assert.deepEqual(Array.from(res.skipped, String), []);
});

// Defense in depth: even when the capability snapshot claims v2, an
// unexpected legacy array answer must merge by rows (and carry no skip
// disclosure) instead of rendering an empty index.
test('an array answer on the v2 path still merges as rows (accept-both)', async () => {
  const state = { sessions: [], activeSessionId: 's1', artifacts: [] };
  const cluster = artifactsCluster({
    hasCommand: () => true,
    invoke(cmd) {
      assert.equal(cmd, 'list_deliverable_index_v2');
      return Promise.resolve([{ path: '/w/stale.pdf', mtime: 5 }]);
    },
    state,
  });
  const res = await cluster.listDeliverableIndex();
  assert.deepEqual(Array.from(res.rows, (r) => String(r.path)), ['/w/stale.pdf']);
  assert.deepEqual(Array.from(res.skipped, String), []);
});
