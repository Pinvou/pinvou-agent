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

// API Key gate 只覆盖正在交互的聊天页。设置页必须始终可达，否则首次启动时
// “去配置”按钮会把用户送到仍被 gate 盖住的设置页，形成无法录入 Key 的死锁。
function shouldShowApiKeyGate(bs, currentView, bridgeAvailable) {
  const inChat = currentView === 'chat'
    || (currentView === 'scheduled' && !!(bs && bs.scheduledRunContext));
  const config = bs && bs.effectiveModelConfig;
  const missingCredential = config
    && (config.credential_state === 'missing' || config.credential_state === 'unavailable');
  return !!(bridgeAvailable && inChat && missingCredential && !isLocalModel(config));
}

export {
  bridge,
  useBridge,
  baseUrlIsLoopback,
  isLocalModel,
  activeModelIsLocal,
  shouldShowApiKeyGate,
};
