/**
 * 子代理全量审计产出的同类竞态回归测试（PR #250 系列，域 D / PR #258）：
 * 陈旧读取覆盖 / await 后写入漂移 / 并发重入——修复后的行为快照。
 * 覆盖 tauri memory/personas 两个可单测 feature 的跨会话竞态（含写函数
 * sid 守卫、候选卡按发起会话路由、syncActivePersona 序号作废、引导卡
 * 定向）；web 侧镜像守卫由 memory_personas_race_web.test.mjs 覆盖。
 * workflow/settings 域由后续拆分 PR 另行覆盖。sessions.js
 * （ensureSession/refreshHistoryList/archiveSession）内部依赖太重
 * （sessionStates/switchActiveTo 等），由代码审查 + 既有套件保证。
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
    addSystemItem(text) { state.chatItems.push({ type: 'system', text, id: 'sys-' + (state.chatItems.length + 1) }); },
    addChatItem(item) { item.id = item.id || ('item-' + (state.chatItems.length + 1)); state.chatItems.push(item); },
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
  const routedPatches = [];
  return Object.assign(loadFeature('memory.js', state, {
    runSyncOnSession(sid, fn) { fn(); },
    runOnSession(sid, fn) { fn(); },
    patchItemById(id, patch) {
      const it = state.chatItems.find(i => i.id === id);
      if (it) Object.assign(it, patch);
    },
    patchItemByIdFor(sid, id, patch) {
      routedPatches.push({ sid, id, patch });
      if (sid === state.activeSessionId) {
        const it = state.chatItems.find(i => i.id === id);
        if (it) Object.assign(it, patch);
      }
    },
  }), { routedPatches });
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

test('loadMemoryOverview 反序返回：旧响应后到不得回退新数据', async () => {
  const rt = loadMemoryFeature();
  const g1 = rt.defer('get_memory_overview');
  const p1 = rt.api.loadMemoryOverview();           // 加载 1
  const g2 = rt.defer('get_memory_overview');
  const p2 = rt.api.loadMemoryOverview();           // 加载 2（序号递增）
  g2.resolve({ profile: { name: '新' }, preferences: [] }); // 新响应先回
  await p2;
  assert.equal(rt.state.memory.profile.name, '新');
  g1.resolve({ profile: { name: '旧' }, preferences: [] }); // 旧响应后到
  await p1;
  assert.equal(rt.state.memory.profile.name, '新', '乱序晚到的旧响应不得回退新数据');
});

test('loadMemoryOverview 切草稿后旧响应作废且 loading 被收尾', async () => {
  const rt = loadMemoryFeature();
  const get = rt.defer('get_memory_overview');
  const p = rt.api.loadMemoryOverview();            // A 会话发起
  rt.state.activeSessionId = null;                  // 切草稿(enterDraft 不续发加载)
  get.resolve({ profile: { name: 'A 的记忆' }, preferences: [] });
  await p;
  assert.equal(rt.state.memory.profile, null, '陈旧响应不得写进草稿态');
  assert.equal(rt.state.memory.loading, false, '无人接管的失效加载必须自己清掉 loading');
});

test('loadMemoryOverview 失败响应切走后不得写进当前面板 error', async () => {
  const rt = loadMemoryFeature();
  const get = rt.defer('get_memory_overview');
  const p = rt.api.loadMemoryOverview();
  rt.state.activeSessionId = 'chat-b';
  get.reject(new Error('boom'));
  await p;
  assert.equal(rt.state.memory.error, null, '陈旧会话的加载失败不得显示在 B 的面板');
});

test('confirmMemoryCandidate 切走后：候选卡按发起会话路由、B 面板不被写入', async () => {
  const rt = loadMemoryFeature();
  const confirm = rt.defer('confirm_pending_memory');
  const p = rt.api.confirmMemoryCandidate('mem-1', 'item-1');   // A 会话确认候选卡
  rt.state.activeSessionId = 'chat-b';                          // invoke 往返期间切走
  confirm.resolve({ value: true, runtime: null, warnings: [] });
  await p;
  assert.equal(rt.routedPatches.length, 1, '候选卡 patch 必须被路由(而非丢弃)');
  assert.equal(rt.routedPatches[0].sid, 'chat-a', 'patch 必须路由回发起会话 A');
  assert.equal(rt.routedPatches[0].patch.resolved, true, '候选卡必须被标记为已记住');
  assert.equal(rt.state.chatItems.length, 0, 'B 的对话流不得被写入 A 的候选卡状态');
});

// ── personas.js：equip/unequip/syncActivePersona 写入漂移 ───────────

function loadPersonasFeature() {
  const state = {
    activeSessionId: 'chat-a',
    personaPool: { loadState: 'ready' },
    activePersona: null,
    personaEvents: [],
    // chat-b 也在列表里：旧代码在 await 后重读 sid，会命中 chat-b 并对其
    // rename——使「不得重命名切走后的会话」断言具备判别力。
    sessions: [{ id: 'chat-a', title: '新对话' }, { id: 'chat-b', title: '新对话' }],
    settings: { language: 'zh' },
    messages: [],
    chatItems: [],
  };
  const routedCalls = [];
  return Object.assign(loadFeature('personas.js', state, {
    ensureSession: async () => state.activeSessionId,
    personaPlaceholderTitles: {},
    isDefaultChatTitle(title) { return !title || title === '新对话'; },
    listen() {},
    // 模拟 per-session UI 路由：目标是当前显示时立即执行；切走后台时只记录
    // (生产环境会写进目标会话的 buffer，不影响当前显示)。
    runOnSession(sid, fn) {
      const executed = sid === state.activeSessionId;
      routedCalls.push({ sid, executed });
      if (executed) fn();
    },
  }), { routedCalls });
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

test('syncActivePersona 慢响应不得覆盖权威 equip（同会话乱序）', async () => {
  const rt = loadPersonasFeature();
  const sync = rt.defer('get_active_persona');      // sync 先发起，读到旧值(无卡)
  const syncP = rt.api.syncActivePersona();         // seq=1，挂起
  const equip = rt.defer('equip_persona');
  const equipP = rt.api.equipPersona('persona-x');  // 同会话权威写
  equip.resolve({ id: 'persona-x', name: '专家X' }); // equip 完成:seq bump + 写挂件
  await equipP;
  assert.equal(rt.state.activePersona.id, 'persona-x');
  sync.resolve(null);                                // 慢的 sync 后到,读到的是旧快照
  await syncP;
  assert.equal(rt.state.activePersona.id, 'persona-x', '慢 sync 的旧快照不得覆盖刚加持的挂件');
});

test('unequipPersona 切走后卸下播报不写进别的会话', async () => {
  const rt = loadPersonasFeature();
  rt.state.activePersona = { id: 'persona-a', name: 'A专家' };
  const unequip = rt.defer('unequip_persona');
  const p = rt.api.unequipPersona();                // A 会话发起
  rt.state.activeSessionId = 'chat-b';              // 切走
  unequip.resolve({});
  await p;
  assert.ok(rt.state.activePersona, '切走后不得清空当前会话的挂件');
  assert.equal(rt.state.activePersona.id, 'persona-a');
  assert.equal(rt.state.chatItems.length, 0, '不得在切走后的会话插入卸下系统消息');
});

test('equipPersona rename 挂起期间切走：不插卡/不写挂件/不记事件', async () => {
  const rt = loadPersonasFeature();
  const equip = rt.defer('equip_persona');
  const rename = rt.defer('rename_session');
  const p = rt.api.equipPersona('persona-x');       // A 会话发起，标题默认 → 走 rename 分支
  equip.resolve({ id: 'persona-x', name: '专家X' });
  await new Promise(r => setTimeout(r, 0));          // 推进微任务：equip 恢复并挂起在 rename
  rt.state.activeSessionId = 'chat-b';               // rename 挂起期间切走
  rename.resolve({});
  await p;
  assert.equal(rt.state.activePersona, null, '不得把加持写进切走后的会话');
  assert.equal(rt.state.personaEvents.length, 0, '不得在切走后的会话记 persona 事件');
  assert.equal(rt.state.chatItems.length, 0, '不得在切走后的会话插入加持卡');
});

test('postCardCreatorIntro 默认定向最近 equip 的会话，切走后不串台', async () => {
  const rt = loadPersonasFeature();
  const equip = rt.defer('equip_persona');
  const p = rt.api.equipPersona('pinvou-card-creator'); // A 会话发起(AI 造卡链路)
  rt.state.activeSessionId = 'chat-b';                 // equip 往返期间切走
  equip.resolve({ id: 'pinvou-card-creator', name: '卡牌制造专家' });
  await p;
  rt.api.postCardCreatorIntro();                       // 引导卡必须仍落在 A(发起会话)
  assert.equal(rt.routedCalls.length, 1, '引导卡必须被 per-session 路由');
  assert.equal(rt.routedCalls[0].sid, 'chat-a', '引导卡必须定向到最近 equip 的发起会话');
  assert.equal(rt.routedCalls[0].executed, false, '已切走时不得写进当前显示(B)');
  assert.equal(rt.state.chatItems.length, 0, 'B 的对话流不得被插入 A 的引导卡');
  assert.equal(rt.state.personaEvents.length, 0, 'B 不得被记 persona 事件');
});

// ── 二审补充：失败分支归属与同会话重入的陈旧值 ───────────────────────

test('equipPersona 失败且切走后：失败气泡不落别的会话，lastEquippedSid 作废', async () => {
  const rt = loadPersonasFeature();
  // 前置：A 会话历史成功加持(旧代码失败后 intro 会回退定向到该会话)
  const prior = rt.defer('equip_persona');
  const priorP = rt.api.equipPersona('persona-a');
  prior.resolve({ id: 'persona-a', name: 'A专家' });
  await priorP;
  assert.ok(rt.state.activePersona, '前置：A 已挂卡');
  // 再次发起 equip，后端失败；invoke 往返期间切走
  const equip = rt.defer('equip_persona');
  const p = rt.api.equipPersona('pinvou-card-creator');
  rt.state.activeSessionId = 'chat-b';
  equip.reject(new Error('boom'));
  const card = await p;
  assert.equal(card, null, '失败必须返回 null(调用方据此跳过引导卡)');
  assert.ok(!rt.state.chatItems.some(i => (i.text || '').startsWith('equipFailed')),
    '失败气泡不得插进 await 窗口内切到的会话(随消息流持久化)');
  rt.api.postCardCreatorIntro(); // 即便被误调，也不得回退定向到历史成功 equip 的 A
  assert.equal(rt.routedCalls[0].sid, 'chat-b',
    'lastEquippedSid 已作废：不得把引导卡定向到与本次造卡无关的历史会话 A');
});

test('同会话连续换卡：第二次换卡播报实际被换下的新卡，而非入口捕获的陈旧旧卡', async () => {
  const rt = loadPersonasFeature();
  rt.state.activePersona = { id: 'persona-a', name: 'A专家' }; // A 会话已挂旧卡
  const equip1 = rt.defer('equip_persona');
  const p1 = rt.api.equipPersona('persona-c1');      // 第一次换卡发起,挂起
  const equip2 = rt.defer('equip_persona');
  const p2 = rt.api.equipPersona('persona-c2');      // 第一次完成前又发起第二次(重入)
  equip1.resolve({ id: 'persona-c1', name: 'C1' });
  await p1;
  equip2.resolve({ id: 'persona-c2', name: 'C2' });
  await p2;
  const unequips = rt.state.personaEvents.filter(e => e.kind === 'unequip');
  assert.equal(unequips.length, 2, '两次换卡各播报一次卸下');
  assert.equal(unequips[0].name, 'A专家', '第一次换卡播报换下原卡');
  assert.equal(unequips[1].name, 'C1', '第二次换卡必须播报写点复核到的 C1(陈旧入口值会重复播报 A)');
  assert.equal(rt.state.activePersona.id, 'persona-c2', '终态挂件为最后 equip 的卡');
});

test('equip 挂起期间 unequip 先完成：不重复播报入口捕获的旧卡', async () => {
  const rt = loadPersonasFeature();
  rt.state.activePersona = { id: 'persona-a', name: 'A专家' };
  const equip = rt.defer('equip_persona');
  const p1 = rt.api.equipPersona('persona-x');       // equip 挂起(入口时刻 activePersona=A)
  const p2 = rt.api.unequipPersona();                // equip 完成前先摘下(播报卸下 A)
  await p2;
  equip.resolve({ id: 'persona-x', name: '专家X' }); // equip 后到
  await p1;
  const unequips = rt.state.personaEvents.filter(e => e.kind === 'unequip');
  assert.equal(unequips.length, 1, '只播报一次卸下(陈旧入口 prev 会重复播报)');
  assert.equal(unequips[0].name, 'A专家');
  assert.equal(rt.state.activePersona.id, 'persona-x', '最后完成的 equip 为终态');
});
