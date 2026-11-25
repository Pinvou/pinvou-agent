/**
 * saveModel 链尾裁定(review P2×2,晚到完成):单行保存的复合刷新链
 * (save_model → loadModels → …)是 upsert **之后**发起的读取,序号后发者胜
 * ——链尾成功列表缺席本身份 ⇒ 记录在刷新尾窗内被删,调用方不得入 committed
 * 簿(否则簿键永无成功列表可确认,合法同身份重加被永久拦死)。本文件锁定
 * 两个平台桥返回的裁定契约:deferred(批量,无刷新尾)/ failed(刷新失败,
 * 无证据照常入簿)/ ok(附链尾列表内容)。
 * tauri lane 复用 tests/settings_race.test.mjs 的 feature 装载器;web lane
 * 复用 tests/memory_personas_race_web.test.mjs 的整体装载(web 桥是独立
 * 文本镜像,单独锁定防只同步一侧)。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const tauriBridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');
const webBridgeDir = path.join(here, '..', 'src', 'platform', 'web');

// ── tauri lane:settings.js feature 装载(同 settings_race.test.mjs)──

function loadTauriSettings(state, contextOverrides) {
  const root = {};
  const src = fs.readFileSync(path.join(tauriBridgeDir, 'settings.js'), 'utf8');
  vm.runInNewContext(
    fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8') + '\n' + src,
    { window: root, globalThis: root, setTimeout, clearTimeout },
  );
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__['settings'];
  const deferreds = {};
  const calls = { invoke: [] };
  const api = factory(Object.assign({
    state,
    listen() {},
    notify() { calls.notify = (calls.notify || 0) + 1; },
    bt(key) { return key; },
    addSystemItem() {},
    addChatItem() {},
    timeStr() { return ''; },
    invoke(name) {
      calls.invoke.push(name);
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      return Promise.resolve({});
    },
  }, contextOverrides || {}));
  return {
    api,
    state,
    calls,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

const SAVED_RECORD = {
  id: 'm1', model: 'glm-5', base_url: 'https://api.example.com/v1', name: 'GLM',
};

function makeState() {
  return { settings: {} };
}

test('tauri saveModel ok: the outcome carries the tail list so the caller can reconcile presence', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  // 刷新尾的 list_models 返回含该身份的权威列表(upsert 后的读取)。
  d.resolve({ models: [SAVED_RECORD, { id: 'm0', model: 'other', base_url: 'https://x/v1' }], active_model_id: 'm0' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '成功刷新给 ok 裁定');
  assert.ok(Array.isArray(outcome.models), 'ok 裁定必须携带链尾列表');
  assert.equal(outcome.models.length, 2, '列表元素与成功读取一致');
  assert.equal(outcome.models[0].id, 'm1', '列表首元素为刚落库记录(逐元素断言,跨 realm 不用 deepEqual)');
  // 链跑全:save_model + list_models + get_settings + 有效配置读取。
  for (const cmd of ['save_model', 'list_models']) {
    assert.ok(rt.calls.invoke.includes(cmd), `链必须包含 ${cmd}`);
  }
});

test('tauri saveModel ok with the identity absent: the tail list reflects a mid-tail deletion (verdict data)', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  // 刷新尾窗内的删除赢下序号:链尾列表不再含该身份——调用方按裁定跳过入簿。
  d.resolve({ models: [{ id: 'm0', model: 'other', base_url: 'https://x/v1' }], active_model_id: 'm0' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '列表读取本身成功,裁定仍是 ok');
  assert.equal(outcome.models.some(m => m && m.id === 'm1'), false, '链尾列表缺席该身份——入簿与否由调用方按此裁定');
});

test('tauri saveModel failed: the swallowed list read still resolves with the failed verdict (no evidence)', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  d.reject(new Error('list read failure')); // loadModels 吞错清列表(r3 lane G 形态)
  const outcome = await p;
  assert.equal(outcome.refresh, 'failed', '刷新失败给 failed 裁定');
  assert.equal(outcome.models, undefined, 'failed 裁定不携带列表——调用方无证据,照常入簿');
  assert.equal(rt.state.modelsListLoadFailed, true, '失败标记已置(镜像 effect 冻结源)');
});

test('tauri saveModel deferred: no reload tail runs and the outcome carries no list evidence', async () => {
  const rt = loadTauriSettings(makeState());
  const outcome = await rt.api.saveModel(SAVED_RECORD, { deferRefresh: true });
  assert.equal(outcome.refresh, 'deferred', '批量跳过刷新尾给 deferred 裁定');
  assert.equal(outcome.models, undefined, 'deferred 无列表证据——调用方入簿且不带缺席地板');
  assert.deepEqual(rt.calls.invoke, ['save_model'], 'deferred 只发 save_model,三个 reload 一个都不跑');
});

// ── web lane:整体装载(platform/web/bridge.js + domain-adapter)────────

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
  const handlers = {};
  const deferreds = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name) => {
        calls.invoke.push(name);
        if (handlers[name]) return handlers[name]();
        if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
        return Promise.resolve(null);
      } },
      event: { listen: async () => function () {} },
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
  vm.runInContext(fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(fs.readFileSync(path.join(webBridgeDir, 'bridge.js'), 'utf8'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  return {
    flat,
    calls,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    setHandler(name, fn) { handlers[name] = fn; },
  };
}

test('web saveModel ok/failed/deferred mirror the tauri verdict contract', async () => {
  const rt = bootWebBridge();
  // ok:刷新尾读取返回含该身份的列表。
  rt.setHandler('list_models', () => Promise.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' }));
  const okOutcome = await rt.flat.saveModel(SAVED_RECORD);
  assert.equal(okOutcome.refresh, 'ok', 'web lane 成功刷新同样给 ok 裁定');
  assert.ok(Array.isArray(okOutcome.models), 'ok 裁定携带链尾列表');
  assert.equal(okOutcome.models[0].id, 'm1', '列表元素与成功读取一致(逐元素断言)');
  // ok 但列表缺席(刷新尾窗内被删):裁定数据交调用方跳过入簿。
  rt.setHandler('list_models', () => Promise.resolve({ models: [], active_model_id: null }));
  const absentOutcome = await rt.flat.saveModel({ ...SAVED_RECORD, id: 'm2' });
  assert.equal(absentOutcome.refresh, 'ok', '读取成功裁定仍 ok');
  assert.equal(absentOutcome.models.length, 0, '缺席裁定:调用方按列表内容自行跳过入簿');
  // failed:loadModels 吞错 → failed 裁定无列表证据。
  rt.setHandler('list_models', () => Promise.reject(new Error('list read failure')));
  const failedOutcome = await rt.flat.saveModel({ ...SAVED_RECORD, id: 'm3' });
  assert.equal(failedOutcome.refresh, 'failed', 'web lane 刷新失败同样给 failed 裁定');
  assert.equal(failedOutcome.models, undefined, 'failed 不携带列表');
  // deferred:批量只发 save_model。
  const deferredOutcome = await rt.flat.saveModel(SAVED_RECORD, { deferRefresh: true });
  assert.equal(deferredOutcome.refresh, 'deferred', 'web lane 批量同样给 deferred 裁定');
  assert.equal(deferredOutcome.models, undefined, 'deferred 无列表证据');
});
