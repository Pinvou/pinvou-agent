// 最近工作区列表：code 模式（CodexAcpView）与普通聊天模式（ChatView 草稿态
// 工作区选择器）共用；storage key 保持不变，两个模式共享同一份最近列表。
//
// 注意：platform/tauri/bridge/sessions.js 是 <script src> 加载的经典脚本，
// 无法 import 本模块；pickDraftWorkspace 内的 remember 逻辑是本文件的逐字
// 镜像（同 key 同语义），改动任一侧必须同步另一侧——
// tests/chat_draft_workspace_logic.test.mjs 会锁定桥侧行为。
import { pathBasename } from './path-utils.js';

export const RECENT_WORKSPACES_KEY = 'pinvou_codex_recent_workspaces';

export function workspaceName(path, unknownDirectory) {
  // 尾分隔符剥离与 Windows 盘符语义统一走 shared/path-utils（评审 #445：
  // 本文件的 regex 版与 CodexAcpView 的 pathBasename 版曾分叉）。
  return pathBasename(path, { collapseTrailing: true, fallback: unknownDirectory });
}

export function loadRecentWorkspaces() {
  try {
    const value = JSON.parse(localStorage.getItem(RECENT_WORKSPACES_KEY) || '[]');
    return Array.isArray(value) ? value.filter(path => typeof path === 'string').slice(0, 6) : [];
  } catch {
    return [];
  }
}

export function rememberWorkspace(path) {
  const next = [path, ...loadRecentWorkspaces().filter(item => item !== path)].slice(0, 6);
  localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  return next;
}

export function forgetWorkspace(path) {
  const next = loadRecentWorkspaces().filter(item => item !== path);
  try {
    localStorage.setItem(RECENT_WORKSPACES_KEY, JSON.stringify(next));
  } catch {
    // localStorage 不可用时仍允许当前窗口继续创建新会话。
  }
  return next;
}
