import { invokeTauri, isTauriAvailable } from '../platform/tauri/client.js';

const TOKEN_KEY = 'pinvou.ctrip.wendao.apiToken';
const WENDAO_ENDPOINT = 'https://externalcallback.ctrip.com/skills/api/crew/qclaw/searchInfo';
export const CTRIP_WENDAO_OPEN_PLATFORM_URL = 'https://www.ctrip.com/wendao/openclaw';
export const CTRIP_FLIGHT_SEARCH_URL = 'https://flights.ctrip.com/online/channel/domestic';

function wait(ms = 260) {
  return new Promise(resolve => window.setTimeout(resolve, ms));
}

function storageAvailable() {
  try {
    return typeof window !== 'undefined' && !!window.localStorage;
  } catch {
    return false;
  }
}

function readToken() {
  if (!storageAvailable()) return '';
  return String(window.localStorage.getItem(TOKEN_KEY) || '').trim();
}

function writeToken(token) {
  if (!storageAvailable()) return;
  const value = String(token || '').trim();
  if (value) window.localStorage.setItem(TOKEN_KEY, value);
  else window.localStorage.removeItem(TOKEN_KEY);
}

export function isCtripTravelPrompt(text) {
  const value = String(text || '').trim();
  if (!value) return false;
  const hasTravelIntent = /(订|买|下单|预订|帮我定|帮我订|查|查询|搜索|看看|规划|推荐|攻略)/.test(value);
  const hasTravelObject = /(机票|航班|飞机票|酒店|携程|旅行|行程|景点|签证|trip\.com|ctrip)/i.test(value);
  return hasTravelIntent && hasTravelObject;
}

export const isCtripBookingPrompt = isCtripTravelPrompt;

export function extractTripDraft(prompt) {
  const text = String(prompt || '').trim();
  const tomorrow = /明天/.test(text);
  const afterTomorrow = /后天/.test(text);
  const fromTo = text.match(/从\s*([\u4e00-\u9fa5A-Za-z]+)\s*(?:到|去)\s*([\u4e00-\u9fa5A-Za-z]+)/);
  const toOnly = text.match(/(?:去|到)\s*([\u4e00-\u9fa5A-Za-z]+)(?:的?机票|航班|出差|旅行|$)/);
  return {
    origin: fromTo?.[1] || '',
    destination: fromTo?.[2] || (!fromTo ? (toOnly?.[1] || '') : ''),
    date: tomorrow ? '明天' : afterTomorrow ? '后天' : '',
    cabin: /商务|公务/.test(text) ? '商务舱' : '经济舱',
    adults: '1',
    children: /儿童|小孩|孩子/.test(text) ? '1' : '0',
    budget: '',
    timePreference: /早上|上午/.test(text) ? '上午' : /下午/.test(text) ? '下午' : /晚上|夜间/.test(text) ? '晚上' : '不限',
  };
}

function buildQuery(prompt, details) {
  const pieces = [];
  if (details.origin || details.destination) {
    pieces.push(`行程：${details.origin || '未指定出发地'} 到 ${details.destination || '未指定目的地'}`);
  }
  if (details.date) pieces.push(`日期：${details.date}`);
  if (details.cabin) pieces.push(`舱位：${details.cabin}`);
  if (details.adults) pieces.push(`成人数量：${details.adults}`);
  if (details.children) pieces.push(`儿童数量：${details.children}`);
  if (details.budget) pieces.push(`预算：${details.budget}`);
  if (details.timePreference) pieces.push(`时间偏好：${details.timePreference}`);
  const suffix = pieces.length
    ? `\n已确认条件：${pieces.join('；')}。\n请基于携程问道查询可选方案，列出价格/时间/退改签要点，并说明需要前往携程完成下单。`
    : '';
  return `${String(prompt || '').trim()}${suffix}`;
}

function normalizeResult(data) {
  const result = data?.result ?? data;
  if (typeof result === 'string') return result.trim();
  if (result && typeof result === 'object') {
    if (typeof result.content === 'string') return result.content.trim();
    if (typeof result.text === 'string') return result.text.trim();
    return JSON.stringify(result, null, 2);
  }
  return '';
}

export function buildCtripSearchUrl(details = {}) {
  const query = [
    details.origin && `from=${encodeURIComponent(details.origin)}`,
    details.destination && `to=${encodeURIComponent(details.destination)}`,
    details.date && `date=${encodeURIComponent(details.date)}`,
    details.adults && `adults=${encodeURIComponent(details.adults)}`,
    details.children && `children=${encodeURIComponent(details.children)}`,
  ].filter(Boolean).join('&');
  return query ? `${CTRIP_FLIGHT_SEARCH_URL}?${query}` : CTRIP_FLIGHT_SEARCH_URL;
}

export const ctripWendaoConnector = {
  async getCapabilityMatrix() {
    await wait(120);
    return {
      provider: '携程问道（WorkBuddy）',
      connectorId: 'ctrip-wendao',
      source: 'WorkBuddy connectors marketplace',
      mode: 'wendao-query',
      tokenAuth: true,
      queryTravel: true,
      searchFlights: true,
      searchHotels: true,
      itineraryPlanning: true,
      attractionRecommendations: true,
      createOrderDraft: false,
      submitOrder: false,
      openPayment: false,
      openOfficialCtrip: true,
    };
  },

  async authStatus() {
    await wait(100);
    const connected = Boolean(readToken());
    return {
      connected,
      accountName: connected ? '携程问道 API Token' : '',
      scopes: connected ? ['wendao.searchInfo'] : [],
      tokenConfigured: connected,
      docUrl: CTRIP_WENDAO_OPEN_PLATFORM_URL,
    };
  },

  async connectAccount({ token } = {}) {
    const value = String(token || '').trim();
    if (!value) throw new Error('需要填写携程问道 API Token。');
    writeToken(value);
    return this.authStatus();
  },

  async disconnectAccount() {
    writeToken('');
    return this.authStatus();
  },

  async queryTravel({ prompt, details }) {
    const token = readToken();
    if (!token) throw new Error('缺少 WENDAO_API_KEY，请先配置携程问道 Token。');
    const query = buildQuery(prompt, details || {});
    if (isTauriAvailable()) {
      const result = await invokeTauri('query_ctrip_wendao', { token, query });
      return {
        query,
        result,
        officialUrl: buildCtripSearchUrl(details || {}),
        rawReceivedAt: new Date().toISOString(),
      };
    }
    const response = await fetch(WENDAO_ENDPOINT, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        inputs: {
          token,
          query,
        },
      }),
    });
    if (!response.ok) {
      throw new Error(`携程问道请求失败：HTTP ${response.status}`);
    }
    const data = await response.json();
    const result = normalizeResult(data);
    if (!result) throw new Error('携程问道没有返回可展示的结果。');
    return {
      query,
      result,
      officialUrl: buildCtripSearchUrl(details || {}),
      rawReceivedAt: new Date().toISOString(),
    };
  },
};
