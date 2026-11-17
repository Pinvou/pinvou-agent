// Web aux 聊天受理面的 Auto 姿态门（round-13 review M-4）的运行时回归：
// aux 面板是第四个 turn-start 受理面，此前是唯一绕过 webAutoAdmissionGate
// 的入口——aux 会话按未绑定 plain 解析（后端默认 Auto），Web 面无审批卡
// 决策面，Auto 姿态下开轮会停在「只有事后通告的等待」。本回归钉：Auto 拒
// 绝不开轮（web_access_chat 不被调用）且通告落 aux 会话自己的 buffer（经
// auxChatSnapshot 可见）；可执行姿态（plan）对照放行、通告不落。
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
  const calls = { invoke: [] };
  const handlers = {
    web_access_list_sessions: () => Promise.resolve([{ id: 'chat-a', title: '新对话', updated_at: 1 }]),
    // Empty-session chunk: eof terminates the loader loop; the empty payload
    // decodes to `[]` (no messages on a fresh aux session).
    web_access_load_session_chunk: () => Promise.resolve({ offset: 0, total: 2, download_id: 'dl-aux', data_base64: 'W10=', eof: true }),
  };
  const listeners = {};
  const deferreds = {};
  const windowObject = {
    // The aux panel's dispatch path waits for the desktop capability
    // snapshot before its first invoke (bootstrap.js's PLATFORM surface);
    // report ready so the test exercises the gate, not the wait.
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false, areInvokeCapabilitiesReady: () => true, getConnectionState: () => 'connected' },
    __TAURI__: {
      core: { invoke: (name, args) => {
        calls.invoke.push(name);
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
    atob,
    btoa,
    addEventListener() {},
    removeEventListener() {},
    setTimeout,
    clearTimeout,
  };
  const context = vm.createContext({
    window: windowObject,
    document: documentObject,
    navigator: { mediaDevices: null },
    localStorage,
    console,
    atob,
    btoa,
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
  assert.equal(typeof flat.auxChatSend, 'function', 'the aux send must be exported for the panel');
  assert.equal(typeof flat.auxChatSnapshot, 'function', 'the aux snapshot must be exported for the panel');
  return {
    flat,
    calls,
    view() { return flat.getState(); },
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    setHandler(name, fn) { handlers[name] = fn; },
  };
}

// 侧聊会话物化（aux 会话永不 active：后台 buffer 路径）。
async function primeAux(rt) {
  const ensure = rt.defer('get_or_create_aux_session');
  const p = rt.flat.auxChatEnsure('task-main');
  ensure.resolve({ id: 'aux-1', title: '侧聊', created_at: 1, message_count: 0 });
  const auxId = await p;
  assert.equal(auxId, 'aux-1', 'precondition: the aux session id must materialize');
  return auxId;
}

function auxBlockedNotices(rt, auxId) {
  // 三语文案都含「Auto」档位词（en "Auto mode" / ja "Auto モード" / zh "Auto 档"）。
  return rt.flat.auxChatSnapshot(auxId).chatItems.filter(
    item => item.type === 'system' && String(item.text || '').includes('Auto')
  );
}

test('web: aux 面板发送在 Auto 姿态被拒且不开轮（第四受理面过同一姿态门）', async () => {
  const rt = bootWebBridge();
  const auxId = await primeAux(rt);
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'auto', post_accept_mode: 'auto', multi_agent: false }));
  await assert.rejects(
    rt.flat.auxChatSend(auxId, '帮我看看这个'),
    error => /Auto/.test(String(error && error.message)),
    'Auto 姿态的 aux 发送必须走共享 Auto 拒绝面'
  );
  assert.equal(
    rt.calls.invoke.filter(name => name === 'web_access_chat').length,
    0,
    '拒绝不得开轮：web_access_chat 不被调用'
  );
  assert.equal(auxBlockedNotices(rt, auxId).length, 1, '拒绝必须落一条用户面通告（aux 自己的 buffer）');
});

test('web: aux 面板发送在可执行姿态照常派发（对照；round-5 review 合并原重复的默认创建全链正例）', async () => {
  const rt = bootWebBridge();
  const auxId = await primeAux(rt);
  // 默认创建链（review JensenChen28 5801173510）：aux 从不激活、面板无 mode
  // 控件，后端在创建时即钉安全档（Rust 侧 aux_session_mode_is_pinned_at_creation
  // 钉住的契约）；Plan 桩镜像该创建时投影。原「默认创建全链」测试与本测试逐
  // 字节重复（同一 primeAux / 同一桩 / 同一断言，vm 桩本就不区分姿态来源），
  // round-5 review 后合并为一个对照面；准入边界未放宽由上方负例钉死。
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: 'plan', post_accept_mode: 'plan', multi_agent: false }));
  const send = rt.defer('web_access_chat');
  const p = rt.flat.auxChatSend(auxId, '帮我看看这个');
  send.resolve({ ok: true });
  await p;
  assert.equal(
    rt.calls.invoke.filter(name => name === 'web_access_chat').length,
    1,
    '可执行姿态的 aux 发送照常派发（对照面即门通过）'
  );
  assert.equal(auxBlockedNotices(rt, auxId).length, 0, '放行不得落 Auto 拦截通告');
});

// Round-18（评审 JensenChen28 5816589125 P2 直发预留的第四面）：共享助手的
// busy 检查是同步的,面板连发会各自通过并并行起两条姿态查询,逆序回执时后
// 输入先派发。修复后 aux 派发在首 await 前认领与 doSendFor 同一个 per-session
// 受理拥有者:第二条等待而非并行查询,首条先派发。
const tick = () => new Promise(r => { setTimeout(r, 0); });
test('web: aux 面板连发由受理拥有者串行化（首条先派发,第二条不并行查询）', async () => {
  const rt = bootWebBridge();
  const auxId = await primeAux(rt);
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  const dispatchOrder = [];
  rt.setHandler('web_access_chat', (args) => {
    dispatchOrder.push(args.message);
    return Promise.resolve({});
  });
  const sendA = rt.flat.auxChatSend(auxId, 'first');
  await tick(); await tick();
  const sendB = rt.flat.auxChatSend(auxId, 'second');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 1, '第二条连发不得并行起第二条姿态查询（等受理拥有者）');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, '首条落定释放后第二条才起自己的查询');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['first', 'second'], '派发顺序必须 first 先 second 后');
  await sendA; await sendB;
});
