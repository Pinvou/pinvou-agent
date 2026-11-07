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
  vm.runInNewContext(fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8') + '\n' + interactionBridgeSource, { window: root, globalThis: root });
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
    invokeCalls: [],
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
  // #263 注入的权威写回收敛点（bridge.js）：任何权威 modeState 写回必须经它
  // （bump epoch + 定向写）。mock 镜像真实实现的 tri-mode 收敛形状（ask 缺省
  // 开、multi_agent 缺失保留当前显示），走 runSyncMock 保留路由证据。它同时
  // 是 chat:mode_changed 广播的落点（chat-events 处理器只做
  // applyAuthoritativeModeState(sid, payload)），经 runtime.broadcastModeState
  // 暴露给用例，模拟「广播在重读窗口内落地」。
  function applyAuthoritativeModeStateMock(sid, st) {
    epochTable[sid] = (epochTable[sid] || 0) + 1;
    runSyncMock(sid, function () {
      state.modeState = {
        mode: st.mode || 'auto',
        multiAgent: st.multi_agent === undefined
          ? !!(state.modeState && state.modeState.multiAgent)
          : !!st.multi_agent,
        askOnGuardrail: st.ask_on_guardrail == null ? true : !!st.ask_on_guardrail,
      };
    });
  }
  runtime.broadcastModeState = applyAuthoritativeModeStateMock;
  runtime.api = factory({
    state,
    notify() { runtime.notifyCount += 1; },
    bt(key) { return key; },
    addSystemItem(text) { runtime.errorItems.push(String(text)); },
    addAuthoritySyncNotice() {},
    addChatItem() {},
    timeStr() { return ''; },
    runSyncOnSession: runSyncMock,
    modeStateEpochs: epochTable,
    bumpModeStateEpoch(sid) { epochTable[sid] = (epochTable[sid] || 0) + 1; },
    applyAuthoritativeModeState: applyAuthoritativeModeStateMock,
    getBuffer() { return null; },
    flushAssistantMessageToHistory() {},
    resetPendingAssistant() {},
    rerenderFromMessages() {},
    currentStreamText: '',
    currentStreamId: 0,
    itemIdSeq: 1000,
    // 草稿态显示解析（与 bridge.js currentDraftModeState 同构，未绑定缺省 auto）：
    // refreshModeDefaults/setDraftMode 的草稿臂会调用它，缺失会把被测分支打成
    // ReferenceError 而非断言失败。
    currentDraftModeState() {
      const lane = state.modeLane === 'code' ? 'code' : 'work';
      const d = state.modeDefaults && state.modeDefaults[lane];
      return { mode: d || 'auto', multiAgent: false };
    },
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
    invoke(name, args) {
      calls.push(name);
      runtime.invokeCalls.push({ name, args });
      const q = runtime.invokeQueue && runtime.invokeQueue[name];
      if (q && q.length) return q.shift();
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

// The YOLO confirmation gate's final action must explicitly target the verdict
// sid. The gating path starts the switch after several awaits, and
// activeSessionIdRef (a render-time mirror) can lag the bridge store by a
// frame; the argument-less exitPlanToYolo reads the live active at call time
// and would swap the target for the post-switch session (bound B flipped to
// Yolo with no card). The verdict sid must be passed to invoke as a parameter,
// not rely on the live active at call time.
test('exitPlanToYolo 支持显式裁决 sid：invoke 与权威写回都作用于传入会话', async () => {
  const rt = loadInteractionRuntime();
  const exit = rt.defer('exit_plan_to_yolo');
  const exitP = rt.api.exitPlanToYolo('chat-gate');         // gate starts from the verdict sid
  rt.state.activeSessionId = 'chat-b';                      // live active is a different session at call time
  exit.resolve({ mode: 'yolo', multi_agent: true });
  await exitP;
  const exitCall = rt.invokeCalls.find(c => c.name === 'exit_plan_to_yolo');
  assert.ok(exitCall, 'exit_plan_to_yolo 必须被调用');
  // Assert with raw values rather than deepEqual: the bridge is loaded in a
  // separate vm context and the args object's prototype comes from another
  // realm, so deepStrictEqual would misreport.
  assert.equal(exitCall.args && exitCall.args.sessionId, 'chat-gate',
    '显式裁决 sid 必须直达 invoke 参数（修复前取实时 active chat-b）');
  assert.ok(rt.calls.includes('runSyncOnSession:chat-gate'),
    '权威写回必须定向裁决会话 chat-gate');
});

// R9 MAJOR 1: the plan-stuck card's Go must also target the verdict sid. That
// path used to finish with an argument-less exitPlanToYolo() — a bound regular
// session went from the plan_stuck card straight to Yolo, bypassing the one-time
// confirm gate (adjudicated in ChatView), and acted on the wrong session when
// the live active had changed. The explicit sid must drive both the exit command
// and the follow-up instruction.
test('planStuckGo 显式裁决 sid：exit 命令与补充指令都作用于传入会话', async () => {
  const rt = loadInteractionRuntime();
  const exit = rt.defer('exit_plan_to_yolo');
  const goP = rt.api.planStuckGo('card-9', 'chat-gate');    // gate starts from the verdict sid
  rt.state.activeSessionId = 'chat-b';                      // live active is a different session at call time
  exit.resolve({ mode: 'yolo', multi_agent: false });
  await goP;
  const exitCall = rt.invokeCalls.find(c => c.name === 'exit_plan_to_yolo');
  assert.ok(exitCall, 'exit_plan_to_yolo 必须被调用');
  assert.equal(exitCall.args && exitCall.args.sessionId, 'chat-gate',
    'exit_plan_to_yolo 必须按裁决 sid 定向(修复前无参取实时 active chat-b)');
  assert.ok(rt.calls.includes('sendMessageToSession:chat-gate'),
    '补充指令必须发往裁决会话 chat-gate');
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

// ── 评审 P2/P3 修复回归：卡片可重试 + 失败后权威收敛 ──────────────────
test('planStuckGoAuto 切换失败不落 resolved：卡保持可重试', async () => {
  const rt = loadInteractionRuntime();
  rt.state.chatItems.push({ id: 77, type: 'planStuck', resolved: false, cardState: 'active' });
  const setMode = rt.defer('set_session_mode');
  const p = rt.api.planStuckGoAuto(77, 'chat-a');
  setMode.reject(new Error('engine gone'));
  await p;
  const item = rt.state.chatItems.find(i => i.id === 77);
  assert.equal(item.resolved, false,
    'set_session_mode 失败不得把卡标成已处理（旧形态先 resolved:true 再早退，渲染层剥掉动作按钮后无重试路径）');
  assert.ok(rt.errorItems.length >= 1, '失败必须落系统提示');
  assert.ok(!rt.calls.some(c => String(c).startsWith('sendMessageToSession')),
    '切换失败后不得继续发补充指令');
});

test('planStuckGoAuto 成功后才落 resolved 并发补充指令', async () => {
  const rt = loadInteractionRuntime();
  rt.state.chatItems.push({ id: 78, type: 'planStuck', resolved: false, cardState: 'active' });
  const setMode = rt.defer('set_session_mode');
  const p = rt.api.planStuckGoAuto(78, 'chat-a');
  setMode.resolve({ mode: 'auto', multi_agent: false });
  await p;
  const item = rt.state.chatItems.find(i => i.id === 78);
  assert.equal(item.resolved, true, '切换提交成功后卡才落已处理');
  assert.ok(rt.calls.includes('sendMessageToSession:chat-a'), '成功后发补充指令');
});

// ── 在途防重入（decideApproval 同款闩）：等待窗口横跨后端往返，双击的第
// 二次点击若放行会派发重复的切换命令，并把补充指令重复排队成独立 turn。
test('planStuckGoAuto 在途闩：双击的第二次点击不派发重复切换与重复补充指令', async () => {
  const rt = loadInteractionRuntime();
  rt.state.chatItems.push({ id: 79, type: 'planStuck', resolved: false, cardState: 'active' });
  const setMode = rt.defer('set_session_mode');
  const first = rt.api.planStuckGoAuto(79, 'chat-a');
  const second = rt.api.planStuckGoAuto(79, 'chat-a'); // 首次仍在途
  setMode.resolve({ mode: 'auto', multi_agent: false });
  await first;
  await second;
  const switches = rt.invokeCalls.filter(c => c.name === 'set_session_mode');
  assert.equal(switches.length, 1, '在途窗口内的双击只允许一次 set_session_mode');
  const sends = rt.calls.filter(c => c === 'sendMessageToSession:chat-a');
  assert.equal(sends.length, 1, '补充指令恰好一条（放行第二次点击会排队成重复 turn）');
});

// 旧形态先 resolved:true 再 await exit_plan_to_yolo（无 try/catch）：切换被
// 拒时卡片已被剥掉动作、异常成为 onClick 上的 unhandled rejection。与
// planStuckGoAuto 的评审 P2 形态对齐：提交后才落已处理，失败恢复可点。
test('planStuckGo 切换失败不落 resolved：卡保持可重试并定向提示', async () => {
  const rt = loadInteractionRuntime();
  rt.state.chatItems.push({ id: 80, type: 'planStuck', resolved: false, cardState: 'active' });
  const exit = rt.defer('exit_plan_to_yolo');
  const p = rt.api.planStuckGo(80, 'chat-a');
  exit.reject(new Error('engine gone'));
  await p;
  const item = rt.state.chatItems.find(i => i.id === 80);
  assert.equal(item.resolved, false, '切换失败不得把卡标成已处理（旧形态先标已处理再裸 await）');
  assert.equal(item.submitting, false, '失败后在途闩必须释放（卡可重试）');
  assert.ok(rt.errorItems.length >= 1, '失败必须落系统提示');
  assert.ok(!rt.calls.some(c => String(c).startsWith('sendMessageToSession')),
    '切换失败后不得继续发补充指令');
});

test('setAskOnGuardrail 失败后重读 get_mode_state 收敛权威态', async () => {
  const rt = loadInteractionRuntime();
  const setAsk = rt.defer('set_ask_on_guardrail');
  const p = rt.api.setAskOnGuardrail('chat-a', true);
  setAsk.reject(new Error('persist failed'));
  await p;
  const reReads = rt.invokeCalls.filter(c => c.name === 'get_mode_state');
  assert.ok(reReads.length >= 1,
    '失败路径必须重读权威态（两个相反翻转都失败时各自回放捕获的乐观值，芯片会与持久值背离）');
});

// ── round-7：失败重读的 epoch 护栏 ──────────────────────────────────
// 场景：toggle 失败 → 回滚 → 重读在途窗口内，chat:mode_changed 权威广播落地
// （bump epoch + 经收敛点写回）。随后重读返回的是广播前的陈旧快照：epoch
// 校验必须丢弃它，否则旧读取把广播值打回 pre-click 值，直到下一次事件。
test('setAskOnGuardrail 失败重读窗口内落地的 mode_changed 广播存活', async () => {
  const rt = loadInteractionRuntime();
  rt.state.modeState = { mode: 'yolo', multiAgent: false, askOnGuardrail: false };
  const setAsk = rt.defer('set_ask_on_guardrail');
  const reRead = rt.defer('get_mode_state');
  const p = rt.api.setAskOnGuardrail('chat-a', true);       // 乐观翻转 ask=true
  setAsk.reject(new Error('persist failed'));               // 失败 → 回滚 ask=false
  // setImmediate 排空微任务：跨 realm await 多耗 tick，须确保 catch 回滚已
  // 落地、重读已发起，广播才算落在「重读窗口内」。
  await new Promise(resolve => {
    setImmediate(resolve);
  });
  // 广播在重读窗口内落地（负载形状同 chat:mode_changed：mode/ask_on_guardrail）。
  rt.broadcastModeState('chat-a', { mode: 'plan', ask_on_guardrail: true });
  reRead.resolve({ mode: 'yolo', multi_agent: false, ask_on_guardrail: false }); // 陈旧快照
  await p;
  assert.equal(rt.state.modeState.askOnGuardrail, true,
    '广播值必须存活：陈旧重读快照被 epoch 校验丢弃（无护栏时被回写为 pre-click 值 false）');
  assert.equal(rt.state.modeState.mode, 'plan', '广播写回的 mode 同样不得被陈旧快照覆盖');
  assert.equal(rt.errorItems.length, 1, '失败提示恰好一条（重读护栏不改变失败路径的回滚与提示）');
});

// ── round-10 补钉：modeDefaultsEpoch 纪律（round-8 L7-2 / round-9 写响应序）──
// 在途 get_mode_defaults 与陈旧的 set_mode_default 响应都不得覆盖更新的
// 草稿默认写。此前的护栏完全无测试：删掉 epoch bump 或两处比较，全套保持绿。
function microtaskFlush(n = 6) {
  let p = Promise.resolve();
  for (let i = 0; i < n; i += 1) p = p.then(() => {});
  return p;
}

test('setDraftMode 使在途 get_mode_defaults 失效：陈旧读取不得落盘新写之前的默认值', async () => {
  const rt = loadInteractionRuntime();
  rt.state.activeSessionId = null;               // 草稿态
  rt.state.modeLane = 'work';
  const staleRead = Promise.withResolvers();
  const writeResp = Promise.withResolvers();
  rt.invokeQueue = {
    get_mode_defaults: [staleRead.promise],
    set_mode_default: [writeResp.promise],
  };
  const readP = rt.api.refreshModeDefaults();    // epochAtRead = 1
  const writeP = rt.api.setDraftMode('plan');    // 入口 bump → epoch = 2, epochAtWrite = 2
  await microtaskFlush();                        // 两边都到达各自 await
  assert.deepEqual(rt.invokeCalls.map((c) => c.name), ['get_mode_defaults', 'set_mode_default'],
    '前提：在途读与写都已发出');
  staleRead.resolve({ work: 'yolo', code: 'yolo' });   // 陈旧读先返回
  await readP;
  assert.ok(!rt.state.modeDefaults || rt.state.modeDefaults.work !== 'yolo',
    '陈旧读取不得写入 modeDefaults（写已 bump epoch 使其失效）');
  assert.ok(!rt.state.modeDefaults || rt.state.modeDefaults.work === undefined,
    '陈旧读取被整批丢弃，不得部分落地');
  writeResp.resolve({ work: 'plan', code: 'yolo' });   // 本次写自己的响应（epoch 仍匹配）
  await writeP;
  assert.equal(rt.state.modeDefaults.work, 'plan', '写自身的响应按当前 epoch 落地');
  assert.equal(rt.state.modeState.mode, 'plan', '草稿态显示跟随已提交的写');
});

test('并发两次 setDraftMode：先落盘的后写生效，陈旧的先写响应不得回写', async () => {
  const rt = loadInteractionRuntime();
  rt.state.activeSessionId = null;
  rt.state.modeLane = 'work';
  const respA = Promise.withResolvers();
  const respB = Promise.withResolvers();
  rt.invokeQueue = { set_mode_default: [respA.promise, respB.promise] };
  const pa = rt.api.setDraftMode('yolo');        // bump → 1, epochAtWrite = 1
  const pb = rt.api.setDraftMode('plan');        // bump → 2, epochAtWrite = 2
  await microtaskFlush();
  respB.resolve({ work: 'plan', code: 'yolo' }); // 后写先返回
  respA.resolve({ work: 'yolo', code: 'yolo' }); // 先写后返回（陈旧响应）
  await Promise.all([pa, pb]);
  assert.equal(rt.state.modeDefaults.work, 'plan',
    '后写的默认值必须存活，不得被先写的陈旧响应覆盖');
  assert.equal(rt.state.modeState.mode, 'plan',
    '草稿显示停留在后写（陈旧响应的草稿臂同样被 epoch 拦下）');
});
