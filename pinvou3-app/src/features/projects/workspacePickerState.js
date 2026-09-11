// 「选择工作区」统一选择器的纯逻辑(设计 §2/§3/§9.3/§9.4):热视图计算、
// 主根解析、分模式告知。无 UI/i18n 依赖,node 侧可单测。

import { resolveSessionProjectId } from './projectGrouping.js';

// 冷项目阈值(§3 积累缓解):origin=folder 的物化项目,30 天无活动且无显式
// 成员 → 从选择器(热视图)隐藏;侧栏全量视图不受影响,不删除。
export const COLD_PROJECT_IDLE_MS = 30 * 24 * 60 * 60 * 1000;

function itemTime(item) {
  return String((item && (item.updatedAt || item.pinnedAt)) || '');
}

// 项目的最近活动时间:成员会话(显式归属 + tier-② 自动归组)的最新
// updatedAt,无成员回落项目自身的 updated_at(物化/编辑时间)。
export function projectLastActivity(project, items, assignments) {
  let latest = String((project && project.updated_at) || '');
  (Array.isArray(items) ? items : []).forEach((item) => {
    if (!item) return;
    if (resolveSessionProjectId(item, [project], assignments) !== project.id) return;
    const time = itemTime(item);
    if (time > latest) latest = time;
  });
  return latest;
}

// 选择器行(热视图):按最近使用降序;冷项目(长期无活动或成员已移空的
// 物化项目)隐藏。`now` 注入便于测试。
export function computePickerRows({ projects, items, assignments, now }) {
  const nowMs = typeof now === 'number' ? now : Date.now();
  const assignmentMap = assignments && typeof assignments === 'object' ? assignments : {};
  return (Array.isArray(projects) ? projects : [])
    .filter(Boolean)
    .map((project) => {
      const hasExplicitMembers = Object.values(assignmentMap).includes(project.id);
      return {
        project,
        hasExplicitMembers,
        lastActivity: projectLastActivity(project, items, assignmentMap),
      };
    })
    .filter((row) => {
      if (row.project.origin !== 'folder') return true;
      if (row.hasExplicitMembers) return true;
      const idleMs = nowMs - Date.parse(row.lastActivity || '');
      // 无法解析的时间按活跃处理(宁可显示,不错藏)。
      return Number.isNaN(idleMs) || idleMs <= COLD_PROJECT_IDLE_MS;
    })
    .sort((a, b) => b.lastActivity.localeCompare(a.lastActivity)
      || String(a.project.id).localeCompare(String(b.project.id)));
}

// 项目的默认主根(§9.3 项目通道):记忆 last_primary_root(仍是 roots 成员才
// 采纳,防移除 root 后残留)优先,否则 roots 第一位。返回 null = 纯标签项目
// (无根),调用方不得用它建绑定会话。
export function pickerPrimaryRoot(project) {
  const roots = (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => (root && typeof root === 'object' ? root.path : root))
    .filter(Boolean)
    .map(String);
  if (!roots.length) return null;
  const remembered = project && project.last_primary_root ? String(project.last_primary_root) : '';
  if (remembered && roots.includes(remembered)) return remembered;
  return roots[0];
}

// 项目的全部根(展示形态数组,顺序 = 存储顺序)。
export function pickerProjectRoots(project) {
  return (project && Array.isArray(project.roots) ? project.roots : [])
    .map(root => (root && typeof root === 'object' ? root.path : root))
    .filter(Boolean)
    .map(String);
}

// 分模式权限告知(§9.4):受限模式(Plan/只读) = 授权语义("将可访问 N 个
// 文件夹");YOLO/全权限 = 可见性语义("模型将知道 N 个文件夹与本对话相关",
// YOLO 下本就不受工作区限制)。模式未知按受限文案(更重的告知更安全)。
export function workspaceNoticeTone(mode) {
  return mode === 'yolo' ? 'visibility' : 'restricted';
}
