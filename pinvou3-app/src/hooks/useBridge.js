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

// 判定单个模型是否本地推理后端（local_vllm 预设，或 base_url 指向 127.0.0.1/localhost）。
// 集中放这里，供加载提示文案按「本地 / 在线」分流，避免在线模型时误显「本地模型生成中」。
function isLocalModel(model) {
  return !!(model && (model.preset === 'local_vllm' || /127\.0\.0\.1|localhost/i.test(model.base_url || '')));
}

// 当前激活模型是否本地推理后端；拿不到模型信息时默认 false（按在线口径显示，绝不误称本地）。
function activeModelIsLocal(bs) {
  if (!bs || !Array.isArray(bs.savedModels) || !bs.activeModelId) return false;
  const m = bs.savedModels.find(x => x && x.id === bs.activeModelId);
  return isLocalModel(m);
}

export { bridge, useBridge, isLocalModel, activeModelIsLocal };
