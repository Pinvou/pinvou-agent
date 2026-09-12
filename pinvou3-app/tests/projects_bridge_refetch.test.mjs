// Regression harness for the projects bridge feature (finding 27, locks the
// finding-14 fix): a projects:list_changed event landing while a fetch is in
// flight must schedule exactly one bounded catch-up refetch — the event must
// not be swallowed (sidebar stuck on a stale snapshot) and must not storm.
import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const source = fs.readFileSync(path.join(root, 'src/platform/tauri/bridge/projects.js'), 'utf8');

function setup() {
  const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(source, { window: windowObject, console });
  const state = {};
  const listeners = {};
  // Only list_projects is queued for manual resolution; mutations resolve
  // immediately so the test can focus on fetch scheduling.
  const pendingLists = [];
  let listCalls = 0;
  const api = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__.projects({
    state,
    notify() {},
    invoke(command) {
      if (command !== 'list_projects') return Promise.resolve({ id: 'p1', name: 'Alpha', roots: [] });
      listCalls += 1;
      return new Promise((resolve) => { pendingLists.push(resolve); });
    },
    listen(event, callback) { listeners[event] = callback; },
  });
  return {
    api,
    state,
    listeners,
    pendingLists,
    listCalls: () => listCalls,
  };
}

const tick = () => new Promise((resolve) => { setTimeout(resolve, 0); });

test('list_changed mid-fetch triggers one catch-up refetch', async () => {
  const { api, state, listeners, pendingLists, listCalls } = setup();

  const first = api.loadProjects();
  assert.equal(pendingLists.length, 1, 'first fetch in flight');

  // The backend broadcasts a change while the first fetch is still resolving.
  listeners['projects:list_changed']();
  assert.equal(pendingLists.length, 1, 'event coalesces instead of double-fetching');

  pendingLists.shift()({ projects: [{ id: 'p1', name: 'Alpha', roots: [] }], assignments: {} });
  await first;
  assert.equal(pendingLists.length, 1, 'mid-flight event must schedule a catch-up fetch');

  pendingLists.shift()({
    projects: [{ id: 'p1', name: 'Alpha', roots: [] }, { id: 'p2', name: 'Beta', roots: [] }],
    assignments: {},
  });
  await tick();
  assert.equal(state.projectsList.projects.length, 2, 'catch-up snapshot replaces the stale one');
  await tick();
  assert.equal(listCalls(), 2, 'catch-up is bounded: no further fetch without a new event');
});

test('event during the post-mutation refetch is not swallowed', async () => {
  const { api, state, listeners, pendingLists } = setup();

  // Bootstrap fetch resolves first.
  const boot = api.loadProjects();
  pendingLists.shift()({ projects: [], assignments: {} });
  await boot;

  const created = api.createProject('Alpha', []);
  await tick(); // create_project resolved; post-mutation loadProjects started
  assert.equal(pendingLists.length, 1, 'post-mutation refetch in flight');

  listeners['projects:list_changed']();
  pendingLists.shift()({ projects: [{ id: 'p1', name: 'Alpha', roots: [] }], assignments: {} });
  await created;
  assert.equal(pendingLists.length, 1, 'mid-flight event schedules a catch-up fetch');

  pendingLists.shift()({ projects: [{ id: 'p1', name: 'Alpha', roots: [] }], assignments: {} });
  await tick();
  assert.equal(state.projectsList.projects.length, 1);
  assert.equal(state.projectsList.projects[0].name, 'Alpha');
});
