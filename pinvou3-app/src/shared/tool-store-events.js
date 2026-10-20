/**
 * tool-store-events.js — 工具/技能跨层 DOM 信号（FFE4-1）。
 *
 * 事件名是跨层契约：composer-shared.jsx 模块级监听 pinvou:chat-round-committed
 * 转正 pending enables；工具页监听 pinvou:tools-changed 刷新计数。此前
 * features/tools/tool-events.js 有一份实现，platform 桥接层（classic 脚本、
 * 依赖方向不许 import features/）另有 8 处逐字内联副本。现实现只在此处一份：
 *   - platform 桥接层经 window.PinvouToolStoreEvents 调用（本文件必须先于两条
 *     bridge 以普通脚本加载，见 index.html）；
 *   - feature 侧由 features/tools/tool-events.js 重导出，消费方不变。
 * 本文件以普通脚本加载（非 ES module），与 bridge-shared-helpers.js 同一模式。
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim classic-script artifact; strict mode is part of the payload
  "use strict";

  const notifyComposerToolsChanged = () => {
    try { root.dispatchEvent(new CustomEvent('pinvou:tools-changed')); } catch { /* silently ignored */ }
  };

  // 新一轮对话已提交（后端受理）：会话中「打开」的工具/技能自此进入上下文，
  // 未提交的「打开」（pending enable）转正并按「只增不减」锁死。scope 为
  // 'plain'（缺省，普通会话）或 'code'（原生代码会话），两条车道各自提交各自的。
  const notifyChatRoundCommitted = (scope) => {
    try {
      root.dispatchEvent(new CustomEvent('pinvou:chat-round-committed', {
        detail: { scope: scope === 'code' ? 'code' : 'plain' },
      }));
    } catch { /* silently ignored */ }
  };

  root.PinvouToolStoreEvents = Object.freeze({ notifyComposerToolsChanged, notifyChatRoundCommitted });
})(window);
