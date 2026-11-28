// 守卫拒绝通告去重的作用域 + web 本地 per-sid 表删除清扫（round-32 review m7/m8）。
//
// m7：web 的 chat:guardrail_denied 去重键（sid\0tool\0reason）不带 per-call id——
// 修复前它存活整个 tab 生命周期，不同 turn 里同会话+同工具+同理由的两次真实拒绝
// 只显示第一条（desktop 侧完全不去重，双车道行为分叉）。去重存在的理由是重连重放/
// 重挂载重喂（只会回放仍在途 turn 的拒绝），因此键按 sid 归档并在该 sid 的 turn
// 终态清除：turn 内重放仍去重，后续 turn 的同形拒绝重新通告。
//
// m8：web bridge 闭包的五个本地 per-sid 表（webAutoSendNoticeSids /
// webApprovalNoticeIds / 守卫键双表 / webApprovalReplayTerminalSeq）是共享
// applyDeletedSession 够不到的闭包态——本文件经 session:deleted 事件走
// applyDeletedSession 包装，验证删除后守卫键确被清扫（同 id 字符串重喂不再被
// 旧键压制；id 实际永不复用，此处以重喂可观察性钉清扫契约，desktop 车道同卫生）。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const webBridgeRoot = path.resolve(here, '..', 'src', 'platform', 'web');
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

const denialPayload = (reason = 'plan_mode') => ({
  session_id: 'chat-a', tool_name: 'write_file', reason,
});

function guardrailNoticeCount(rt) {
  return (rt.view().chatItems || []).filter(item => item && item.guardrailDenied).length;
}

test('web 守卫拒绝通告：turn 内重放去重保留，跨 turn 同形拒绝重新通告（round-32 m7）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // 同一 turn 内两次同形拒绝 → 只通告一次（round-13 重放去重契约保留）。
  rt.emit('chat:guardrail_denied', denialPayload());
  rt.emit('chat:guardrail_denied', denialPayload());
  await tick();
  assert.equal(guardrailNoticeCount(rt), 1, 'turn 内重放仍去重（恰好一条）');

  // turn 终态清除该 sid 的键 → 后续 turn 的同形拒绝必须重新通告
  // （修复前：平坦键存活整个 tab 生命周期，第二条被吞）。
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1401 });
  await tick(); await tick();
  rt.emit('chat:guardrail_denied', denialPayload());
  await tick();
  assert.equal(guardrailNoticeCount(rt), 2,
    'turn 终态后同形拒绝必须重新通告（修复前被生命周期键吞掉）');
});

test('web 守卫拒绝通告：session 删除清扫本地 per-sid 键表（round-32 m8）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  rt.emit('chat:guardrail_denied', denialPayload());
  await tick();
  assert.equal(guardrailNoticeCount(rt), 1, 'precondition: 首条拒绝已通告');

  // 删除会话（session:deleted → applyDeletedSession 包装 → web 本地表清扫）。
  rt.emit('session:deleted', { id: 'chat-a' });
  await tick();

  // 重新物化同 id 会话（id 永不复用是产品事实；此处以同 id 字符串重喂的可观察
  // 性钉清扫契约）。注意 applyDeletedSession 会重置缓冲——计数从头开始：
  // 修复后重喂拒绝照常通告（=1）；修复前残留键压制首条拒绝（=0），这正是判别面。
  await primeSessionA(rt);
  assert.equal(guardrailNoticeCount(rt), 0, 'precondition: 重建缓冲的通告计数归零');
  rt.emit('chat:guardrail_denied', denialPayload());
  await tick();
  assert.equal(guardrailNoticeCount(rt), 1,
    '删除清扫后同形拒绝必须重新通告（修复前被残留键压制，计数为 0）');
});
