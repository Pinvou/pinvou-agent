/**
 * 同类竞态回归测试（PR #250 审计 → #260 settings 域）：
 * 陈旧读取覆盖 / await 后写入漂移——修复后的行为快照。
 * Covers model-list loading in the tauri settings feature.
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
  const root = {};
  const src = fs.readFileSync(path.join(bridgeDir, fileName), 'utf8');
  vm.runInNewContext(fs.readFileSync(path.join(bridgeDir, '..', '..', '..', 'shared', 'bridge-shared-helpers.js'), 'utf8') + '\n' + src, {
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
    // eslint-disable-next-line no-unused-vars -- test stub / deferred assignment kept
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

// ── settings.js: loadModels ─────────────────────────────────────────

function loadSettingsFeature() {
  const state = { settings: {} };
  return loadFeature('settings.js', state, { listen() {} });
}

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
  assert.notEqual(rt.state.modelsListLoadFailed, true, '陈旧失败不得置失败标记');
});

// Failure marker (review P2, Edge reproduced one identity under two ids):
// the catch arm sets modelsListLoadFailed while clearing the list, so the
// SettingsView saved-identity guards can tell the empty list handed over by
// a failed read from a genuinely empty one; the next successful read clears
// it and a real deletion's refresh thaws the guards as usual.
test('loadModels sets the failure marker, the next successful read clears it', async () => {
  const rt = loadSettingsFeature();
  assert.notEqual(rt.state.modelsListLoadFailed, true, '初始无失败标记');
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();
  d1.reject(new Error('fresh read failure'));        // 当次失败(序号仍最新)
  await p1.catch(function () { /* 已知 reject */ });
  assert.equal(rt.state.savedModels.length, 0, '失败清空列表(展示语义不变)');
  assert.equal(rt.state.modelsListLoadFailed, true, '当次失败置失败标记');
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  d2.resolve({ models: [{ id: 'back' }], active_model_id: 'back' });
  await p2;
  assert.equal(rt.state.modelsListLoadFailed, false, '成功读取清零失败标记');
});

// Pre-mount boundary (review P2, two independent reviewer reproductions):
// the shared loader owns the last successful read. A failed read empties
// the displayed list but must retain lastConfirmedModels — the SettingsView
// mirror seeds its freeze from this snapshot when the failure lands before
// the consumer's first mount (a component-owned freeze alone would start
// from a never-populated mirror and re-open a confirmed identity for a
// second record id). The next successful read — a real deletion's refresh
// included — replaces the snapshot, which is what keeps a genuinely deleted
// model re-addable.
test('loadModels retains lastConfirmedModels across a failed read and replaces it on success', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();
  d1.resolve({ models: [{ id: 'a' }], active_model_id: 'a' });
  await p1;
  // Element-wise asserts (not deepEqual): the vm harness normalizes the
  // state array into the bridge realm, so a cross-realm deepStrictEqual
  // would misfire on the different Array prototype.
  assert.equal(rt.state.lastConfirmedModels.length, 1, '成功读取写入末次确认快照');
  assert.equal(rt.state.lastConfirmedModels[0].id, 'a', '快照元素与成功读取一致');
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  d2.reject(new Error('pre-mount refresh failure')); // 当次失败(序号仍最新)
  await p2.catch(function () { /* 已知 reject */ });
  assert.equal(rt.state.savedModels.length, 0, '失败清空展示列表(展示语义不变)');
  assert.equal(rt.state.modelsListLoadFailed, true, '失败置失败标记');
  assert.equal(rt.state.lastConfirmedModels.length, 1, '失败保留末次确认快照(pre-mount 首挂载播种源)');
  assert.equal(rt.state.lastConfirmedModels[0].id, 'a', '快照元素在失败后不变');
  const d3 = rt.defer('list_models');
  const p3 = rt.api.loadModels();
  d3.resolve({ models: [{ id: 'b' }], active_model_id: 'b' });
  await p3;
  assert.equal(rt.state.lastConfirmedModels.length, 1, '成功读取(含真实删除后的刷新)替换快照');
  assert.equal(rt.state.lastConfirmedModels[0].id, 'b', '快照元素替换为新读取');
  assert.equal(rt.state.modelsListLoadFailed, false, '成功读取清零失败标记');
});

test('stale loadModels failure keeps both the newer list and the newer snapshot', async () => {
  const rt = loadSettingsFeature();
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();                    // 加载 1(序号 1,后失败)
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();                    // 加载 2(序号 2,先成功)
  d2.resolve({ models: [{ id: 'new' }], active_model_id: 'new' });
  await p2;
  d1.reject(new Error('stale failure'));
  await p1.catch(function () { /* 已知 reject */ });
  assert.equal(rt.state.lastConfirmedModels.length, 1, '陈旧失败不得回退快照');
  assert.equal(rt.state.lastConfirmedModels[0].id, 'new', '快照仍指向较新的成功读取');
  assert.notEqual(rt.state.modelsListLoadFailed, true, '陈旧失败不得置失败标记');
});
