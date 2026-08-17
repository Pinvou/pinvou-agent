import { invokeTauri, isTauriAvailable } from '../platform/tauri/client.js';

function wait(ms = 800) {
  return new Promise(resolve => window.setTimeout(resolve, ms));
}

export const ctripBrowserAssist = {
  capabilityStatus() {
    const available = isTauriAvailable();
    return {
      available,
      mode: available ? 'tauri-webview' : 'unavailable',
      canOpenAssistWindow: available,
      canInjectSearch: available,
      canSubmitOrder: false,
      canPay: false,
    };
  },

  async startSearch({ url, details } = {}) {
    if (!isTauriAvailable()) {
      throw new Error('当前环境不是桌面端，无法打开携程专用协助窗口。');
    }
    const targetUrl = String(url || '').trim();
    if (!targetUrl) throw new Error('缺少携程页面链接。');
    await invokeTauri('open_ctrip_assist_window', { url: targetUrl });
    await wait(1400);
    const result = await invokeTauri('run_ctrip_search_assist', {
      details: details || {},
    });
    return result || {};
  },

  async openWindow(url) {
    if (!isTauriAvailable()) {
      throw new Error('当前环境不是桌面端，无法打开携程专用协助窗口。');
    }
    await invokeTauri('open_ctrip_assist_window', { url });
  },
};
