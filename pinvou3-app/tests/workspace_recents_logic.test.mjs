// Logic tests for shared/workspace-recents.js: the recent-workspaces storage
// extracted verbatim from CodexAcpView. Code mode and the regular-chat draft
// share one localStorage list (key pinvou_codex_recent_workspaces); behavior
// must stay identical to before extraction.
import assert from 'node:assert/strict';
import test from 'node:test';

// In-memory localStorage stub (the module reads global localStorage at call time, so install it before import).
const storage = new Map();
globalThis.localStorage = {
  getItem(key) { return storage.has(key) ? storage.get(key) : null; },
  setItem(key, value) { storage.set(key, String(value)); },
  removeItem(key) { storage.delete(key); },
};

const {
  RECENT_WORKSPACES_KEY,
  forgetWorkspace,
  loadRecentWorkspaces,
  rememberWorkspace,
  workspaceName,
} = await import('../src/shared/workspace-recents.js');

test.beforeEach(() => storage.clear());

test('storage key 保持 pinvou_codex_recent_workspaces（两模式共享同一份列表）', () => {
  assert.equal(RECENT_WORKSPACES_KEY, 'pinvou_codex_recent_workspaces');
});

test('loadRecentWorkspaces：空 / 损坏 JSON / 非数组 / 非字符串条目均容错', () => {
  assert.deepEqual(loadRecentWorkspaces(), []);
  storage.set(RECENT_WORKSPACES_KEY, '{broken json');
  assert.deepEqual(loadRecentWorkspaces(), []);
  storage.set(RECENT_WORKSPACES_KEY, '{"not":"array"}');
  assert.deepEqual(loadRecentWorkspaces(), []);
  storage.set(RECENT_WORKSPACES_KEY, JSON.stringify(['/a', 42, null, '/b', {}]));
  assert.deepEqual(loadRecentWorkspaces(), ['/a', '/b']);
});

test('loadRecentWorkspaces：最多返回 6 条（截断历史脏数据）', () => {
  storage.set(RECENT_WORKSPACES_KEY, JSON.stringify(['/1', '/2', '/3', '/4', '/5', '/6', '/7', '/8']));
  assert.deepEqual(loadRecentWorkspaces(), ['/1', '/2', '/3', '/4', '/5', '/6']);
});

test('rememberWorkspace：新条目置顶并写回 localStorage', () => {
  rememberWorkspace('/a');
  const next = rememberWorkspace('/b');
  assert.deepEqual(next, ['/b', '/a']);
  assert.deepEqual(JSON.parse(storage.get(RECENT_WORKSPACES_KEY)), ['/b', '/a']);
});

test('rememberWorkspace：重复选择去重并置顶', () => {
  rememberWorkspace('/a');
  rememberWorkspace('/b');
  rememberWorkspace('/c');
  const next = rememberWorkspace('/b');
  assert.deepEqual(next, ['/b', '/c', '/a']);
});

test('rememberWorkspace：6 条上限，最旧的被淘汰', () => {
  for (const path of ['/1', '/2', '/3', '/4', '/5', '/6']) rememberWorkspace(path);
  const next = rememberWorkspace('/7');
  assert.deepEqual(next, ['/7', '/6', '/5', '/4', '/3', '/2']);
  assert.equal(JSON.parse(storage.get(RECENT_WORKSPACES_KEY)).length, 6);
});

test('forgetWorkspace：移除指定目录，其余保留顺序', () => {
  rememberWorkspace('/a');
  rememberWorkspace('/b');
  rememberWorkspace('/c');
  const next = forgetWorkspace('/b');
  assert.deepEqual(next, ['/c', '/a']);
  assert.deepEqual(JSON.parse(storage.get(RECENT_WORKSPACES_KEY)), ['/c', '/a']);
  // Removing a non-existent entry is a no-op.
  assert.deepEqual(forgetWorkspace('/nope'), ['/c', '/a']);
});

test('workspaceName：取末段目录名，剥离尾部分隔符，兼容两种分隔符', () => {
  assert.equal(workspaceName('/home/user/project', '?'), 'project');
  assert.equal(workspaceName('/home/user/project/', '?'), 'project');
  assert.equal(workspaceName('C:\\work\\repo\\', '?'), 'repo');
  assert.equal(workspaceName('C:\\work\\repo', '?'), 'repo');
  assert.equal(workspaceName('/', '?'), '?');
  assert.equal(workspaceName('', '?'), '?');
  assert.equal(workspaceName(null, '?'), '?');
  // A single-segment path is returned as-is.
  assert.equal(workspaceName('project', '?'), 'project');
});
