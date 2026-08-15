/**
 * web 桥（platform/web/bridge.js）记忆/专家卡跨会话竞态回归测试（PR #258）。
 * tauri 侧由 memory_personas_race.test.mjs 覆盖；web 桥是独立文本镜像，
 * 单独锁定，防止未来只同步一侧时漏改守卫。
 *
 * 状态访问说明：flat.getState() 返回 structuredClone 快照（写它无效），
 * 断言前必须重新拉快照；「切走」用 createNewSession()（即 enterDraft，
 * activeSessionId → null，零 invoke）驱动——这正是审计发现的 enterDraft
 * 高危路径，与切到 B 会话命中同一守卫分支。会话 A 的物化走真实 lazy-session
 * 链路（草稿态 equipPersona → ensureSession → web_access_create_session）。
 * 启动方式复用 web_bridge_domain_contract.test.mjs：vm 整体装载 web 桥
 * + domain-adapter，invoke 注入 per-command deferred。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const webBridgeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');

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
    list_sessions: () => Promise.resolve([{ id: 'chat-a', title: '新对话', updated_at: 1 }]),
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
  vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state');
  vm.runInContext(read('bridge/domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
  const api = windowObject.TauriBridge;
  return {
    flat,
    calls,
    // 断言用：每次读桥内最新快照（写快照无效）。
    view() { return flat.getState(); },
    // 切走：enterDraft 把 activeSessionId 置 null（零 invoke）。
    leave() { flat.createNewSession(); },
    defer(name) {
      // 每次创建全新对象：同一 invoke 名的第二次 defer 不得覆盖第一次的 resolve。
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    emit(name, payload) { (listeners[name] || []).forEach(fn => fn({ payload })); },
    personas: api.personas,
    memory: api.memory,
  };
}

// 物化会话 A 并挂上一张引导卡：走真实 lazy-session 链路(草稿 equip →
// ensureSession → web_access_create_session → equip_persona 全 happy path)。
async function primeSessionA(rt) {
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const rename = rt.defer('rename_session');
  const p = rt.personas.equipPersona('persona-prime');
  create.resolve({ id: 'chat-a', transcript_revision: 1 });
  await new Promise(r => setTimeout(r, 0)); // ensureSession 链推进到 equip_persona
  equip.resolve({ id: 'persona-prime', name: '引导卡' });
  await new Promise(r => setTimeout(r, 0)); // equip 恢复并挂起在 rename(标题默认)
  rename.resolve({});
  await p;
  const view = rt.view();
  assert.equal(view.activeSessionId, 'chat-a', 'precondition: chat-a must be active');
  assert.equal(view.activePersona && view.activePersona.id, 'persona-prime', 'precondition: persona mounted');
  return view;
}

test('web: loadMemoryOverview 切走后旧响应不覆盖当前显示', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const get = rt.defer('get_memory_overview');
  const p = rt.memory.loadMemoryOverview();
  rt.leave(); // 切草稿
  get.resolve({ profile: { name: 'A 的记忆' }, preferences: [] });
  await p;
  const view = rt.view();
  assert.equal(view.memory.profile, null, '陈旧响应不得把 A 的记忆写进草稿显示');
  assert.equal(view.memory.loading, false, '无人接管的失效加载必须自己清掉 loading');
});

test('web: equipPersona 切走后不得改名字/插卡/写挂件', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const renameCallsBefore = rt.calls.invoke.filter(n => n === 'rename_session').length;
  const equip = rt.defer('equip_persona');
  const p = rt.personas.equipPersona('persona-x');
  rt.leave(); // equip 往返期间切草稿(草稿工作集已换空)
  equip.resolve({ id: 'persona-x', name: '专家X' });
  await p;
  const view = rt.view();
  // 草稿合法显示空挂件；判别点是旧代码会把加持写进草稿(挂件+插卡+rename)。
  assert.equal(view.activePersona, null, '不得把加持写进切走后的草稿显示');
  assert.equal(view.personaEvents.length, 0, '不得在切走后记 persona 事件');
  assert.equal(view.chatItems.length, 0, '不得在切走后插入加持/卸下卡');
  assert.equal(rt.calls.invoke.filter(n => n === 'rename_session').length, renameCallsBefore, '不得重命名切走后的会话');
});

test('web: unequipPersona 切走后卸下播报不写进别的会话', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const unequip = rt.defer('unequip_persona');
  const p = rt.personas.unequipPersona();
  rt.leave(); // unequip 往返期间切草稿(草稿工作集已换空)
  unequip.resolve({});
  await p;
  const view = rt.view();
  assert.equal(view.activePersona, null, '草稿无挂件(合法)');
  assert.equal(view.chatItems.length, 0, '卸下播报不得插进切走后的草稿对话流');
  assert.equal(view.personaEvents.length, 0, '不得在切走后记 persona 事件');
});

test('web: syncActivePersona 切走后陈旧快照不覆盖新会话挂件', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  // syncActivePersona 是桥内部函数：经真实 persona_changed 事件路径触发。
  const get = rt.defer('get_active_persona');
  rt.emit('session:persona_changed', { id: 'chat-a' }); // A 的后端事件到达
  rt.leave(); // 响应返回前切草稿(草稿工作集已换空)
  get.resolve({ id: 'persona-other', name: '别的专家' });
  await new Promise(r => setTimeout(r, 0));
  assert.equal(rt.view().activePersona, null, '陈旧快照不得把挂件写进切走后的草稿');
});

test('web: saveMemoryProfilePatch 切走后写结果不落进当前面板', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const update = rt.defer('update_memory_profile');
  const overview = rt.defer('get_memory_overview'); // 尾部重载挂起，锁定中间态
  const p = rt.memory.saveMemoryProfilePatch({ name: 'A 的档案' });
  rt.leave(); // update 往返期间切草稿
  update.resolve({ profile: { name: 'A 的档案' }, runtime: null, warnings: [] });
  await new Promise(r => setTimeout(r, 0)); // update 恢复；尾部重载已发起并挂起
  assert.equal(rt.view().memory.profile, null, '切走后 A 的写结果不得即时渲染进草稿面板');
  overview.resolve(null);
  await p;
  assert.equal(rt.view().memory.profile, null, '最终也不得显示 A 的档案');
});
