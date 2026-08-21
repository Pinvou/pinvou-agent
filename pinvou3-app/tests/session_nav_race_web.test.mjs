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
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const webBridgeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');

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
  const deferreds = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name, args) => {
        if (handlers[name]) return handlers[name](args);
        if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
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
    addEventListener() {},
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
    personas: api.personas,
    sessions: api.sessions,
  };
}

// ── ensureSession（经 equipPersona 物化链驱动）：再进草稿不劫持 ──

test('web ensureSession：create_session 等待期间再进草稿 → 不物化、equip 放弃', async () => {
  const rt = bootWebBridge();
  const create = rt.defer('web_access_create_session');
  const equip = rt.defer('equip_persona');
  const p = rt.personas.equipPersona('persona-x'); // 草稿加卡 → ensureSession → create
  rt.leave(); // 慢 create_session 期间用户点了「新建对话」（再进草稿）
  create.resolve({ id: 'chat-new', transcript_revision: 1 });
  await new Promise(r => setTimeout(r, 0)); // ensureSession 返回 null → equipPersona 放弃
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
  await new Promise(r => setTimeout(r, 0));
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
  await new Promise(r => setTimeout(r, 0)); // 物化推进到尾部 refreshHistoryList 并挂起
  rt.leave(); // 尾部 await 期间再进草稿
  list.resolve([]);
  const equip = rt.defer('equip_persona');
  await new Promise(r => setTimeout(r, 0)); // ensureSession 收尾返回 null → equip 放弃
  const view = rt.view();
  assert.equal(view.activeSessionId, null, '尾部窗口的再进草稿同样必须中止物化');
  equip.resolve({ id: 'persona-x', name: 'X' });
  await p;
});

// ── archiveSession：失败回滚不劫持新草稿 ──────────────────────────

// 构造 scheduled run 场景：先 loadScheduledTasks 登记任务（merge 前提），
// 再 loadScheduledTaskRuns 把 sched-run-1 写进 recentRuns（scheduled 分支
// 的前提），最后走 load_session 快速路径激活。
async function primeScheduledRun(rt, sessionId) {
  const tasks = rt.defer('list_scheduled_tasks');
  const loadingTasks = rt.flat.loadScheduledTasks();
  tasks.resolve([{ id: 'task-1', name: 'T' }]);
  await loadingTasks;

  const runs = rt.defer('list_scheduled_task_runs');
  const loading = rt.flat.loadScheduledTaskRuns('task-1');
  runs.resolve([{ id: 'run-1', sessionId, status: 'completed', archived: false }]);
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
  await new Promise(r => setTimeout(r, 0));
  equip.resolve({ id: 'persona-x', name: 'X' });
  list1.resolve([{ id: 'chat-a', title: 'A', updated_at: 1 }]);
  rename.resolve({});
  await priming;
  assert.equal(rt.view().activeSessionId, 'chat-a', 'precondition: chat-a active');

  const list2 = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('chat-a');
  await new Promise(r => setTimeout(r, 0)); // 推进到 leaveSessionView 已执行
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
  await new Promise(r => setTimeout(r, 0));
  equip.resolve({ id: 'persona-x', name: 'X' });
  list1.resolve([{ id: 'chat-a', title: 'A', updated_at: 1 }]);
  rename.resolve({});
  await priming;

  const list2 = rt.defer('list_sessions');
  const set = rt.defer('set_session_archived');
  const p = rt.sessions.archiveSession('chat-a');
  await new Promise(r => setTimeout(r, 0));
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
  await new Promise(r => setTimeout(r, 0));
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
  await new Promise(r => setTimeout(r, 0));
  set.reject(new Error('backend down'));
  list.resolve([]);
  assert.equal(await p, false);
  assert.equal(rt.view().activeSessionId, 'sched-run-1', '无新导航时成对回滚');
});
