// IME(输入法)合成状态守卫:阻止"一次回车既上屏候选词又触发业务动作"。
//
// 单独依赖 isComposing 在 macOS WKWebView 上不可靠:WebKit 历史缺陷(bug 165004,
// 直到 2026-04 才进入主线修复,且无对应发布版本号)会把确认 IME 候选词的 Enter
// keydown 延迟到 compositionend 之后派发,此时 isComposing 已为 false。但 W3C
// UI Events 规范要求 IME 处理期间的 keydown 其 keyCode 为 229,且该标记在上述
// WebKit 场景下仍然保留——这是唯一可靠的兜底信号,与 React/Slack/Figma 等业界
// 通行做法一致。项目最低支持 macOS 11,不能假定运行环境已含上述修复。
//
// 同时兼容 React 合成事件(含 nativeEvent)与原生 DOM 事件(如隔离 iframe 内
// design-runtime 派发的事件):优先取 nativeEvent,缺失时回退到事件本身。

/**
 * 判断键盘事件是否处于 IME 合成状态。
 * @param {KeyboardEvent & { nativeEvent?: KeyboardEvent }} event - React 合成事件或原生 DOM 事件。
 * @returns {boolean} 处于合成中返回 true(此时回车仅用于 IME 提交,不应触发业务动作)。
 */
export function isImeComposing(event) {
  const ne = event?.nativeEvent ?? event;
  return Boolean(ne?.isComposing) || ne?.keyCode === 229;
}
