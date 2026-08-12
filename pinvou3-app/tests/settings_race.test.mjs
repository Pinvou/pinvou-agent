/**
 * 子代理全量审计产出的同类竞态回归测试（PR #250 延续）：
 * 陈旧读取覆盖 / await 后写入漂移 / 并发重入——修复后的行为快照。
 * 覆盖 tauri memory/personas/workflow/settings 四个可单测 feature；
 * sessions.js（ensureSession/refreshHistoryList/archiveSession）内部依赖
 * 太重（sessionStates/switchActiveTo 等），由代码审查 + 既有套件保证。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

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

// ── memory.js：loadMemoryOverview 陈旧覆盖 ──────────────────────────

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
