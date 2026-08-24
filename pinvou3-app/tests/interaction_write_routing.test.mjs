/** interaction 桥「写入定向触发会话」契约：await 挂起期间切走，权威写回/恢复/提示必须落回触发会话。 */
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
    busy: false,
    thinking: {},
  };
  const deferred = {};
  const calls = [];
  const epochTable = {};                                    // 与 bridge.js 共享 epoch 表同构（#263）
  // 简化 mock：真实 runSyncOnSession（bridge.js）会 swap 到 sid 的工作集执行
  // 并在结束后 restore 当前显示，且 sid 无 buffer 时静默丢弃；本 mock 只记录
  // 定向调用证据后直接执行 fn。因此下方对 state.modeState 的断言验证的是
  // 「fn 携权威 st 被执行」，非端到端显示语义。
  function runSyncMock(sid, fn) {
    if (sid !== state.activeSessionId) calls.push('runSyncOnSession:' + sid);
    fn();
  }
  const runtime = {
    state,
    calls,
    errorItems: [],
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
    addSystemItem(text) { runtime.errorItems.push(String(text)); },
    addAuthoritySyncNotice() {},
    addChatItem() {},
    timeStr() { return ''; },
    runSyncOnSession: runSyncMock,
    // #263 注入的权威写回收敛点（bridge.js）：任何权威 modeState 写回必须经它
    // （bump epoch + 定向写）。mock 镜像真实实现，走 runSyncMock 保留路由证据。
    modeStateEpochs: epochTable,
    bumpModeStateEpoch(sid) { epochTable[sid] = (epochTable[sid] || 0) + 1; },
    applyAuthoritativeModeState(sid, st) {
      epochTable[sid] = (epochTable[sid] || 0) + 1;
      runSyncMock(sid, function () {
        state.modeState = { mode: st.mode || 'yolo', multiAgent: !!st.multi_agent };
      });
    },
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    resetPendingAssistant() {},
    rerenderFromMessages() {},
    currentStreamText: '',
    currentStreamId: 0,
    itemIdSeq: 1000,
    ensureSession: async () => (state.activeSessionId || 'chat-a'),
    sendMessage: async () => {},
    sendMessageToSession: async (sid) => {
      calls.push('sendMessageToSession:' + sid);
      if (runtime.failSendMessageToSession) throw new Error('target session missing');
    },
    reconcileRemoteTurn: async () => true,
    isBusyFor() { return false; },
    markRemoteTurn() {},
    turnUsageDirty: {},
    // eslint-disable-next-line no-unused-vars -- 桩函数保留完整调用签名
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
  exit.resolve({ mode: 'yolo', multi_agent: true });
  await exitP;
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    'exitPlanToYolo 写回必须定向触发会话 chat-a（修复前直接写全局、无此调用）');
  // 成功路径必须真实执行：#250 的 bumpModeStateEpoch 缺失时会被 ReferenceError
  // 打成失败提示（历史假阳性：错误路径也记录 runSyncOnSession，仅靠上面断言无法区分）。
  assert.equal(rt.errorItems.length, 0,
    '成功路径不得走错误分支（不得出现 exitPlanFailed + ReferenceError 提示）');
  assert.equal(rt.state.modeState.multiAgent, true,
    '权威写回必须被应用（成功路径的 applyModeFromState 生效）');
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
  assert.equal(rt.errorItems.length, 0,
    '成功路径不得走错误分支（不得出现 switchModeFailed + ReferenceError 提示）');
  assert.equal(rt.state.modeState.mode, 'plan',
    '权威写回必须被应用（成功路径的 applyModeFromState 生效）');
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
  // 恢复内容：messages 回滚到编辑前快照、busy 复位、错误提示恰好落一次。
  assert.equal(rt.state.messages.length, 0,
    '失败后 messages 必须回滚到快照（编辑重跑的乐观 user 消息被撤销）');
  assert.equal(rt.state.busy, false,
    '失败后 busy 必须复位（否则触发会话被永久卡成忙碌）');
  assert.equal(rt.errorItems.length, 1,
    '失败提示恰好一条（"⚠️ Error: boom"，落进定向恢复的会话）');
});

test('planStuckGo 补充指令失败定向提示：sendMessageToSession 抛错不得成为 unhandled rejection', async () => {
  const rt = loadInteractionRuntime();
  rt.failSendMessageToSession = true;                        // 模拟目标会话已删/对账中
  const goP = rt.api.planStuckGo('card-1');                  // A 会话发起
  rt.state.activeSessionId = 'chat-b';                       // await 期间切走
  await goP;                                                 // 无 catch 时此处即 rejected（用例直接红）
  assert.ok(rt.calls.includes('sendMessageToSession:chat-a'),
    '补充指令必须发往触发会话 chat-a');
  assert.ok(rt.calls.includes('runSyncOnSession:chat-a'),
    '失败提示必须定向回触发会话（不得落进当前显示的 chat-b）');
  assert.equal(rt.errorItems.length, 1,
    '失败提示恰好一条（planContinueFailed + 原因）');
  assert.ok(String(rt.errorItems[0]).includes('planContinueFailed'),
    '失败提示必须携带 planContinueFailed 文案而非静默吞掉');
});
