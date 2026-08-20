/**
 * 同类竞态回归测试（PR #250 审计 → #260 settings 域）：
 * 陈旧读取覆盖 / await 后写入漂移——修复后的行为快照。
 * 覆盖 tauri settings（vllm 检测/bootstrap 重检/loadModels）。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

/** 通用 feature 装载器：vm 加载 IIFE(window) 形态的桥 feature 文件。 */
function loadFeature(fileName, state, contextOverrides) {
  const root = { __PINVOU_SHARED_I18N__: {} };
  const src = fs.readFileSync(path.join(bridgeDir, fileName), 'utf8');
  vm.runInNewContext(src, {
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
  });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__[fileName.replace('.js', '')];
  const deferreds = {};
  const calls = { invoke: [] };
  const api = factory(Object.assign({
    state,
    notify() { calls.notify = (calls.notify || 0) + 1; },
    bt(key) { return key; },
    addSystemItem() {},
    addChatItem() {},
    timeStr() { return ''; },
    invoke(name, args) {
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
      // 每次创建全新对象：同一 invoke 名的第二次 defer 不得覆盖第一次的
      // resolve（否则旧 promise 永远无人 resolve → 测试挂起）。
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

// ── settings.js：vllm 检测 / bootstrap 重检 / loadModels ─────────────

function loadSettingsFeature() {
  const state = { settings: {}, vllmSetup: undefined, vllmBootstrapping: false };
  return loadFeature('settings.js', state, { listen() {} });
}

test('detectLocalVllmSetup 陈旧检测快照作废（新检测优先）', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('detect_local_vllm_setup');
  const p1 = rt.api.detectLocalVllmSetup({});       // 检测 1
  const d2 = rt.defer('detect_local_vllm_setup');
  const p2 = rt.api.detectLocalVllmSetup({});       // 检测 2（序号递增）
  d1.resolve({ engine_state: 'starting', may_offer_setup: true }); // 旧响应
  await p1;
  assert.equal(rt.state.vllmSetup, undefined, '陈旧检测快照不得写入');
  d2.resolve({ engine_state: 'ready', may_offer_setup: false });   // 新响应
  await p2;
  assert.equal(rt.state.vllmSetup.engine_state, 'ready', '新检测正常写入');
});

test('bootstrap 完成作废在途检测并续接轮询（新检测收敛就绪状态）', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('detect_local_vllm_setup');
  const p1 = rt.api.detectLocalVllmSetup({});        // 检测 1（序号 1）
  const db = rt.defer('bootstrap_local_vllm');
  const pB = rt.api.bootstrapLocalVllm();            // 引导开始
  const d2 = rt.defer('detect_local_vllm_setup');    // 为引导完成后的重检预占槽位
  db.resolve({ done: true });                        // 引导完成：作废在途检测 + 主动重检
  await pB;
  d1.resolve({ engine_state: 'starting', may_offer_setup: true }); // 陈旧快照
  await p1;
  assert.equal(rt.state.vllmSetup, undefined, '引导完成前的陈旧快照不得写入');
  d2.resolve({ engine_state: 'ready', may_offer_setup: false });   // 重检快照
  await new Promise(function (resolve) { setTimeout(resolve, 0); });
  assert.equal(rt.state.vllmSetup.engine_state, 'ready', '引导后的重检正常收敛就绪状态');
});

test('loadModels 后发者胜（旧列表不得覆盖新列表）', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();                    // 加载 1（序号 1）
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();                    // 加载 2（序号 2）
  d1.resolve({ models: [{ id: 'old' }], active_model_id: 'old' }); // 旧响应
  await p1;
  assert.equal(rt.state.savedModels, undefined, '陈旧列表不得写入');
  assert.equal(rt.state.activeModelId, undefined, '陈旧 activeModelId 不得写入');
  d2.resolve({ models: [{ id: 'new' }], active_model_id: 'new' }); // 新响应
  await p2;
  assert.equal(rt.state.savedModels[0].id, 'new', '新列表正常写入');
  assert.equal(rt.state.activeModelId, 'new', '新 activeModelId 正常写入');
});

test('loadModels 陈旧失败不覆盖（后发者胜同样适用于 catch 分支）', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();                    // 加载 1（序号 1）
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();                    // 加载 2（序号 2）
  d2.resolve({ models: [{ id: 'new' }], active_model_id: 'new' }); // 新响应先落地
  await p2;
  assert.equal(rt.state.savedModels[0].id, 'new', '新列表正常写入');
  d1.reject(new Error('stale network failure'));     // 旧请求后失败
  await p1.catch(function () { /* 已知 reject */ });
  assert.equal(rt.state.savedModels[0].id, 'new', '陈旧失败不得清空新列表');
  assert.equal(rt.state.activeModelId, 'new', '陈旧失败不得覆盖 activeModelId');
});
