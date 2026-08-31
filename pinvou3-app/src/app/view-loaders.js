// 低频视图懒加载:用户典型路径是聊天,Settings/Codex/卡池/工具商店/定时/知识库/
// 监控/搜索视图切到才加载对应 chunk(rolldown 对动态 import 自动分割)。VIEW_LOADERS
// 是唯一的动态 import 出口:main.jsx 的 React.lazy 与 NavItem 悬停/聚焦预取、
// DetachedShell 的撕离窗视图共用同一工厂,保证命中同一模块缓存。
// 本模块不得静态引入任何视图,否则对应视图会被钉回主 chunk。
export const VIEW_LOADERS = {
  settings: () => import('../features/settings/SettingsView.jsx'),
  codex: () => import('../features/codex/CodexAcpView.jsx'),
  cardpool: () => import('../features/personas/Personas.jsx'),
  toolStore: () => import('../features/tools/ToolStoreView.jsx'),
  scheduled: () => import('../features/scheduled/ScheduledTasksView.jsx'),
  knowledge: () => import('../features/knowledge/KnowledgeView.jsx'),
  monitor: () => import('../features/monitor/MonitorView.jsx'),
  search: () => import('../features/search/SearchView.jsx'),
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
