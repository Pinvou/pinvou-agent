import { useEffect, useState } from 'react';

const bridge = window.TauriBridge || { available: false };

function mergeSlices(domains) {
  if (!bridge.available || !bridge.state) return null;
  return bridge.state.getMany(domains);
}

function useBridgeState(domains) {
  const domainKey = domains.join('|');
  const [bridgeState, setBridgeState] = useState(() => mergeSlices(domains));
  useEffect(() => {
    if (!bridge.available) return undefined;
    bridge.lifecycle.init().catch(e => console.warn('[TauriBridge] init failed', e));
    return bridge.state.subscribeMany(domains, setBridgeState);
  }, [domainKey]);
  return bridgeState;
}

    /* ==========================================
       自定义标题栏（无边框窗口）—— 最小化 / 最大化 / 关闭
       ========================================== */

export { bridge, useBridgeState };
