import assert from 'node:assert/strict';
import {
  isMissingWorkspaceDirectoryError,
  pruneMissingDirectory,
} from '../src/features/codex/workspace-tree.js';

// ── isMissingWorkspaceDirectoryError：后端固定文案匹配 ───────────────
// 与 features/codex_acp/workspace.rs 的「工作区路径不存在: <path>」对齐
// （Tauri invoke 把 with_context 错误链字符串化后 reject，前端见完整链）。
{
  const backendError = '读取 Codex 工作区失败: 工作区路径不存在: .luzeyang: 系统找不到指定的文件。 (os error 2)';
  assert.equal(isMissingWorkspaceDirectoryError(backendError), true);
  assert.equal(isMissingWorkspaceDirectoryError(new Error(backendError)), true);
  // 其他错误（权限/IO/工作区整体不可用）不误判。
  assert.equal(isMissingWorkspaceDirectoryError('读取 Codex 工作区失败: 权限不足'), false);
  assert.equal(isMissingWorkspaceDirectoryError(new Error('network error')), false);
  assert.equal(isMissingWorkspaceDirectoryError(null), false);
  assert.equal(isMissingWorkspaceDirectoryError(), false);
  assert.equal(isMissingWorkspaceDirectoryError(''), false);
}

// ── pruneMissingDirectory：逐出消失目录及其子路径 ────────────────────
// 目标目录（含子路径）从展开集合与条目缓存移除，祖先/兄弟/无关路径保留。
{
  const expanded = new Set(['.luzeyang', '.luzeyang/docs', 'src', 'src/features']);
  const entriesByDirectory = {
    '': [{ name: 'root' }],
    '.luzeyang': [{ name: 'a.md' }],
    '.luzeyang/docs': [{ name: 'b.md' }],
    src: [{ name: 'main.ts' }],
  };
  const { expanded: nextExpanded, entriesByDirectory: nextEntries } =
    pruneMissingDirectory(expanded, entriesByDirectory, '.luzeyang');
  assert.deepEqual([...nextExpanded].sort((a, b) => a.localeCompare(b)), ['src', 'src/features']);
  assert.deepEqual(Object.keys(nextEntries).sort((a, b) => a.localeCompare(b)), ['', 'src']);
  // 保留的条目引用不变（不重建数组），根目录（''）不受逐出影响。
  assert.equal(nextEntries[''], entriesByDirectory['']);
  assert.equal(nextEntries.src, entriesByDirectory.src);
}

// 逐出中间目录：子路径一并移除，更深层但非其后代的路径不受影响。
{
  const expanded = new Set(['src', 'src/old', 'src/old/deep', 'src/older', 'src/new']);
  const { expanded: nextExpanded } = pruneMissingDirectory(expanded, {}, 'src/old');
  assert.deepEqual([...nextExpanded].sort((a, b) => a.localeCompare(b)), ['src', 'src/new', 'src/older']);
}

// 边界：missingPath 为空/非法时逐出为空操作；空集合输入安全。
{
  const expanded = new Set(['', 'src']);
  const entries = { '': [], src: [] };
  const unchanged = pruneMissingDirectory(expanded, entries, '');
  assert.deepEqual([...unchanged.expanded].sort((a, b) => a.localeCompare(b)), ['', 'src']);
  assert.deepEqual(Object.keys(unchanged.entriesByDirectory).sort((a, b) => a.localeCompare(b)), ['', 'src']);
  const empty = pruneMissingDirectory(null, undefined, 'src');
  assert.equal(empty.expanded.size, 0);
  assert.deepEqual(empty.entriesByDirectory, {});
}

// 「消失目录从未展开过」场景（仅条目缓存里有）：条目被清，expanded 不变。
{
  const { expanded: nextExpanded, entriesByDirectory: nextEntries } =
    pruneMissingDirectory(new Set(['src']), { gone: [{ name: 'x' }] }, 'gone');
  assert.deepEqual([...nextExpanded], ['src']);
  assert.deepEqual(nextEntries, {});
}

console.log('codex_workspace_tree_logic: all assertions passed');
