/**
 * web 桥（platform/web/bridge.js）会话导航竞态回归测试（PR #257 三审）：
 * ensureSession / archiveSession 的「null → null」导航窗口——再进草稿时
 * activeSessionId 保持 null 但 sessionSwitchRequestToken 已前移，仅判
 * activeSessionId 拦不住在途异步操作劫持新草稿。
 *
 * tauri 侧由 session_nav_race.test.mjs 覆盖；web 桥是独立文本镜像，单独
 * 锁定防止未来只同步一侧时漏改守卫。启动方式复用
 * memory_personas_race_web.test.mjs：vm 整体装载 web 桥 + domain-adapter，
 * invoke 注入 per-command deferred。ensureSession 不在公开 API 面，经
 * personas.equipPersona（草稿加卡 → lazy 物化链）驱动，与生产入口一致。
 *
 * window 必须提供真实配对的 add/removeEventListener：main #315 起
 * loadSessionForClient 先过 waitForWebInvokeCapabilities 门（等待期间注册
 * pinvou:web-capabilities / pinvou:web-connection 监听，落定时移除），空实现
 * 会在 finish 处炸出 removeEventListener is not a function。能力就绪默认
 * 开（对齐 scheduled_tasks_unit.test.js 的 webCapabilitiesReady 默认值），
 * 分块下载路径确定性直行、不进 10s 等待。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const webBridgeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');
// bridge.js delegates shared helpers to window.PinvouBridgeShared (index.html loads the
// shared payload before both bridges); VM harnesses must do the same first.
const readSharedHelpers = () => fs.readFileSync(path.join(webBridgeRoot, '..', '..', 'shared', 'bridge-shared-helpers.js'), 'utf8');
// rerenderFromMessages 的信封判定经 window.PinvouBridgeMessages（同样先于桥加载）。
const readSharedMessages = () => fs.readFileSync(path.join(webBridgeRoot, '..', '..', 'shared', 'bridge-messages.js'), 'utf8');

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
  const handlers = {};
  const listeners = {};
  const windowListeners = {};
  const deferreds = {};
  // main #159 起 web 桥按平台分流命令名（IS_WEB ? "web_access_xxx" : "xxx"）。
  // 测试用例以桌面命令名登记 deferred，这里归一两种名字，两侧语义等价。
  const commandAliases = {
    web_access_list_sessions: 'list_sessions',
    web_access_create_session: 'create_session',
    web_access_load_session_chunk: 'load_session_chunk',
  };
  const resolveCommand = name => commandAliases[name] || name;
  const windowObject = {
    PinvouPlatform: {
      kind: 'web',
      isWeb: true,
      capabilities: {},
      can: () => false,
      canInvoke: () => false,
      areInvokeCapabilitiesReady: () => true,
    },
    __TAURI__: {
      core: { invoke: (name, args) => {
        // 归一名与原始名都查：用例登记 deferred 时两种名字混用
        //（create 用 web_access_* 全名，list_sessions 用桌面短名）。
        const command = resolveCommand(name);
        if (handlers[command]) return handlers[command](args);
        if (handlers[name]) return handlers[name](args);
        const d = deferreds[command] || deferreds[name];
        if (d && d.promise) return d.promise;
        return Promise.resolve(null);
      } },
      event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return function () {}; } },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
    // 分块 transcript 下载用（loadSessionForClient → window.atob）。
    atob: value => Buffer.from(String(value), 'base64').toString('binary'),
    btoa: value => Buffer.from(String(value), 'binary').toString('base64'),
    // 配对的真实监听器行为（非空实现）：#315 的能力等待门依赖
    // addEventListener 注册 / removeEventListener 成对移除。
    addEventListener(name, listener) {
      (windowListeners[name] = windowListeners[name] || []).push(listener);
    },
    removeEventListener(name, listener) {
      windowListeners[name] = (windowListeners[name] || [])
        .filter(candidate => candidate !== listener);
    },
    dispatchEvent(event) {
      [...(windowListeners[event.type] || [])].forEach(listener => listener(event));
      return true;
    },
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
  vm.runInContext(readSharedHelpers(), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(readSharedMessages(), context, { filename: 'shared/bridge-messages.js' });
  vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state');
  vm.runInContext(read('bridge/domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
  const api = windowObject.TauriBridge;
  return {
    flat,
    view() { return flat.getState(); },
    leave() { flat.createNewSession(); }, // enterDraft：activeSessionId → null，零 invoke
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    setHandler(name, fn) { handlers[name] = fn; },
    emit(name, payload) { (listeners[name] || []).forEach(fn => fn({ payload })); },
    fireWindow(type, detail) { windowObject.dispatchEvent({ type, detail }); },
    personas: api.personas,
    sessions: api.sessions,
    interaction: api.interaction,
  };
}

// 一次性回传完整 transcript JSON 的分块下载应答（web 权威加载的唯一通道）。
function chunkResponse(saved) {
  const encoded = Buffer.from(JSON.stringify(saved), 'utf8');
  return {
    offset: 0, total: encoded.length, download_id: 'dl-test',
    data_base64: encoded.toString('base64'), eof: true,
  };
}

test('web cold replay preserves first duplicate tool matching and cached session navigation', async () => {
  const rt = bootWebBridge();
  const loaded = [];
  rt.setHandler('web_access_load_session_chunk', ({ id }) => {
    loaded.push(id);
    return chunkResponse({
      metadata: { id, title: id }, artifacts: [], transcript_revision: 'fixture-revision',
      messages: id === 'replay-A' ? [
        { role: 'user', content: [{ type: 'text', text: 'Restore tool history' }] },
        { role: 'assistant', content: [
          { type: 'tool_use', id: 'duplicate', name: 'Bash', input: { command: 'first' } },
          { type: 'tool_use', id: 'duplicate', name: 'Bash', input: { command: 'second' } },
          { type: 'tool_use', id: 'failure', name: 'Bash', input: { command: 'fail' } },
          { type: 'tool_use', id: 'pending', name: 'Bash', input: { command: 'pending' } },
        ] },
        { role: 'user', content: [
          { type: 'tool_result', tool_use_id: 'duplicate', content: 'complete' },
          { type: 'tool_result', tool_use_id: 'failure', content: 'failed', is_error: true },
        ] },
        { role: 'assistant', content: [{ type: 'text', text: 'Restored' }] },
      ] : [{ role: 'user', content: [{ type: 'text', text: 'Other session' }] }],
    });
  });
  assert.equal(await rt.sessions.switchToSession('replay-A'), true);
  const rows = () => Array.from(rt.view().chatItems.filter(item => item.type === 'tool'), item => ({
    command: item.args.command, output: item.output, state: item.state, success: item.success,
  }));
  const expected = [
    { command: 'first', output: 'complete', state: 'done', success: true },
    { command: 'second', output: null, state: 'pending', success: null },
    { command: 'fail', output: 'failed', state: 'failed', success: false },
    // Web's existing replay hook leaves an unresolved historical tool pending.
    { command: 'pending', output: null, state: 'pending', success: null },
  ];
  assert.deepEqual(rows(), expected);
  assert.equal(await rt.sessions.switchToSession('replay-B'), true);
  assert.equal(await rt.sessions.switchToSession('replay-A'), true);
  assert.deepEqual(rows(), expected);
  assert.deepEqual(loaded, ['replay-A', 'replay-B'], 'returning to A must use its cached buffer');
});

// 轮询至条件成立或超时（对账/物化链跨多个宏任务，不能单次 tick 断言）。
async function waitFor(predicate, label) {
  const deadline = Date.now() + 2000;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise(resolve => { setTimeout(resolve, 10); });
  }
  assert.ok(predicate(), label);
}

// ── ensureSession（经 equipPersona 物化链驱动）：再进草稿不劫持 ──

test('web ensureSession：create_session 等待期间再进草稿 → 不物化、equip 放弃', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const p = rt.personas.equipPersona('persona-x'); // 草稿加卡 → ensureSession → create
  rt.leave(); // 慢 create_session 期间用户点了「新建对话」（再进草稿）
  create.resolve({ id: 'chat-new', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); }); // ensureSession 返回 null → equipPersona 放弃
  const view = rt.view();
  assert.equal(view.activeSessionId, null, '不得劫持用户的新草稿');
  assert.equal(view.activePersona, null, '卡不得挂进新草稿');
  equip.resolve({ id: 'persona-x', name: 'X' }); // 后台不该有人消费；防御性 resolve 防挂起
  await p;
});

test('web ensureSession：无导航 → 正常物化（既有行为保持）', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const list = rt.defer('list_sessions');
  const rename = rt.defer('rename_session');
  const p = rt.personas.equipPersona('persona-x');
  create.resolve({ id: 'chat-ok', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); });
  equip.resolve({ id: 'persona-x', name: 'X' });
  list.resolve([]);
  rename.resolve({});
  await p;
  const view = rt.view();
  assert.equal(view.activeSessionId, 'chat-ok');
  assert.equal(view.activePersona && view.activePersona.id, 'persona-x');
});

test('web ensureSession：尾部 await 期间再进草稿 → 物化中止不漂移', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const list = rt.defer('list_sessions'); // 挂住尾部链，制造真实 await 窗口
  const p = rt.personas.equipPersona('persona-x');
  create.resolve({ id: 'chat-tail', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); }); // 物化推进到尾部 refreshHistoryList 并挂起
  rt.leave(); // 尾部 await 期间再进草稿
  list.resolve([]);
  const equip = rt.defer('equip_persona');
  await new Promise(r => { setTimeout(r, 0); }); // ensureSession 收尾返回 null → equip 放弃
  const view = rt.view();
  assert.equal(view.activeSessionId, null, '尾部窗口的再进草稿同样必须中止物化');
  equip.resolve({ id: 'persona-x', name: 'X' });
  await p;
});

// ── archiveSession：失败回滚不劫持新草稿 ──────────────────────────

// Build the scheduled-run scenario: register the task via loadScheduledTasks
// first (a prerequisite of the merge), then write sched-run-1 into recentRuns
// via loadScheduledTaskRecentRuns (a prerequisite of the scheduled branch;
// loadScheduledTaskRuns was retired from the flat facade, and the two paths
// merge identically into recentRuns), and finally activate through the
// load_session fast path.
async function primeScheduledRun(rt, sessionId) {
  const tasks = rt.defer('list_scheduled_tasks');
  const loadingTasks = rt.flat.loadScheduledTasks();
  tasks.resolve([{ id: 'task-1', name: 'T' }]);
  await loadingTasks;

  const runs = rt.defer('list_scheduled_runs');
  const loading = rt.flat.loadScheduledTaskRecentRuns();
  runs.resolve([{ id: 'run-1', automationId: 'task-1', sessionId, status: 'completed', archived: false }]);
  await loading;
  assert.ok(
    (rt.view().scheduledTaskRecentRuns || []).some(run => run && run.sessionId === sessionId),
    'precondition: sched run recorded in recent runs',
  );
  // web 版切换走分块下载：一次性回传完整 transcript JSON（eof=true）。
  const chunk = rt.defer('web_access_load_session_chunk');
  const switching = rt.flat.switchToSession(sessionId);
  const saved = { metadata: { id: sessionId, title: 'R' }, messages: [], artifacts: [], transcript_revision: 1 };
  const encoded = Buffer.from(JSON.stringify(saved), 'utf8');
  chunk.resolve({
    offset: 0, total: encoded.length, download_id: 'dl-1',
    data_base64: encoded.toString('base64'), eof: true,
  });
  await switching;
  assert.equal(rt.view().activeSessionId, sessionId, 'precondition: sched run active');
}

test('web archiveSession（普通会话）：归档失败但等待期间再进草稿 → 不回滚 active', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const list1 = rt.defer('list_sessions');
  const rename = rt.defer('rename_session');
  const priming = rt.personas.equipPersona('persona-x'); // 物化并激活 chat-a
  create.resolve({ id: 'chat-a', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); });
  equip.resolve({ id: 'persona-x', name: 'X' });
  list1.resolve([{ id: 'chat-a', title: 'A', updated_at: 1 }]);
  rename.resolve({});
  await priming;
  assert.equal(rt.view().activeSessionId, 'chat-a', 'precondition: chat-a active');

  const list2 = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('chat-a');
  await new Promise(r => { setTimeout(r, 0); }); // 推进到 leaveSessionView 已执行
  assert.equal(rt.view().activeSessionId, null);
  rt.leave(); // 归档等待期间用户再进草稿：token 前移、active 仍为 null
  set.reject(new Error('backend down'));
  list2.resolve([]);
  assert.equal(await p, false);
  assert.equal(rt.view().activeSessionId, null, '失败回滚不得把用户拽回归档会话（劫持新草稿）');
  assert.equal(rt.view().sessions.length, 1, '会话列表仍按失败语义回滚恢复');
});

test('web archiveSession（普通会话）：归档失败且无导航 → 回滚 active（既有行为保持）', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const list1 = rt.defer('list_sessions');
  const rename = rt.defer('rename_session');
  const priming = rt.personas.equipPersona('persona-x');
  create.resolve({ id: 'chat-a', transcript_revision: 1 });
  await new Promise(r => { setTimeout(r, 0); });
  equip.resolve({ id: 'persona-x', name: 'X' });
  list1.resolve([{ id: 'chat-a', title: 'A', updated_at: 1 }]);
  rename.resolve({});
  await priming;

  const list2 = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('chat-a');
  await new Promise(r => { setTimeout(r, 0); });
  set.reject(new Error('backend down'));
  list2.resolve([]);
  assert.equal(await p, false);
  assert.equal(rt.view().activeSessionId, 'chat-a', '无新导航时仍应回滚恢复 active');
});

test('web archiveSession（scheduled run）：归档失败但等待期间再进草稿 → 不回滚 active/context', async () => {
  const rt = bootWebBridge();
  await primeScheduledRun(rt, 'sched-run-1');

  const list = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('sched-run-1');
  await new Promise(r => { setTimeout(r, 0); });
  assert.equal(rt.view().activeSessionId, null);
  rt.leave(); // 等待期间再进草稿
  set.reject(new Error('backend down'));
  list.resolve([]);
  assert.equal(await p, false);
  assert.equal(rt.view().activeSessionId, null, '失败回滚不得劫持新草稿');
});

test('web archiveSession（scheduled run）：归档失败且无导航 → active/context 成对回滚（既有行为保持）', async () => {
  const rt = bootWebBridge();
  await primeScheduledRun(rt, 'sched-run-1');

  const list = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('sched-run-1');
  await new Promise(r => { setTimeout(r, 0); });
  set.reject(new Error('backend down'));
  list.resolve([]);
  assert.equal(await p, false);
  assert.equal(rt.view().activeSessionId, 'sched-run-1', '无新导航时成对回滚');
});

// ── 权威重载/对账的 tauri 对齐回归（FE14 分歧修复）──────────────────

test('web 权威重载：旧 steered 消息携带的 <turn_meta> 信封必须在用户气泡中剥除', async () => {
  const rt = bootWebBridge();
  rt.setHandler('web_access_load_session_chunk', () => chunkResponse({
    metadata: { id: 'chat-meta', title: 'M' },
    messages: [
      { role: 'user', content: [
        { type: 'text', text: '接着刚才的方案继续' },
        // 持久化对齐前保存的会话在 steered 消息上带引擎烘焙的 turn_meta 块。
        { type: 'text', text: '<turn_meta>\n{"steer":"mid-turn"}\n</turn_meta>' },
      ] },
    ],
    artifacts: [],
    transcript_revision: 1,
  }));
  await rt.sessions.switchToSession('chat-meta');
  const bubble = rt.view().chatItems.find(item => item.type === 'user');
  assert.ok(bubble, 'precondition: hydrated user bubble');
  assert.ok(!String(bubble.text).includes('<turn_meta>'),
    `rehydrated bubble must strip engine envelopes, got: ${bubble.text}`);
  assert.ok(String(bubble.text).includes('接着刚才的方案继续'),
    'the visible text must survive the envelope strip');
});

test('web 权威对账：运行中的后台 shell 卡必须在 reconcile 后保留（tauri 对齐）', async () => {
  const rt = bootWebBridge();
  rt.setHandler('web_access_load_session_chunk', () => chunkResponse({
    metadata: { id: 'chat-bg', title: 'B' },
    messages: [],
    artifacts: [],
    transcript_revision: 'rev-0',
  }));
  await rt.sessions.switchToSession('chat-bg');
  // shell 轮询空转快速耗尽等待预算，避免测试尾部悬挂真实定时器。
  rt.setHandler('list_shell_tasks', () => []);
  rt.emit('chat:tool_start', {
    session_id: 'chat-bg', id: 't-1', name: 'exec_shell', args: { command: 'npm run dev' },
  });
  rt.emit('chat:tool_end', {
    session_id: 'chat-bg', id: 't-1', success: true, output: 'running',
    metadata: { task_id: 'job-1', backgrounded: true, status: 'running' },
  });
  const live = rt.view().chatItems.find(item => item.type === 'tool' && item.toolId === 't-1');
  assert.ok(live && live.background === true && live.state === 'running',
    'precondition: live running background shell card');
  // 远端回合终点 → 权威屏障；重连事件触发 reconcileRemoteTurn 权威重载。
  rt.emit('chat:done', { session_id: 'chat-bg' });
  const durableMessages = [
    { role: 'user', content: [{ type: 'text', text: '起一个 dev 服务' }] },
    { role: 'assistant', content: [
      { type: 'tool_use', id: 't-1', name: 'exec_shell', input: { command: 'npm run dev' } },
    ] },
    { role: 'user', content: [
      { type: 'tool_result', tool_use_id: 't-1', content: 'running' },
    ] },
  ];
  rt.setHandler('web_access_load_session_chunk', () => chunkResponse({
    metadata: { id: 'chat-bg', title: 'B' },
    messages: durableMessages,
    artifacts: [],
    transcript_revision: 'rev-1',
  }));
  rt.fireWindow('pinvou:web-connection', { status: 'connected' });
  // 权威重载完成的判据：messages 换成持久化终稿（含用户文本气泡）。
  // 没有它，对账失败/未运行时旧卡原样保留会让断言空转通过。
  await waitFor(() => rt.view().messages.length === 3,
    'precondition: authoritative reconcile completed (durable transcript loaded)');
  const survivor = rt.view().chatItems.find(item => item.type === 'tool' &&
    item.background === true && item.state === 'running' && item.toolId === 't-1');
  assert.ok(survivor,
    '权威重载不得丢弃仍在运行的后台 shell 卡（transcript 无 taskId/background，无法重建）');
});

test('web discardPlan：成功路径必须回写 planResolutionConfirmed（tauri 对齐）', async () => {
  const rt = bootWebBridge();
  rt.setHandler('web_access_load_session_chunk', () => chunkResponse({
    metadata: { id: 'chat-plan', title: 'P' },
    messages: [],
    artifacts: [],
    transcript_revision: 1,
  }));
  await rt.sessions.switchToSession('chat-plan');
  rt.setHandler('get_mode_state', () => ({ mode: 'plan' }));
  rt.setHandler('discard_plan', () => ({ mode: 'plan' }));
  rt.emit('chat:plan_ready', {
    session_id: 'chat-plan',
    plan_id: 'plan-ticket-1',
    mode_state: { mode: 'plan' },
    plan_snapshot: { items: [{ step: 'do it', status: 'pending' }] },
  });
  const plan = rt.view().chatItems.find(item => item.type === 'plan_card');
  assert.ok(plan && plan.cardState === 'active', 'precondition: actionable plan card');
  assert.equal(plan.planResolutionConfirmed, false, 'precondition: not yet confirmed');
  await rt.interaction.discardPlan(plan.id, plan.planId);
  await waitFor(() => {
    const card = rt.view().chatItems.find(item => item.type === 'plan_card');
    return card && card.planResolutionConfirmed === true;
  }, '成功放弃方案后必须确认卡片已决议；否则 catch 复位守卫会把已放弃卡复活为 active');
});
