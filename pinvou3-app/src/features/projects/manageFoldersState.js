// 「管理文件夹」面板(§4)的纯逻辑:roots 行形态、移除判定、主根解析。
// 无 UI/i18n 依赖,node 侧可单测。

function rootPathOf(root) {
  return String((root && typeof root === 'object' ? root.path : root) || '');
}

// 面板行:路径 + 可用性徽标 + 主根标记。主根 = last_primary_root(仍是
// roots 成员才采纳)否则 roots 第一位(与 pickerPrimaryRoot 同口径)。
export function manageFolderRows(project) {
  const roots = (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => ({
      path: rootPathOf(root),
      available: !!(root && typeof root === 'object' ? root.available : true),
    }))
    .filter(row => row.path);
  if (!roots.length) return [];
  const remembered = project && project.last_primary_root ? String(project.last_primary_root) : '';
  const primaryIndex = remembered && roots.some(row => row.path === remembered)
    ? roots.findIndex(row => row.path === remembered)
    : 0;
  return roots.map((row, index) => ({ ...row, isPrimary: index === primaryIndex }));
}

// 移除判定(§4/§9.5):移除主根且仍有其它根时必须先另选主根(降级提示,
// 不替用户挑);唯一根移除 → 项目降级为纯标签(roots 空),允许;非主根
// 直接可移除。重复/不存在的路径返回 removed=false。
export function removeRootPlan(project, path) {
  const rows = manageFolderRows(project);
  const target = rows.find(row => row.path === path);
  if (!target) return { removed: false, roots: rows.map(row => row.path), needsNewPrimary: false, becomesTagOnly: false };
  const remaining = rows.filter(row => row.path !== path);
  return {
    removed: true,
    roots: remaining.map(row => row.path),
    needsNewPrimary: target.isPrimary && remaining.length > 0,
    becomesTagOnly: remaining.length === 0,
  };
}

// 添加判重:已覆盖(精确同路径)时无需再添加;嵌套/重叠合法(§9.9),只挡
// 纯重复(同路径重复添加会让 update_project 的组内去重校验报错——提前拦住)。
export function rootAlreadyPresent(project, path) {
  const target = String(path || '');
  if (!target) return false;
  return manageFolderRows(project).some(row => row.path === target);
}
