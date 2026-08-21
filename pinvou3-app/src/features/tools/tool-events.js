const notifyComposerToolsChanged = () => {
  try { window.dispatchEvent(new CustomEvent('pinvou:tools-changed')); } catch (_) {}
};

// 新一轮对话已提交（后端受理）：会话中「打开」的工具/技能自此进入上下文，
// 未提交的「打开」（pending enable）转正并按「只增不减」锁死。scope 为
// 'plain'（缺省，普通会话）或 'code'（原生代码会话），两条车道各自提交各自的。
const notifyChatRoundCommitted = (scope) => {
  try {
    window.dispatchEvent(new CustomEvent('pinvou:chat-round-committed', {
      detail: { scope: scope === 'code' ? 'code' : 'plain' },
    }));
  } catch (_) {}
};

export { notifyComposerToolsChanged, notifyChatRoundCommitted };
