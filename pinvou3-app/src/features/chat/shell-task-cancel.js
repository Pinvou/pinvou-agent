import { useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';

// 取消后台 shell 任务的共享状态机：会话里的 shell ToolCard（features/tools/
// tool-renderers.jsx）与后台任务指示器（ChatView 的浮层行）走同一个
// bridge.chat.cancelShellTask 调用、同一套 cancelling 防抖与 i18n 错误文案，
// 抽在这里避免两处各写一份。
export function useShellTaskCancel(t) {
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState('');
  const cancel = async (sessionId, taskId) => {
    if (!taskId || cancelling) return;
    setCancelling(true);
    setCancelError('');
    try {
      await bridge.chat.cancelShellTask(sessionId, taskId);
    } catch (error) {
      console.warn('cancel shell task failed', error);
      setCancelError(`${t.shellCancelFailed || t.toolFailed}: ${String(error)}`);
    } finally {
      setCancelling(false);
    }
  };
  return { cancelling, cancelError, cancel };
}
