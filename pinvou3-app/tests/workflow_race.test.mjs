/**
 * 工作流 run 竞态回归测试（PR #250 系列拆分的 domain E，PR #259 延续）：
 * 陈旧读取覆盖 / await 后写入漂移 / 并发重入——修复后的行为快照。
 * 仅覆盖 tauri bridge 的 workflow feature（web/bridge.js 走同一批守卫逻辑，
 * 由代码审查 + 既有套件保证；web 版 approve/reject 守卫与本套件同源）。
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

function loadWorkflowFeature() {
  const state = {
    activeSessionId: 'chat-a',
    workflow: {
      run: { active: false, sessionId: null, status: 'idle', agents: {}, cards: [], selectedRole: null },
      demo: null,
      bindings: {},
      activeSkillName: null,
      phases: [],
      currentPhaseId: null,
      reachedPhaseIds: [],
      loadState: 'ready',
      skills: [],
    },
    messages: [],
    chatItems: [],
  };
  let rt;
  rt = loadFeature('workflow.js', state, {
    dialogOpen() {},
    resetPendingAssistant() {},
    syncModeState: async () => {},
    refreshHistoryList: async () => {},
    markWorkflowRunStopped() { rt.calls.markStopped = (rt.calls.markStopped || 0) + 1; state.workflow.run.status = 'stopped'; },
    refreshRunState: async () => {},
    resolveRunCard(cardId, cardState) {
      state.workflow.run.cards.forEach((c) => { if (c.cardId === cardId) { c.resolved = true; c.cardState = cardState; } });
    },
    resolveRunCardsForRole(roleId, cardState) {
      state.workflow.run.cards.forEach((c) => { if (c.kind === 'gate' && c.roleId === roleId && !c.resolved) { c.resolved = true; c.cardState = cardState; } });
    },
  });
  return rt;
}

test('openDemo 关闭后陈旧响应不得重新弹开', async () => {
  const rt = loadWorkflowFeature();
  const read = rt.defer('read_skill_demo');
  const p = rt.api.openDemo('demo-a');
  rt.api.closeDemo();                                // 用户关闭
  read.resolve({ content: '...', file_kind: 'md' });
  await p;
  assert.equal(rt.state.workflow.demo, null, '已关闭的弹窗不得被陈旧响应重开');
});

test('openDemo 改开别的 demo 时旧响应不得覆盖新内容', async () => {
  const rt = loadWorkflowFeature();
  const readA = rt.defer('read_skill_demo');
  const pA = rt.api.openDemo('demo-a');
  const readB = rt.defer('read_skill_demo');         // B 的独立请求（真实场景两次 IPC）
  const pB = rt.api.openDemo('demo-b');              // 用户改开 B
  readA.resolve({ content: 'A 的内容', file_kind: 'md' });
  await pA;
  assert.equal(rt.state.workflow.demo.name, 'demo-b', 'A 的陈旧响应不得覆盖 B');
  assert.equal(rt.state.workflow.demo.loading, true, 'B 仍在加载（未被 A 完成）');
  readB.resolve({ content: 'B 的内容', file_kind: 'md' });
  await pB;
  assert.equal(rt.state.workflow.demo.content, 'B 的内容', 'B 的内容正常写入');
});

test('stopWorkflowTask 停止旧 run 不得把新 run 标记为 stopped', async () => {
  const rt = loadWorkflowFeature();
  rt.state.workflow.run = { active: true, sessionId: 'run-a', status: 'running', agents: {}, cards: [] };
  const stop = rt.defer('stop_workflow');
  const p = rt.api.stopWorkflowTask('user_stopped'); // 停 run-a
  rt.state.workflow.run = { active: true, sessionId: 'run-b', status: 'running', agents: {}, cards: [] }; // 新 run 开始
  stop.resolve({ brief: 'old' });
  await p;
  assert.equal(rt.calls.markStopped, undefined, '不得把新 run-b 标记为 stopped');
  assert.equal(rt.state.workflow.run.status, 'running', 'run-b 保持运行');
});

test('approveWorkflowGate 批准旧 run 不得落到新 run 的卡片上', async () => {
  const rt = loadWorkflowFeature();
  rt.state.workflow.run = {
    active: true, sessionId: 'run-a', status: 'running',
    agents: {}, cards: [{ cardId: 1, kind: 'gate', roleId: 'r1', resolved: false }],
  };
  const approve = rt.defer('approve_workflow_gate');
  const p = rt.api.approveWorkflowGate(1, 'r1');     // 批准 run-a 的 gate
  rt.state.workflow.run = {
    active: true, sessionId: 'run-b', status: 'running',
    agents: {}, cards: [{ cardId: 2, kind: 'gate', roleId: 'r1', resolved: false }],
  };                                                // 新 run 开始（同角色 gate 卡）
  approve.resolve({});
  await p;
  assert.equal(rt.state.workflow.run.cards[0].resolved, false, 'run-b 的 gate 卡不得被旧批准误标');
});

test('rejectWorkflowGate 打回旧 run 不得落到新 run 的卡片上', async () => {
  const rt = loadWorkflowFeature();
  rt.state.workflow.run = {
    active: true, sessionId: 'run-a', status: 'running',
    agents: {}, cards: [{ cardId: 1, kind: 'gate', roleId: 'r1', resolved: false }],
  };
  const reject = rt.defer('reject_workflow_gate');
  const p = rt.api.rejectWorkflowGate(1, 'r1', '返工');
  rt.state.workflow.run = {
    active: true, sessionId: 'run-b', status: 'running',
    agents: {}, cards: [{ cardId: 2, kind: 'gate', roleId: 'r1', resolved: false }],
  };
  reject.resolve({});
  await p;
  assert.equal(rt.state.workflow.run.cards[0].resolved, false, 'run-b 的 gate 卡不得被旧打回误标');
});

test('attachRun 陈旧快照不得覆盖已启动的新 run', async () => {
  const state = {
    activeSessionId: 'chat-a',
    workflow: {
      run: { active: false, sessionId: null, status: 'idle', agents: {}, cards: [], selectedRole: null },
      demo: null, bindings: {}, activeSkillName: null, phases: [], currentPhaseId: null,
      reachedPhaseIds: [], loadState: 'ready', skills: [],
    },
    messages: [], chatItems: [],
  };
  const rt = loadFeature('workflow-runtime.js', state, {
    listen() {},
    refreshHistoryList: async () => {},
  });
  rt.state.workflow.run = { active: true, sessionId: 'run-b', status: 'running', agents: {}, cards: [] };
  const snap = rt.defer('get_workflow_state');
  const p = rt.api.attachRun('run-a');               // 恢复旧 run-a 的快照
  snap.resolve({ roles: { r1: { status: 'running' } }, project_dir: '/p' });
  await p;
  assert.equal(rt.state.workflow.run.sessionId, 'run-b', 'attach 旧 run 不得覆盖新 run-b');
});

test('refreshRunState 陈旧快照不得 merge 进新 run', async () => {
  const state = {
    activeSessionId: 'chat-a',
    workflow: {
      run: { active: false, sessionId: null, status: 'idle', agents: {}, cards: [], selectedRole: null },
      demo: null, bindings: {}, activeSkillName: null, phases: [], currentPhaseId: null,
      reachedPhaseIds: [], loadState: 'ready', skills: [],
    },
    messages: [], chatItems: [],
  };
  const rt = loadFeature('workflow-runtime.js', state, {
    listen() {},
    refreshHistoryList: async () => {},
  });
  rt.state.workflow.run = { active: true, sessionId: 'run-b', status: 'running', agents: {}, cards: [] };
  const snap = rt.defer('get_workflow_state');
  const p = rt.api.refreshRunState();                 // 刷新 run-b（内部捕获 sid=run-b）
  rt.state.workflow.run = { active: true, sessionId: 'run-c', status: 'running', agents: {}, cards: [] }; // 又切到 run-c
  snap.resolve({ roles: { r1: { status: 'complete' } }, stopped: true });
  await p;
  assert.equal(rt.state.workflow.run.status, 'running', '旧 run-b 的 stopped 快照不得 merge 进 run-c');
  assert.deepEqual(rt.state.workflow.run.agents, {}, '旧快照角色不得混入 run-c');
});

test('activateSkill 响应时已新建会话则不得劫持 activeSessionId', async () => {
  const rt = loadWorkflowFeature();
  rt.state.activeSessionId = null;                    // 入口无会话
  const start = rt.defer('start_skill_session');
  const p = rt.api.activateSkill('skill-a');
  rt.state.activeSessionId = 'new-chat';              // await 期间用户新建聊天会话
  start.resolve({ skill: { name: 'skill-a' }, session: { id: 'skill-sess' } });
  await p;
  assert.equal(rt.state.activeSessionId, 'new-chat', '不得劫持用户新建的会话');
  assert.deepEqual(rt.state.messages, [], '消息区未被技能会话清空');
});
