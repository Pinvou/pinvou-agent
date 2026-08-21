// 侧栏 code 样式:code 会话按文件夹(workspace)分组的纯逻辑。
// 与 date-utils.js 同为纯函数模块,不依赖 UI/i18n,便于 node 侧单测。

// temporary 工作区会话的统一组 key:它们的工作区目录由 session id 推导,
// 按路径分桶会产生大量一次性组,归入一组沉底更符合「临时」语义。
const TEMPORARY_GROUP_KEY = '__temporary__';

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// 输入仅 code 会话:[{ workspacePath, workspaceKind, updatedAt, ... }]。
// 返回 [{ key, rows, latestAt }]:project 会话按 workspacePath 分桶,
// temporary 会话合并为一组;组内按最后活跃(updatedAt)倒序,
// 组间按组内最新活跃倒序,temporary 组恒沉底。
function groupSessionsByFolder(items) {
  const byFolder = new Map();
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    const key = item.workspaceKind === 'project' && item.workspacePath
      ? String(item.workspacePath)
      : TEMPORARY_GROUP_KEY;
    if (!byFolder.has(key)) byFolder.set(key, []);
    byFolder.get(key).push(item);
  });
  const groups = [];
  byFolder.forEach((rows, key) => {
    rows.sort((a, b) => itemTime(b).localeCompare(itemTime(a)));
    groups.push({ key, rows, latestAt: itemTime(rows[0]) });
  });
  groups.sort((a, b) => {
    if (a.key === TEMPORARY_GROUP_KEY) return 1;
    if (b.key === TEMPORARY_GROUP_KEY) return -1;
    return b.latestAt.localeCompare(a.latestAt);
  });
  return groups;
}

export { TEMPORARY_GROUP_KEY, groupSessionsByFolder };
