import { useEffect, useState } from 'react';

const bridge = window.TauriBridge || { available: false };

    function useBridge() {
      const [bs, setBs] = useState(() => bridge.available ? bridge.getState() : null);
      useEffect(() => {
        if (!bridge.available) return;
        if (typeof bridge.init === 'function') bridge.init().catch(e => console.warn('[TauriBridge] init failed', e));
        return bridge.subscribe(setBs);
      }, []);
      return bs;
    }

    /* ==========================================
       自定义标题栏（无边框窗口）—— 最小化 / 最大化 / 关闭
       ========================================== */

export { bridge, useBridge };
