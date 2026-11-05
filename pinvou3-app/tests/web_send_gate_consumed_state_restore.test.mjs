// Web 姿态门拒绝路径的已消费 UI 轮次状态恢复（评审 zhuowp 5785283622 P2-2）：
// sendMessage 在受理前消费 pending task guide（隐藏引导词 + restrictTools）
// 与 active skill；Auto 姿态门 throw（而非 resolve false）时，消费掉的状态
// 必须恢复——拒绝后换支持档位重试仍携带隐藏引导与工具限制，否则任务创建
// 协议只剩可见文本。运行时回归与 chat_bound_workspace_logic /
// web_accept_plan_post_accept_gate 同款 vm harness；桌面侧同一恢复形状由
// platform/tauri/bridge/chat.js 的既有 catch 承担（本修复对齐桌面车道）。
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

// 物化活动会话（与 web_accept_plan_post_accept_gate 的 primeSessionA 同款）。
async function primeSessionA(rt) {
  const create = rt.setHandler('web_access_create_session', () => Promise.resolve({ id: 'chat-a', transcript_revision: 1 }));
  void create;
  const equip = rt.setHandler('equip_persona', () => Promise.resolve({ id: 'persona-prime', name: '引导卡' }));
  void equip;
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

const GUIDE = 'SCHEDULED-TASK-GUIDE 请一次只问我一个问题';

async function armConsumedTurnState(rt) {
  // 隐藏引导词：经桥自身入口 startScheduledTaskChat（enterDraft 落回草稿，
  // 活动会话清空——发送时由 sendMessage 先物化）。
  rt.setHandler('scheduled_task_chat_prompt', () => Promise.resolve(GUIDE));
  await rt.flat.startScheduledTaskChat();
  assert.equal(rt.view().scheduledTaskPendingGuide, GUIDE, 'precondition: the pending task guide is armed');
  assert.equal(rt.view().activeSessionId, null, 'precondition: the armed guide lives in the draft');
  // 活动技能标：草稿内经 chat:tool_start 的 load_skill 臂点亮（无 session_id
  // = 当前工作集，草稿同此）。
  rt.emit('chat:tool_start', { id: 'skill-arm-1', name: 'load_skill', args: { name: 'PPT' } });
  assert.equal(rt.view().activeSkill, 'pptx', 'precondition: the active skill is armed');
}

test('web: Auto 姿态门拒绝后已消费的引导词与技能标恢复，换档重试仍携带隐藏引导', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  await armConsumedTurnState(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'auto', multi_agent: false }));
  let chatCalls = 0;
  rt.setHandler('web_access_chat', () => { chatCalls += 1; return Promise.resolve({}); });
  await assert.rejects(
    rt.flat.sendMessage('创建晨报任务'),
    error => /Auto/.test(String(error && error.message)),
    'Auto 姿态门必须拒绝（webAutoSendBlocked 文案）'
  );
  assert.equal(chatCalls, 0, '拒绝不得开 turn：web_access_chat 不被调用');
  assert.equal(rt.view().scheduledTaskPendingGuide, GUIDE,
    '拒绝路径必须恢复已消费的 pending task guide（否则换档重试只剩可见文本）');
  assert.equal(rt.view().activeSkill, 'pptx', '拒绝路径必须恢复已消费的 active skill');
  assert.equal(rt.view().scheduledTaskCreationSessionId, null,
    '拒绝路径不得把目标会话钉在被拒的发送上');
  // 换支持档位重试：隐藏引导 + 工具限制必须随行（任务创建协议的既有形状）。
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', multi_agent: false }));
  let retryArgs = null;
  rt.setHandler('web_access_chat', (args) => { retryArgs = args; return Promise.resolve({}); });
  await rt.flat.sendMessage('创建晨报任务');
  assert.ok(retryArgs, '换档重试必须真正起轮');
  assert.ok(String(retryArgs.message).includes('SCHEDULED-TASK-GUIDE'),
    '换档重试必须携带隐藏引导词（恢复后的 guide 再次消费进 payload）');
  assert.equal(retryArgs.restrictTools, true, '换档重试必须保留 restrictTools');
  assert.equal(rt.view().scheduledTaskPendingGuide, null, '成功发送后 guide 按既有契约消费掉');
});

test('web: 姿态门放行路径不恢复（对照——成功发送后引导词保持已消费）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);
  await armConsumedTurnState(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', multi_agent: false }));
  let chatArgs = null;
  rt.setHandler('web_access_chat', (args) => { chatArgs = args; return Promise.resolve({}); });
  await rt.flat.sendMessage('创建晨报任务');
  assert.ok(chatArgs, '可执行姿态照常起轮（对照）');
  assert.ok(String(chatArgs.message).includes('SCHEDULED-TASK-GUIDE'), '首轮发送携带隐藏引导');
  assert.equal(chatArgs.restrictTools, true);
  assert.equal(rt.view().scheduledTaskPendingGuide, null, '成功路径 guide 保持已消费（现行为不变）');
});

console.log('web_send_gate_consumed_state_restore: ok');
