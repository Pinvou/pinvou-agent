// Web acceptPlan 生效执行模式门（评审 zhuowp 5780927743 第三条）的运行时回归：
// 从 Auto 进入的 Plan 卡（mode === "plan"、post_accept_mode === "auto"）在 Web
// 被接受时必须显式拒绝且不开 turn（accept_plan 不被调用）；post_accept_mode
// 可执行的接受照常进入受理主体（对照）；当前档 Auto 的既有拒绝面不回潮；
// 拒绝通告按 sid 去重。后端投影（get_mode_state 的 post_accept_mode）的语义
// 由 Rust 侧 post_accept_mode_mirrors_claim_restore 钉住。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const webBridgeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');
const readSharedHelpers = () => fs.readFileSync(path.join(webBridgeRoot, '..', '..', 'shared', 'bridge-shared-helpers.js'), 'utf8');

function bootWebBridge() {
  const storage = new Map();
  const localStorage = {
    getItem(key) { return storage.has(key) ? storage.get(key) : null; },
    setItem(key, value) { storage.set(key, String(value)); },
    removeItem(key) { storage.delete(key); },
  };
  const documentObject = {
    readyState: 'loading',
    addEventListener() {},
    createElement() { return { click() {}, remove() {}, style: {}, setAttribute() {} }; },
    body: { appendChild() {} },
  };
  const calls = { invoke: [] };
  const handlers = {
    web_access_list_sessions: () => Promise.resolve([{ id: 'chat-a', title: '新对话', updated_at: 1 }]),
  };
  const listeners = {};
  const deferreds = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name, args) => {
        calls.invoke.push(name);
        if (handlers[name]) return handlers[name](args);
        if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
        return Promise.resolve(null);
      } },
      event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return function () {}; } },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    addEventListener() {},
    setTimeout,
    clearTimeout,
  };
  const context = vm.createContext({
    window: windowObject,
    document: documentObject,
    navigator: { mediaDevices: null },
    localStorage,
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    structuredClone,
    URL,
    URLSearchParams,
    Blob,
    Uint8Array,
    ArrayBuffer,
    TextEncoder,
    TextDecoder,
  });
  vm.runInContext(readSharedHelpers(), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state');
  vm.runInContext(read('bridge/domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
  const api = windowObject.TauriBridge;
  return {
    flat,
    calls,
    view() { return flat.getState(); },
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    setHandler(name, fn) { handlers[name] = fn; },
    emit(name, payload) { (listeners[name] || []).forEach(fn => fn({ payload })); },
    interaction: api.interaction,
  };
}

// 物化活动会话（最小链路，与 memory_personas_race_web 的 primeSessionA 同款：
// equipPersona → ensureSession → web_access_create_session）。
async function primeSessionA(rt) {
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const rename = rt.defer('rename_session');
  const p = rt.interaction.equipPersona
    ? rt.interaction.equipPersona('persona-prime')
    : rt.flat.equipPersona('persona-prime');
  create.resolve({ id: 'chat-a', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); });
  equip.resolve({ id: 'persona-prime', name: '引导卡' });
  await new Promise(r => { setTimeout(r, 0); });
  rename.resolve({});
  await p;
  const view = rt.view();
  assert.equal(view.activeSessionId, 'chat-a', 'precondition: chat-a must be active');
  return view;
}

function blockedNoticeCount(view) {
  // 三语文案都含「Auto」档位词（en "Auto mode" / ja "Auto モード" / zh "Auto 档"）。
  return view.chatItems.filter(item => item.type === 'system' && String(item.text || '').includes('Auto')).length;
}

test('web: acceptPlan 拒绝将回退 Auto 的计划卡且不开 turn（post_accept_mode 门）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  // 从 Auto 进入的 Plan 卡：当前档 plan（旧门检放行），accept 恢复 Auto。
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', post_accept_mode: 'auto', multi_agent: false }));
  let acceptPlanInvoked = false;
  rt.setHandler('accept_plan', () => { acceptPlanInvoked = true; return Promise.resolve({}); });
  await assert.rejects(
    rt.interaction.acceptPlan('item-1', '# Plan', undefined, 'ticket-1'),
    error => /Auto/.test(String(error && error.message)),
    'post-accept Auto 的接受必须走既有 Auto 拒绝面（webAutoSendBlocked 文案）'
  );
  assert.equal(acceptPlanInvoked, false, '拒绝不得开 turn：accept_plan 不被调用');
  assert.equal(blockedNoticeCount(rt.view()), 1, '拒绝必须落一条用户面通告');
});

test('web: acceptPlan 的 post_accept_mode 可执行时照常进入受理主体（对照）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', post_accept_mode: 'yolo', multi_agent: false }));
  rt.setHandler('accept_plan', () => Promise.resolve({ mode: 'yolo', multi_agent: false }));
  // 无可受理卡（本 harness 未种 plan_card）→ 受理主体按既有契约静默返回，
  // 但门必须放行（不拒绝、不落 Auto 通告）——本断言的对照面即门通过。
  await rt.interaction.acceptPlan('item-1', '# Plan', undefined, 'ticket-1');
  assert.equal(blockedNoticeCount(rt.view()), 0, '可执行的接受不得被姿态门拦截');
});

test('web: 当前档 Auto 的既有拒绝面经共享助手不回潮', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'auto', post_accept_mode: 'auto', multi_agent: false }));
  let acceptPlanInvoked = false;
  rt.setHandler('accept_plan', () => { acceptPlanInvoked = true; return Promise.resolve({}); });
  await assert.rejects(
    rt.interaction.acceptPlan('item-1', '# Plan', undefined, 'ticket-1'),
    error => /Auto/.test(String(error && error.message))
  );
  assert.equal(acceptPlanInvoked, false);
});

test('web: 拒绝通告按 sid 去重（同一会话重复接受不刷屏）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', post_accept_mode: 'auto', multi_agent: false }));
  rt.setHandler('accept_plan', () => Promise.resolve({}));
  await assert.rejects(rt.interaction.acceptPlan('item-1', '# Plan', undefined, 'ticket-1'), () => true);
  await assert.rejects(rt.interaction.acceptPlan('item-2', '# Plan', undefined, 'ticket-1'), () => true);
  assert.equal(blockedNoticeCount(rt.view()), 1, '同一会话的重复拒绝只落一条通告');
});
