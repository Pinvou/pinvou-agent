// 工作区面板目录树的纯逻辑（可单测）。
//
// isMissingWorkspaceDirectoryError：判定「目录已从磁盘消失」类错误。匹配自家
// 后端的固定文案——features/codex_acp/workspace.rs 在 fs::canonicalize 失败时
// 以 `工作区路径不存在: <path>` 报错（with_context，Tauri invoke 把完整错误链
// 字符串化后 reject）。这是后端约定文案的按现状子串匹配，后端改文案时需同步；
// 匹配不到时退化为普通 showError，不会误判吞错。
//
// pruneMissingDirectory：把消失的目录（回退撤销 agent 创建的目录、agent 回合中
// 删目录、用户外部删除都会触发）从「展开集合 + 条目缓存」中逐出，含其全部子
// 路径（relativePath 恒为 '/' 分隔，见 workspace.rs normalize_relative_path）。
// 返回新对象不原地修改；树随状态更新自然折叠到仍存在的祖先。

export function isMissingWorkspaceDirectoryError(error) {
  return String(error && error.message ? error.message : error).includes('工作区路径不存在');
}

export function pruneMissingDirectory(expanded, entriesByDirectory, missingPath) {
  const missing = String(missingPath || '');
  const isGone = path => Boolean(
    missing
      && typeof path === 'string'
      && path
      && (path === missing || path.startsWith(`${missing}/`)),
  );
  const nextExpanded = new Set();
  for (const path of expanded || []) {
    if (!isGone(path)) nextExpanded.add(path);
  }
  const nextEntries = {};
  for (const [directory, entries] of Object.entries(entriesByDirectory || {})) {
    if (!isGone(directory)) nextEntries[directory] = entries;
  }
  return { expanded: nextExpanded, entriesByDirectory: nextEntries };
}
