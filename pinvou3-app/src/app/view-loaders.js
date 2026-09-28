// Lazy loading for low-frequency views and overlays: the typical user path is
// chat, so the Settings / Codex / card pool / tool store / scheduled /
// knowledge / monitor / search views, the search overlay, the project dialogs
// and the rare global dialogs load only when used (rolldown splits dynamic
// imports automatically). VIEW_LOADERS is the single dynamic-import exit:
// main.jsx's React.lazy and the hover / focus / pre-open prefetches share the
// same factories so they hit the same module cache, and DetachedShell reuses
// the view factories for torn-off windows. This module must not statically
// import any of these components, or they would be pinned back into the main
// chunk.
export const VIEW_LOADERS = {
  settings: () => import('../features/settings/SettingsView.jsx'),
  codex: () => import('../features/codex/CodexAcpView.jsx'),
  cardpool: () => import('../features/personas/Personas.jsx'),
  toolStore: () => import('../features/tools/ToolStoreView.jsx'),
  scheduled: () => import('../features/scheduled/ScheduledTasksView.jsx'),
  knowledge: () => import('../features/knowledge/KnowledgeView.jsx'),
  monitor: () => import('../features/monitor/MonitorView.jsx'),
  search: () => import('../features/search/SearchView.jsx'),
  searchOverlay: () => import('../features/search/SearchOverlay.jsx'),
  moveToProjectDialog: () => import('../features/projects/MoveToProjectDialog.jsx'),
  rebindFolderDialog: () => import('../features/projects/RebindFolderDialog.jsx'),
  pinvouSummon: () => import('../features/tools/PinvouSummonCard.jsx'),
  updateNotice: () => import('../features/updater/UpdateNoticeButton.jsx'),
  savedPersonaConfirmDialog: () => import('../features/personas/SavedPersonaConfirmDialog.jsx'),
  apiKeyGateDialog: () => import('../features/settings/ApiKeyGateDialog.jsx'),
  archiveConfirmDialog: () => import('../features/sessions/ArchiveConfirmDialog.jsx'),
  // chat is not here: the main window renders ChatView at startup and imports
  // it statically in main.jsx; a dynamic import would not produce a separate
  // chunk (rolldown reports INEFFECTIVE_DYNAMIC_IMPORT). A detached window
  // (DetachedShell) loads the same index.html as the main window (the main
  // chunk is necessarily already loaded), so it statically imports ChatView
  // directly and does not go through this table.
};

// 预取专用包装:挂 catch 吞掉加载失败(预取失败无害——真实切视图时 React.lazy
// 重新发起 import 会重试),消除悬停/空闲预取产生的 unhandledrejection 噪音。
// React.lazy 的工厂不能用这个:错误必须传给 Suspense/ErrorBoundary。
export const prefetchView = (name) => {
  const loader = VIEW_LOADERS[name];
  if (loader) loader().catch(() => {});
};
