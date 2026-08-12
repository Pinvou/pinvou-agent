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

function loadMemoryFeature() {
  const state = {
    activeSessionId: 'chat-a',
    memory: { loading: false, profile: null, preferences: [], work_context: [], pending: [], never: [] },
    chatItems: [],
    messages: [],
  };
  return loadFeature('memory.js', state, {
    runSyncOnSession(sid, fn) { fn(); },
    runOnSession(sid, fn) { fn(); },
    patchItemById() {},
  });
}

test('loadMemoryOverview 切走后旧响应不覆盖当前会话记忆', async () => {
  const rt = loadMemoryFeature();
  const get = rt.defer('get_memory_overview');
  const p = rt.api.loadMemoryOverview();            // A 会话发起
  rt.state.activeSessionId = 'chat-b';              // 切走
  get.resolve({ profile: { name: 'A 的记忆' }, preferences: [] });
  await p;
  assert.equal(rt.state.memory.profile, null, '陈旧响应不得把 A 的记忆写进 B 的显示');
});

test('loadMemoryOverview 同会话两次加载：旧响应作废、新响应生效', async () => {
  const rt = loadMemoryFeature();
  const g1 = rt.defer('get_memory_overview');
  const p1 = rt.api.loadMemoryOverview();           // 加载 1
  const g2 = rt.defer('get_memory_overview');
  const p2 = rt.api.loadMemoryOverview();           // 加载 2（序号递增）
  g1.resolve({ profile: { name: '旧' }, preferences: [] });
  await p1;
  assert.equal(rt.state.memory.profile, null, '旧加载的响应必须被序号作废');
  g2.resolve({ profile: { name: '新' }, preferences: [] });
  await p2;
  assert.equal(rt.state.memory.profile.name, '新', '新加载正常写入');
});

// ── personas.js：equip/unequip/syncActivePersona 写入漂移 ───────────

function loadPersonasFeature() {
  const state = {
    activeSessionId: 'chat-a',
    personaPool: { loadState: 'ready' },
    activePersona: null,
    personaEvents: [],
    sessions: [{ id: 'chat-a', title: '新对话' }],
    settings: { language: 'zh' },
    messages: [],
    chatItems: [],
  };
  return loadFeature('personas.js', state, {
    ensureSession: async () => state.activeSessionId,
    personaPlaceholderTitles: {},
    isDefaultChatTitle(title) { return !title || title === '新对话'; },
    listen() {},
    __root: undefined,
  });
}

test('equipPersona 切走后不得改名字/插卡/写挂件（错误会话污染）', async () => {
  const rt = loadPersonasFeature();
  const equip = rt.defer('equip_persona');
  const p = rt.api.equipPersona('persona-x');       // A 会话发起
  rt.state.activeSessionId = 'chat-b';              // 切走
  equip.resolve({ id: 'persona-x', name: '专家X' });
  await p;
  assert.equal(rt.state.activePersona, null, '不得把加持写进切走后的会话');
  assert.equal(rt.state.personaEvents.length, 0, '不得在切走后的会话记 persona 事件');
  assert.equal(rt.calls.invoke.includes('rename_session'), false, '不得重命名切走后的会话');
  assert.equal(rt.state.chatItems.length, 0, '不得在切走后的会话插入加持卡');
});

test('syncActivePersona 切走后陈旧快照不覆盖新会话挂件', async () => {
  const rt = loadPersonasFeature();
  const get = rt.defer('get_active_persona');
  const p = rt.api.syncActivePersona();             // A 发起
  rt.state.activeSessionId = 'chat-b';              // 切走
  get.resolve({ id: 'persona-a', name: 'A专家' });
  await p;
  assert.equal(rt.state.activePersona, null, '陈旧快照不得覆盖切走后的挂件');
});

test('unequipPersona 切走后卸下播报不写进别的会话', async () => {
  const rt = loadPersonasFeature();
  rt.state.activePersona = { id: 'persona-a', name: 'A专家' };
  const unequip = rt.defer('unequip_persona');
  const p = rt.api.unequipPersona();                // A 会话发起
  rt.state.activeSessionId = 'chat-b';              // 切走
  unequip.resolve({});
  await p;
  assert.equal(rt.state.activePersona.id, 'persona-a', '切走后不得清空当前会话的挂件');
  assert.equal(rt.state.chatItems.length, 0, '不得在切走后的会话插入卸下系统消息');
});

// ── workflow.js：openDemo 陈旧覆盖 / stopWorkflowTask 覆盖新 run ────

