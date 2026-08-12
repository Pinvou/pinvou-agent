/** 多智能体（会话内主动委派，ADR-0006）薄层契约：桥、专家卡、只读面板与取消级联。 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const interactionBridgeSource = fs.readFileSync(path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'interaction.js'), 'utf8');

// ── modeState 竞态回归：陈旧读取不得覆盖权威改写（审计意见）────────
// 装载 interaction 桥的 runtime 快照，用可控 invoke 精确编排异步返回顺序。
// 场景：syncModeState 先发起 get_mode_state（将返回旧值），toggle 先落盘
// （in-flight 清空）后旧读取才返回。只靠瞬时 in-flight 集合识别不了这种
// 顺序——epoch 校验必须在场（审计 P1）。
function loadInteractionRuntime() {
  const root = {};
  vm.runInNewContext(interactionBridgeSource, { window: root, globalThis: root });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__.interaction;
  const state = {
    activeSessionId: 'chat-a',
    modeState: { mode: 'yolo', multiAgent: false },
    pendingDraftMultiAgent: false,
    chatItems: [],
    messages: [],
  };
  const deferred = {};
  const calls = [];
  const runtime = {
    state,
    calls,
    notifyCount: 0,
    defer(name) {
      deferred[name] = deferred[name] || {};
      deferred[name].promise = new Promise((resolve, reject) => {
        deferred[name].resolve = resolve;
        deferred[name].reject = reject;
      });
      return deferred[name];
    },
  };
  runtime.api = factory({
    state,
    notify() { runtime.notifyCount += 1; },
    bt(key) { return key; },
    addSystemItem() {},
    addAuthoritySyncNotice() {},
    addChatItem() {},
    timeStr() { return ''; },
    runSyncOnSession(sid, fn) {
      // 记录跨会话定向调用：sid !== active 时 fn 必须落在 sid 的 buffer 上，
      // 不能直接改当前显示。本 mock 简化执行 fn 但保留调用证据。
      if (sid !== state.activeSessionId) calls.push('runSyncOnSession:' + sid);
      fn();
    },
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    resetPendingAssistant() {},
    rerenderFromMessages() {},
    ensureSession: async () => (state.activeSessionId || 'chat-a'),
    sendMessage: async () => {},
    reconcileRemoteTurn: async () => true,
    isBusyFor() { return false; },
    markRemoteTurn() {},
    turnUsageDirty: {},
    invoke(name, args) {
      calls.push(name);
      if (deferred[name] && deferred[name].promise) return deferred[name].promise;
      return Promise.resolve({ mode: 'yolo', multi_agent: false });
    },
  });
  return runtime;
}

// ── 写入归属回归：await 期间切会话，权威写回/恢复必须定向触发会话 ──
// 与 syncModeState 串台同源的镜像面：读取串台已修，写入串台（切走后把
// A 会话的模式/卡片状态写进 B 的显示）同样必须定向回 sid 的 buffer。
test('exitPlanToYolo 权威写回定向触发会话：await 期间切走不污染当前显示', async () => {
  const rt = loadInteractionRuntime();
  const exit = rt.defer('exit_plan_to_yolo');
  const exitP = rt.api.exitPlanToYolo();                    // A 会话发起，invoke 挂起
  rt.state.activeSessionId = 'chat-b';                      // await 期间切走
  exit.resolve({ mode: 'yolo', multi_agent: false });
  await exitP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'exitPlanToYolo 写回必须定向触发会话 chat-a（修复前直接写全局、无此调用）');
});

test('setPlanModeNext 权威写回定向触发会话：await 期间切走不污染当前显示', async () => {
  const rt = loadInteractionRuntime();
  const setMode = rt.defer('set_plan_mode_next');
  const modeP = rt.api.setPlanModeNext();                   // A 会话发起，invoke 挂起
  rt.state.activeSessionId = 'chat-b';                      // await 期间切走
  setMode.resolve({ mode: 'plan', multi_agent: true });
  await modeP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'setPlanModeNext 写回必须定向触发会话 chat-a');
});

test('submitUserInput 响应写入定向触发会话：切走后 echo/卡片状态不进错会话', async () => {
  const rt = loadInteractionRuntime();
  const submit = rt.defer('submit_user_input');
  const submitP = rt.api.submitUserInput('card-1', 'tool-1',
    [{ label: '是' }], [{ header: '确认执行？' }]);
  rt.state.activeSessionId = 'chat-b';                      // await 期间切走
  submit.resolve({});
  await submitP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'submitUserInput 的 echo/卡片状态写入必须定向回触发会话 chat-a');
});

test('cancelUserInput 响应写入定向触发会话：切走后卡片状态不进错会话', async () => {
  const rt = loadInteractionRuntime();
  const cancel = rt.defer('cancel_user_input');
  const cancelP = rt.api.cancelUserInput('card-1', 'tool-1');
  rt.state.activeSessionId = 'chat-b';                      // await 期间切走
  cancel.resolve({});
  await cancelP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'cancelUserInput 的卡片状态写入必须定向回触发会话 chat-a');
});

test('editLastTurn 失败恢复定向触发会话：切走后 busy/错误提示不砸进当前会话', async () => {
  const rt = loadInteractionRuntime();
  const edit = rt.defer('edit_last_turn');
  const editP = rt.api.editLastTurn('新的文本');             // A 会话发起
  rt.state.activeSessionId = 'chat-b';                      // await 期间切走
  edit.reject(new Error('boom'));
  await editP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'editLastTurn 失败恢复（messages/busy/错误提示）必须定向回发起会话 chat-a');
});
