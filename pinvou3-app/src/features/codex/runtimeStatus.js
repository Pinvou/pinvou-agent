import { useCallback, useEffect, useRef } from 'react';
import { getAcpAgentStatus, listAcpAgents } from '../../platform/acp/client.js';

export function useAcpAgentStatus(activeAgentIdRef, setStatus) {
  const mountedRef = useRef(true);
  const requestSeqRef = useRef({});
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  const acceptStatus = useCallback((agentId, next, requestId = null) => {
    const sequence = requestId ?? (Number(requestSeqRef.current[agentId] || 0) + 1);
    if (requestId === null) requestSeqRef.current[agentId] = sequence;
    if (requestSeqRef.current[agentId] !== sequence) return false;
    if (next?.agent_id !== agentId || agentId !== activeAgentIdRef.current) return false;
    if (!mountedRef.current) return false;
    setStatus(next);
    return true;
  }, [activeAgentIdRef, setStatus]);
  const refreshStatus = useCallback(async (agentId, recheck = false) => {
    const requestId = Number(requestSeqRef.current[agentId] || 0) + 1;
    requestSeqRef.current[agentId] = requestId;
    const next = await getAcpAgentStatus(agentId, recheck);
    acceptStatus(agentId, next, requestId);
    return next;
  }, [acceptStatus]);
  return { acceptStatus, refreshStatus };
}

export async function refreshAcpAgentCatalog(setAgents, transform = list => list) {
  try {
    const next = await listAcpAgents();
    const list = transform(next || []);
    setAgents(list);
    return list;
  } catch (error) {
    // 首次失败时结束骨架屏并回退到当前 Agent；重连失败则保留旧目录。
    setAgents(current => current ?? transform([]));
    throw error;
  }
}

// 定时任务严格串行；stop 会等待在途任务结束，调用方随后可安全读取最终状态。
export function startSerialStatusPolling(task, initialDelay = 500, interval = 750) {
  let timer = null;
  let running = true;
  let inFlight = Promise.resolve();
  const poll = async () => {
    inFlight = Promise.resolve().then(task).catch(() => {});
    await inFlight;
    if (running) timer = window.setTimeout(poll, interval);
  };
  timer = window.setTimeout(poll, initialDelay);
  return async () => {
    running = false;
    if (timer !== null) window.clearTimeout(timer);
    await inFlight;
  };
}
