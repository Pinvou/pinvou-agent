// Web 车道 planStuckGo 的在途闩 + 提交后才落已处理（round-15 评审 D）：
// round-14 的修复只落在 tauri 车道（platform/tauri/bridge/interaction.js），
// web 车道保留旧形态——先 resolved:true 再 await，exit_plan_to_yolo 被拒时
// 卡片已被剥掉动作、补充指令仍无条件发出；等待窗口内第二次点击派发重复的
// 模式切换/重复补充指令。本测试锁 web 车道的镜像修复：同步闩（submitting
// 检查置位）+ 直接 invoke exit_plan_to_yolo（包装层吞错，提交标记需要拒绝）
// + 失败恢复可点并定向提示。运行时回归与 web_queue_admission_fifo 同款
// vm harness；plan_stuck 卡经生产路径（chat:tool_end 的 Plan 拦截兜底卡）创建。
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const webBridgeRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'platform', 'web');
const read = relativePath => fs.readFileSync(path.join(webBridgeRoot, relativePath), 'utf8');
const readSharedHelpers = () => fs.readFileSync(path.join(webBridgeRoot, '..', '..', 'shared', 'bridge-shared-helpers.js'), 'utf8');

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
  const handlers = {
    web_access_list_sessions: () => Promise.resolve([{ id: 'chat-a', title: '新对话', updated_at: 1 }]),
  };
  const listeners = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name, args) => {
        if (handlers[name]) return handlers[name](args);
        return Promise.resolve(null);
      } },
      event: { listen: async (name, fn) => { (listeners[name] = listeners[name] || []).push(fn); return function () {}; } },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-4000-8000-000000000000' },
    performance: { now: () => 0 },
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
  vm.runInContext(readSharedHelpers(), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(read('bridge.js'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  assert.equal(typeof flat.getState, 'function', 'Web transport must expose its private flat state');
  vm.runInContext(read('bridge/domain-adapter.js'), context, { filename: 'platform/web/bridge/domain-adapter.js' });
  return {
    flat,
    view() { return flat.getState(); },
    setHandler(name, fn) { handlers[name] = fn; },
    emit(name, payload) { (listeners[name] || []).forEach(fn => fn({ payload })); },
  };
}

async function primeSessionA(rt) {
  rt.setHandler('web_access_create_session', () => Promise.resolve({ id: 'chat-a', transcript_revision: 1 }));
  rt.setHandler('equip_persona', () => Promise.resolve({ id: 'persona-prime', name: '引导卡' }));
  rt.setHandler('rename_session', () => Promise.resolve({}));
  const p = rt.flat.equipPersona
    ? rt.flat.equipPersona('persona-prime')
    : rt.interaction.equipPersona('persona-prime');
  await new Promise(r => { setTimeout(r, 0); });
  await new Promise(r => { setTimeout(r, 0); });
  await p;
  const view = rt.view();
  assert.equal(view.activeSessionId, 'chat-a', 'precondition: chat-a must be active');
}

const tick = () => new Promise(r => { setTimeout(r, 0); });

// 经生产路径创建 plan_stuck 卡：Plan 模式下被拦工具的 chat:tool_end 兜底卡。
async function armPlanStuckCard(rt) {
  rt.emit('chat:mode_changed', { session_id: 'chat-a', mode: 'plan' });
  await tick(); await tick();
  rt.emit('chat:tool_start', { id: 't-stuck-1', name: 'write_file', args: { path: 'x.txt' } });
  rt.emit('chat:tool_end', { id: 't-stuck-1', success: false, output: 'PermissionDenied: write blocked' });
  const card = rt.view().chatItems.find(item => item.type === 'plan_stuck');
  assert.ok(card, 'precondition: Plan 拦截兜底卡已创建');
  assert.equal(card.resolved, false, 'precondition: 卡初始未处理');
  return card;
}

test('web planStuckGo：在途闩拒绝双击，提交后才落已处理', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const card = await armPlanStuckCard(rt);

  let exitCalls = 0;
  let exitRelease = null;
  rt.setHandler('exit_plan_to_yolo', () => {
    exitCalls += 1;
    return new Promise(resolve => { exitRelease = resolve; });
  });
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'yolo', multi_agent: false }));
  const chatArgs = [];
  rt.setHandler('web_access_chat', args => { chatArgs.push(args); return Promise.resolve({}); });

  // 同一 tick 内两次点击：第二次必须被同步闩挡下。
  const p1 = rt.flat.planStuckGo(card.id, 'chat-a');
  const p2 = rt.flat.planStuckGo(card.id, 'chat-a');
  await tick(); await tick();
  assert.equal(exitCalls, 1, '在途闩：等待窗口内第二次点击不得再派发 exit_plan_to_yolo');
  assert.equal(rt.view().chatItems.find(item => item.id === card.id).resolved, false,
    '提交前不得落 resolved（旧形态先 resolved 再 await）');
  exitRelease({ mode: 'yolo' });
  await p1; await p2;
  await tick(); await tick();
  const settled = rt.view().chatItems.find(item => item.id === card.id);
  assert.equal(settled.resolved, true, '切换提交后才落已处理');
  assert.equal(exitCalls, 1, '全程恰一次模式切换');
  assert.equal(chatArgs.length, 1, '补充指令恰一条');
  // VM harness 无语言偏好，bt() 走 zh 表（与既有 web vm 用例同口径）。
  assert.ok(String(chatArgs[0].message).includes('继续执行任务'),
    '补充指令是 planStuckGoPrompt 文案（harness 默认 zh 表）');
});

test('web planStuckGo：切换被拒时卡片恢复可点、定向提示、不发补充指令', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  const card = await armPlanStuckCard(rt);

  rt.setHandler('exit_plan_to_yolo', () => Promise.reject(new Error('store write failed')));
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', multi_agent: false }));
  const chatArgs = [];
  rt.setHandler('web_access_chat', args => { chatArgs.push(args); return Promise.resolve({}); });

  await rt.flat.planStuckGo(card.id, 'chat-a');
  await tick(); await tick();
  const settled = rt.view().chatItems.find(item => item.id === card.id);
  assert.equal(settled.resolved, false, '被拒后卡片不得落已处理（保持可点可重试）');
  assert.equal(settled.submitting, false, '被拒后释放在途闩');
  assert.equal(chatArgs.length, 0, '被拒后不得发送补充指令 turn');
  const notice = rt.view().chatItems.find(item => item.type === 'system' &&
    String(item.text || '').includes('退出 Plan 失败'));
  assert.ok(notice, '被拒路径落定向失败提示（exitPlanFailed）');
});

console.log('web_plan_stuck_go_latch: ok');
