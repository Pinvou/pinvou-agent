// 欢迎卡的 pre-send opt-in（评审 #455 R8-2 / R9 覆盖注记）：安装路径刻意
// 保持开关关闭（DenyAll 收敛），欢迎卡展示期间的首次发送——无论点击示例
// 提问还是自由输入——都必须先把该包移出 plain 禁用集，模型才能收到工具。
// 提取为纯模块以便像 scene-capabilities 一样做 node 直测（项目暂无 React
// 测试设施）。失败不阻断发送（fail-visible：调用方按 failed 展示提示，
// 工具缺席在回复中可见），不静默吞错。

async function consumeWelcomeOptIn({ getToolId, consume, invoke }) {
  const toolId = getToolId && getToolId();
  if (!toolId) return { attempted: false };
  // 一次性消费：无论 enable 成败，同一张欢迎卡只 opt-in 一次（失败重试
  // 由用户在工具列表显式完成，不在发送路径反复打点）。
  if (consume) consume();
  try {
    await invoke('enable_marketplace_packages', { packageIds: [toolId], scope: 'plain' });
    return { attempted: true, failed: false };
  } catch (error) {
    return {
      attempted: true,
      failed: true,
      error: String((error && error.message) || error || ''),
    };
  }
}

export { consumeWelcomeOptIn };
