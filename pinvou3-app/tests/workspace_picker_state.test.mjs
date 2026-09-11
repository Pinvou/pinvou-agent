// workspacePickerState 纯函数测试:热视图(最近使用排序/冷项目隐藏)、
// 主根解析、分模式告知(§2/§3/§9.3/§9.4)。
import test from 'node:test';
import assert from 'node:assert/strict';

import {
  COLD_PROJECT_IDLE_MS,
  computePickerRows,
  pickerPrimaryRoot,
  pickerProjectRoots,
  projectLastActivity,
  workspaceNoticeTone,
} from '../src/features/projects/workspacePickerState.js';

const project = (id, name, roots, position, extra = {}) => ({
  id,
  name,
  roots: roots.map((path) => ({ path, available: true })),
  position,
  updated_at: '2026-09-01T08:00:00Z',
  ...extra,
});
const item = (id, workspacePath, updatedAt) => ({
  id,
  workspaceKind: 'project',
  workspacePath,
  updatedAt,
});

test('pickerPrimaryRoot prefers remembered root, falls back to first root', () => {
  const p = project('p1', 'A', ['/a', '/b'], 0, { last_primary_root: '/b' });
  assert.equal(pickerPrimaryRoot(p), '/b', '记忆主根优先');
  // 记忆已不在 roots(被移除)→ 回落第一位。
  const stale = project('p2', 'B', ['/a'], 1, { last_primary_root: '/gone' });
  assert.equal(pickerPrimaryRoot(stale), '/a');
  assert.equal(pickerPrimaryRoot(project('p3', 'C', [], 2)), null, '纯标签项目无根');
  assert.equal(pickerPrimaryRoot(null), null);
});

test('pickerProjectRoots unwraps {path} objects and keeps order', () => {
  assert.deepEqual(pickerProjectRoots(project('p1', 'A', ['/a', '/b'], 0)), ['/a', '/b']);
  assert.deepEqual(pickerProjectRoots({ id: 'p2', roots: [] }), []);
});

test('computePickerRows sorts by latest activity descending', () => {
  const projects = [
    project('p1', 'Cold-ish', ['/w/a'], 0),
    project('p2', 'Hot', ['/w/b'], 1),
  ];
  const items = [
    item('s1', '/w/a', '2026-09-01T08:00:00Z'),
    item('s2', '/w/b', '2026-09-10T08:00:00Z'),
  ];
  const rows = computePickerRows({ projects, items, assignments: {}, now: Date.parse('2026-09-10T12:00:00Z') });
  assert.deepEqual(rows.map(r => r.project.id), ['p2', 'p1'], '最近使用排序');
  assert.equal(rows[0].lastActivity, '2026-09-10T08:00:00Z');
});

test('computePickerRows hides cold materialized projects, keeps warm/manual ones', () => {
  const now = Date.parse('2026-09-10T12:00:00Z');
  const old = new Date(now - COLD_PROJECT_IDLE_MS - 86400000).toISOString();
  const projects = [
    // 冷物化项目:30 天无活动、无显式成员 → 隐藏。
    project('cold', 'Cold', ['/w/cold'], 0, { origin: 'folder', updated_at: old }),
    // 同冷但手工项目(非 origin=folder)→ 保留(用户亲手建的)。
    project('manual', 'Manual', ['/w/manual'], 1, { updated_at: old }),
    // 同冷但有显式成员 → 保留(有人主动归属过)。
    project('claimed', 'Claimed', ['/w/claimed'], 2, { origin: 'folder', updated_at: old }),
  ];
  const rows = computePickerRows({
    projects,
    items: [],
    assignments: { s9: 'claimed' },
    now,
  });
  assert.deepEqual(rows.map(r => r.project.id).sort((a, b) => a.localeCompare(b)), ['claimed', 'manual']);
});

test('computePickerRows treats unparseable timestamps as active', () => {
  const projects = [project('p1', 'A', ['/w/a'], 0, { origin: 'folder', updated_at: 'not-a-date' })];
  const rows = computePickerRows({ projects, items: [], assignments: {}, now: Date.now() });
  assert.equal(rows.length, 1, '时间解析失败宁可显示');
});

test('projectLastActivity falls back to project updated_at without members', () => {
  const p = project('p1', 'A', ['/w/a'], 0, { updated_at: '2026-09-05T08:00:00Z' });
  assert.equal(projectLastActivity(p, [], {}), '2026-09-05T08:00:00Z');
  // 显式移出的会话不算成员(tier-① null 条目)。
  const movedOut = item('s1', '/w/a', '2026-09-09T08:00:00Z');
  assert.equal(projectLastActivity(p, [movedOut], { s1: null }), '2026-09-05T08:00:00Z');
});

test('workspaceNoticeTone: plan is restricted wording, yolo is visibility wording', () => {
  assert.equal(workspaceNoticeTone('plan'), 'restricted');
  assert.equal(workspaceNoticeTone('yolo'), 'visibility');
  assert.equal(workspaceNoticeTone(), 'restricted', '未知按更重的受限文案');
  assert.equal(workspaceNoticeTone(null), 'restricted');
});
