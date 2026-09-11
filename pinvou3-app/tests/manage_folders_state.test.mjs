// 第二批纯函数测试:钥匙串 chip 形态(describeKeychain)与管理文件夹面板
// 状态解析(manageFolderRows/removeRootPlan/rootAlreadyPresent,§4/§9.5)。
import test from 'node:test';
import assert from 'node:assert/strict';

import { describeKeychain } from '../src/features/projects/workspacePickerState.js';
import { manageFolderRows, removeRootPlan, rootAlreadyPresent } from '../src/features/projects/manageFoldersState.js';

const project = (id, roots, extra = {}) => ({
  id,
  name: id,
  roots,
  position: 0,
  ...extra,
});

test('describeKeychain: first root is primary, rest are additional', () => {
  assert.deepEqual(describeKeychain(['/a', '/b', '/c']), { primary: '/a', additional: 2, roots: ['/a', '/b', '/c'] });
  assert.deepEqual(describeKeychain(['/a']), { primary: '/a', additional: 0, roots: ['/a'] });
  assert.deepEqual(describeKeychain([]), { primary: null, additional: 0, roots: [] });
  assert.deepEqual(describeKeychain(null), { primary: null, additional: 0, roots: [] });
});

test('manageFolderRows: primary = remembered root, else first; availability kept', () => {
  const rows = manageFolderRows(project('p1', [
    { path: '/a', available: true },
    { path: '/b', available: false },
  ], { last_primary_root: '/b' }));
  assert.deepEqual(rows, [
    { path: '/a', available: true, isPrimary: false },
    { path: '/b', available: false, isPrimary: true },
  ]);
  // 记忆失效(已不在 roots)→ 第一位兜底。
  const fallback = manageFolderRows(project('p2', [{ path: '/a', available: true }], { last_primary_root: '/gone' }));
  assert.equal(fallback[0].isPrimary, true);
  assert.deepEqual(manageFolderRows(project('p3', [])), [], '纯标签项目空行');
});

test('removeRootPlan: primary removal blocked while siblings remain', () => {
  const p = project('p1', [{ path: '/a', available: true }, { path: '/b', available: true }]);
  const blocked = removeRootPlan(p, '/a');
  assert.equal(blocked.removed, true);
  assert.equal(blocked.needsNewPrimary, true, '主根且有余根 → 先另选主根');
  assert.deepEqual(blocked.roots, ['/b']);
  const ok = removeRootPlan(p, '/b');
  assert.equal(ok.needsNewPrimary, false);
  assert.deepEqual(ok.roots, ['/a']);
  // 唯一根移除 → 降级纯标签项目,允许。
  const single = removeRootPlan(project('p2', [{ path: '/a', available: true }]), '/a');
  assert.equal(single.becomesTagOnly, true);
  assert.deepEqual(single.roots, []);
  // 不存在/重复路径不动。
  assert.equal(removeRootPlan(p, '/nope').removed, false);
});

test('rootAlreadyPresent dedupes exact paths only', () => {
  const p = project('p1', [{ path: '/a', available: true }]);
  assert.equal(rootAlreadyPresent(p, '/a'), true);
  assert.equal(rootAlreadyPresent(p, '/a/sub'), false, '嵌套合法(§9.9),不算重复');
  assert.equal(rootAlreadyPresent(p, ''), false);
});
