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

// 只把真正的 loopback URL 视为本地端点，避免正则把
// `https://localhost.example.com` / `http://127.0.0.10.example.com` 误判为本地。
function baseUrlIsLoopback(baseUrl) {
  try {
    const hostname = new URL(baseUrl).hostname.replace(/^\[|\]$/g, '').replace(/\.$/, '').toLowerCase();
    if (hostname === 'localhost' || hostname === '::1') return true;
    const octets = hostname.split('.');
    return octets.length === 4
      && octets.every(part => /^\d+$/.test(part) && Number(part) <= 255)
      && Number(octets[0]) === 127;
  } catch {
    return false;
  }
}

// 判定单个模型是否本地推理后端（local_vllm 预设，或 base_url 指向 loopback）。
// 集中放这里，供加载提示与 API Key gate 共用，避免两套规则漂移。
function isLocalModel(model) {
  return !!(model && (model.preset === 'local_vllm' || baseUrlIsLoopback(model.base_url || '')));
}

// 当前激活模型是否本地推理后端；拿不到模型信息时默认 false（按在线口径显示，绝不误称本地）。
function activeModelIsLocal(bs) {
  if (!bs || !Array.isArray(bs.savedModels) || !bs.activeModelId) return false;
  const m = bs.savedModels.find(x => x && x.id === bs.activeModelId);
  return isLocalModel(m);
}

export {
  bridge,
  useBridgeState,
  baseUrlIsLoopback,
  isLocalModel,
  activeModelIsLocal,
};
