/**
 * 打开子智能体只读面板的唯一事件契约(`pinvou:open-subagent`)。
 *
 * 产出方:RunningAgentsOverlay 的列表行,以及 swarm spawn 计数行
 * (features/tools/tool-renderers.jsx)。
 * 消费方:主窗口把该事件路由到 SubagentTranscriptPanel。
 */

/** Request opening the read-only subagent transcript panel. */
export function dispatchOpenSubagent(agentId, sessionId) {
  if (typeof window === 'undefined' || typeof window.dispatchEvent !== 'function') return;
  // agentId === null is a valid request: open the panel's list state (the
  // swarm count row's entry point).
  if (!agentId && agentId !== null) return;
  window.dispatchEvent(new CustomEvent('pinvou:open-subagent', {
    detail: { agentId, sessionId: sessionId || null },
  }));
}
