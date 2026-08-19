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
  // 仅供撕离窗(DetachedShell)按需加载;主窗口 ChatView 启动即渲染,在 main.jsx 静态 import
  chat: () => import('../features/chat/ChatView.jsx'),
};
