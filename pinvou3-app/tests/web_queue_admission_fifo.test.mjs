// Web queued-admission FIFO (review JensenChen28 5804488461 P2): during the
// get_mode_state posture-gate round trip inside doSendFor, busy is still
// false, so async triggers such as mode_changed can re-enter within that
// window and shift away the next message; if the second message's posture
// query settles first, it is admitted ahead of the queue head — FIFO order is
// broken and the backend occupancy fence cannot restore it. Fix = hold the
// admission owner per sid across awaits and release it once the admission
// settles (admitted, or refused back into the queue). This test reproduces
// overlapping triggers + out-of-order posture completion with per-command
// deferreds (the shape requested in review), asserting that within the
// window: the queue stays put and only one posture query runs; after the
// queue head is admitted, the remaining messages are still dispatched one by
// one in queue order.
// Second round (review JensenChen28 5810381950 P2): new input arriving inside
// the in-flight admission window must not take the direct-send path — once
// the queue head has been shifted out by the admission owner,
// queued.length===0 and busy===false, so if the direct-send checks do not
// consult the admission owner, new input bypasses the queue head and starts a
// second posture query (D,A out of order). The runtime regression reuses the
// same vm harness as web_send_gate_consumed_state_restore.
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

test('web 排队受理 FIFO：姿态门在途窗口内的重入触发不得越过队首', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Round 1: A is direct-sent and admitted (web_access_chat pending keeps the
  // busy surface up). Posture gate: A's query is released immediately; after
  // that (B's query) it stays pending until gateRelease.
  let gateCalls = 0;
  let gateRelease = null;
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    if (gateCalls === 1) return Promise.resolve({ mode: 'plan', multi_agent: false });
    return new Promise(resolve => { gateRelease = resolve; });
  });
  rt.setHandler('web_access_chat', () => Promise.resolve({}));
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理，busy 面');
  assert.equal(gateCalls, 1, 'precondition: A 的姿态门查询恰一次');

  // B and C queue up.
  await rt.flat.sendMessage('B');
  await rt.flat.sendMessage('C');
  assert.equal(rt.view().queued.length, 2, 'precondition: B、C 入队');

  // Turn reaches its terminal state → flush takes queue head B; its
  // posture-gate query stays pending (the in-flight admission window).
  const doneSeq = 100 + 1; // the payload only needs a terminal-order-comparable shape; see the chat:done consumer
  rt.emit('chat:done', { session_id: 'chat-a', seq: doneSeq });
  await tick(); await tick();
  assert.equal(rt.view().busy, false, 'A 轮终态后 busy 落下');
  assert.equal(gateCalls, 2, 'B 的姿态门查询已起');
  assert.equal(rt.view().queued.length, 1, '仅队首 B 出队（C 仍在队）');

  // Re-entrant trigger inside the window (mode_changed, non-Auto →
  // flushQueued): before the fix this would shift away C and start a second
  // posture query; after the fix it yields.
  rt.emit('chat:mode_changed', { session_id: 'chat-a', mode: 'plan' });
  await tick(); await tick();
  assert.equal(gateCalls, 2, '受理在途时重入触发不得再起姿态查询');
  assert.equal(rt.view().queued.length, 1, '重入触发不得 shift 走 C');
  assert.equal(rt.view().busy, false, '重入触发不得伪造 busy 面');

  // B's query (non-Auto) is released → B is admitted.
  gateRelease({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(rt.view().busy, true, 'B 受理后 busy 顶上');
  const bPosition = JSON.stringify(rt.view().messages).indexOf('"B"');
  assert.ok(bPosition >= 0, 'B 的乐观提交已入 transcript');

  // B's turn reaches its terminal state → flush C; C's query is released → C
  // is admitted. The order must be B before C.
  rt.emit('chat:done', { session_id: 'chat-a', seq: doneSeq + 1 });
  await tick(); await tick();
  assert.equal(gateCalls, 3, 'C 的姿态门查询在 B 终态后才起（受理拥有者已释放）');
  gateRelease({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 0, '队列清空');
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(orderText.includes('"B"') && orderText.indexOf('"C"') > orderText.indexOf('"B"'),
    'B 必须先于 C 入 transcript（FIFO）');
});

test('web 排队受理 FIFO：队首被 Auto 门拒绝回队后仍保持队首（对照）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Controllable posture gate: A's direct send is released; B's flush hits
  // Auto; once out of Auto, it is released again.
  let autoNow = false;
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: autoNow ? 'auto' : 'plan', multi_agent: false }));
  rt.setHandler('web_access_chat', () => Promise.resolve({}));
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  assert.equal(rt.view().queued.length, 1, 'precondition: B 入队');

  autoNow = true; // queue-head B's retry hits the Auto posture gate
  rt.emit('chat:done', { session_id: 'chat-a', seq: 201 });
  await tick(); await tick();
  // B is refused by the Auto gate: web_access_chat is not called, B is
  // requeued in place, and the admission owner is released.
  assert.equal(rt.view().queued.length, 1, 'Auto 拒绝后队首回队（原行为不变）');
  assert.equal(rt.view().busy, false, '拒绝不开 turn');
  // A mode_changed leaving Auto triggers the retry: this time the query is
  // released → B is admitted.
  autoNow = false;
  rt.emit('chat:mode_changed', { session_id: 'chat-a', mode: 'plan' });
  await tick(); await tick();
  assert.equal(rt.view().busy, true, '离开 Auto 后重试受理由 mode_changed 触发照常完成');
  assert.equal(rt.view().queued.length, 0, '重试消费队首');
});

test('web 排队受理 FIFO：受理在途窗口的新输入必须入队，不得直发绕过队首', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A is direct-sent and admitted (query 1 is released immediately); later
  // queries stay pending until gateRelease.
  let gateCalls = 0;
  let gateRelease = null;
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    if (gateCalls === 1) return Promise.resolve({ mode: 'plan', multi_agent: false });
    return new Promise(resolve => { gateRelease = resolve; });
  });
  rt.setHandler('web_access_chat', () => Promise.resolve({}));
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');

  // B queues up; the turn reaches its terminal state → flush takes B; its
  // posture-gate query stays pending (the in-flight admission window).
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 301 });
  await tick(); await tick();
  assert.equal(rt.view().busy, false, 'B 在姿态门窗口：busy 未顶上');
  assert.equal(rt.view().queued.length, 0, 'B 已被受理 owner shift 出队');
  assert.equal(gateCalls, 2, 'B 的姿态门查询在途');

  // New input D inside the window: before the fix, neither direct-send check
  // consulted the admission owner, so D was direct-sent and started a second
  // query that hung (D settling first means D,A out of order); after the fix,
  // D is queued behind B and no new query starts.
  const sendD = rt.flat.sendMessage('D');
  await tick(); await tick();
  const dLanded = await Promise.race([
    sendD.then(() => true, () => false),
    new Promise(resolve => { setTimeout(() => resolve(false), 50); }),
  ]);
  assert.equal(dLanded, true, 'D 必须走入队路径即时落定，而非挂在新姿态查询上');
  assert.equal(gateCalls, 2, '受理在途时新输入不得起第二条姿态查询');
  assert.equal(rt.view().queued.length, 1, 'D 必须入队排在 B 后');
  assert.equal(rt.view().busy, false, 'D 不得在 B 受理前开 turn');

  // B is released and admitted → B's turn reaches its terminal state → flush
  // D, released → D is admitted. The order must be B before D.
  gateRelease({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(rt.view().busy, true, 'B 受理后 busy 顶上');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 302 });
  await tick(); await tick();
  assert.equal(gateCalls, 3, 'D 的姿态门查询在 B 终态后才起（受理拥有者已释放）');
  gateRelease({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(orderText.includes('"B"') && orderText.indexOf('"D"') > orderText.indexOf('"B"'),
    'B 必须先于 D 入 transcript（FIFO）');
  assert.equal(rt.view().queued.length, 0, '队列清空');
});

test('web 排队受理 FIFO：终态先于回执时，受理释放后必须补排空队首', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Both the posture gate and receipts are fully controllable: A is released
  // immediately with an immediate receipt; B's receipt stays pending after
  // release; C is released afterwards.
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => { chatQueue.push(resolve); });
  });
  // Round 1 (A): direct send; the query and receipt are released in order.
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  chatQueue.shift()({});
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');

  // B and C queue up → A reaches its terminal state → flush takes B; B's
  // query is released for admission and its receipt stays pending.
  await rt.flat.sendMessage('B');
  await rt.flat.sendMessage('C');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 401 });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, 'precondition: 仅队首 B 出队（C 在队）');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 2, 'precondition: B 的 web_access_chat 回执在途');

  // B's terminal state arrives before its receipt: busy drops, the
  // done-triggered flush is turned back by the in-flight owner, and C stays
  // put.
  rt.emit('chat:done', { session_id: 'chat-a', seq: 402 });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, '拥有者在途：终态触发的 flush 不得动 C');

  // B's receipt arrives: released where the successful admission settles, plus
  // a follow-up drain — C is picked up automatically (no external trigger at
  // all).
  chatQueue.shift()({});
  await tick(); await tick();
  assert.equal(gateCalls, 3, '补排空必须拉起 C 的姿态查询');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 3, '补排空必须发出 C');
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(
    orderText.includes('"B"') && orderText.indexOf('"C"') > orderText.indexOf('"B"'),
    'B 必须先于 C 入 transcript（FIFO）'
  );
  assert.equal(rt.view().queued.length, 0, '队列清空');
});

// Round-31 (review zhuowp 5854578975 P2, the direct-entry twins of the
// flush-chain top-up above): a DIRECT send that is accepted after its own
// terminal event must resume the missed idle drain from the direct path
// itself. While A's web_access_chat receipt is pending, the direct admission
// owner is still held, so A's done-triggered flush stands down with B still
// queued; when the receipt then releases the owner, neither the composer
// success path nor sendMessageToSession starts another drain — B sits queued
// with busy=false until an unrelated action/event. Both direct entries are
// pinned separately (composer sendMessage / explicit sendMessageToSession).
test('web 排队受理 FIFO：终态先于回执时，直发受理释放后必须补排空队首（composer 直发）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => { chatQueue.push(resolve); });
  });

  // A is direct-sent: its posture query is released immediately, its receipt
  // stays pending (the direct admission owner is held for the whole window).
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 1, 'precondition: A 已直发，回执在途');
  assert.equal(rt.view().busy, true, 'precondition: A 已受理，busy 面');

  // B queues while A's receipt is pending; A's terminal state arrives first:
  // the done-triggered flush must stand down against the in-flight owner.
  await rt.flat.sendMessage('B');
  assert.equal(rt.view().queued.length, 1, 'precondition: B 入队');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 601 });
  await tick(); await tick();
  assert.equal(gateCalls, 1, '拥有者在途：终态触发的 flush 不得起 B 的查询');
  assert.equal(rt.view().queued.length, 1, 'B 仍在队');

  // A's receipt arrives and the direct owner releases here: the missed drain
  // must resume from the direct success path itself (no external trigger).
  chatQueue.shift()({});
  await tick(); await tick();
  assert.equal(gateCalls, 2, '直发受理释放后必须补排空队首（拉起 B 的姿态查询）');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 2, '补排空必须发出 B');
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(
    orderText.includes('"A"') && orderText.indexOf('"B"') > orderText.indexOf('"A"'),
    'A 必须先于 B 入 transcript（FIFO）'
  );
  assert.equal(rt.view().queued.length, 0, '队列清空');
  await sendA;
});

test('web 排队受理 FIFO：终态先于回执时，直发受理释放后必须补排空队首（显式会话直发）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => { chatQueue.push(resolve); });
  });

  // A goes through the explicit-session surface's direct arm; its receipt
  // stays pending past its terminal state.
  const toA = rt.flat.sendMessageToSession('chat-a', 'A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 1, 'precondition: A 已直发，回执在途');
  assert.equal(rt.view().busy, true, 'precondition: A 已受理，busy 面');

  await rt.flat.sendMessageToSession('chat-a', 'B');
  assert.equal(rt.view().queued.length, 1, 'precondition: B 入队');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 611 });
  await tick(); await tick();
  assert.equal(gateCalls, 1, '拥有者在途：终态触发的 flush 不得起 B 的查询');
  assert.equal(rt.view().queued.length, 1, 'B 仍在队');

  chatQueue.shift()({});
  await tick(); await tick();
  assert.equal(gateCalls, 2, '直发受理释放后必须补排空队首（拉起 B 的姿态查询）');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 2, '补排空必须发出 B');
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(
    orderText.includes('"A"') && orderText.indexOf('"B"') > orderText.indexOf('"A"'),
    'A 必须先于 B 入 transcript（FIFO）'
  );
  assert.equal(rt.view().queued.length, 0, '队列清空');
  const settledA = await toA;
  assert.equal(settledA.accepted, true, '显式会话直发面返回受理形状');
});

test('web 排队受理 FIFO：前驱的尾清理不得抹掉后继的受理所有权', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Fully controllable: A/B/C are admitted in turn; B's receipt is held past
  // its terminal state (to force a follow-up drain), and C's query is started
  // inside B's success arm (the successor takes ownership synchronously).
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => { chatQueue.push(resolve); });
  });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  chatQueue.shift()({});
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');

  // B and C queue up → A reaches its terminal state → flush B → B is admitted
  // but its receipt stays pending.
  await rt.flat.sendMessage('B');
  await rt.flat.sendMessage('C');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 501 });
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 2, 'precondition: B 的回执在途');

  // B's terminal state arrives before its receipt: busy drops and C stays put
  // (B's ownership is in flight).
  rt.emit('chat:done', { session_id: 'chat-a', seq: 502 });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, 'precondition: C 在队');

  // B's receipt arrives: the success arm releases B's token and runs the
  // follow-up drain — C leaves the queue synchronously and takes a new token,
  // its posture query pending. B's tail cleanup then runs and must not delete
  // C's token.
  chatQueue.shift()({});
  await tick(); await tick();
  await tick(); await tick();
  assert.equal(gateCalls, 3, '补排空拉起 C 的姿态查询');
  assert.equal(rt.view().queued.length, 0, 'precondition: C 已出队（受理在途）');

  // Fix point: C's ownership is still held (B's tail cleanup did not
  // overstep) → new input D must be queued behind C, not direct-sent past it.
  // Before the fix, D was direct-sent here and admitted ahead of C (wire
  // A,B,D,C).
  // Bounded wait: the reverted mutation of the admission token (release does
  // not check the token) would make this send never settle — a race timeout
  // turns it into a deterministically failing assertion instead of a hung CI.
  await Promise.race([
    rt.flat.sendMessage('D'),
    new Promise((resolve, reject) => { setTimeout(() => reject(new Error('D admission timed out')), 2000); }),
  ]);
  await tick(); await tick();
  assert.equal(gateCalls, 3, 'C 的所有权仍被持有：D 不得另起姿态查询');
  assert.equal(rt.view().queued.length, 1, 'D 必须入队排在 C 后');

  // C is released and admitted → C reaches its terminal state → flush D → D
  // is admitted. Order B,C,D.
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 3, 'C 受理发出');
  chatQueue.shift()({});
  await tick(); await tick();
  assert.equal(rt.view().busy, true, 'precondition: C 已受理（busy 顶上）');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 503 });
  await tick(); await tick();
  assert.equal(gateCalls, 4, 'D 的姿态查询在 C 终态后才起');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  const orderText = JSON.stringify(rt.view().messages);
  const ib = orderText.indexOf('"B"');
  const ic = orderText.indexOf('"C"');
  const id = orderText.indexOf('"D"');
  assert.ok(ib >= 0 && ic > ib && id > ic, 'wire 顺序必须 B,C,D');
  assert.equal(rt.view().queued.length, 0, '队列清空');
});

test('web 排队受理 FIFO：acceptPlan / editLastTurn 不得越过在途受理拥有者', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A direct-sent and admitted (query and receipt released in order) → B
  // queues up → A reaches its terminal state → flush takes B: owner held,
  // busy not yet raised, B's posture query pending = the admission window.
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(resolve); });
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => { chatQueue.push(resolve); });
  });
  let acceptCalls = 0;
  rt.setHandler('accept_plan', () => { acceptCalls += 1; return Promise.resolve({ mode: 'plan' }); });
  let editCalls = 0;
  rt.setHandler('edit_last_turn', () => { editCalls += 1; return Promise.resolve({ mode: 'plan' }); });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  chatQueue.shift()({});
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');

  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 601 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: B 的姿态查询在途（owner held）');
  assert.equal(rt.view().busy, false, 'precondition: 受理窗口 busy 未顶上');

  // Accepting the plan card + edit-rerun inside the admission window: both are
  // admission surfaces and must wait for the owner to settle — no new posture
  // query, and certainly no admission ahead of queue-head B. Before the fix,
  // gateCalls jumped by 2 here (two out-of-order posture queries; whichever
  // settled first was admitted ahead of B).
  const accept = rt.flat.acceptPlan(77, '# plan', 'go', 'plan-1');
  const edit = rt.flat.editLastTurn('edited');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, '在途拥有者未落定：acceptPlan/editLastTurn 不得另起姿态查询');
  assert.equal(acceptCalls, 0, '拥有者在途：accept_plan 不得先受理');
  assert.equal(editCalls, 0, '拥有者在途：edit_last_turn 不得先受理');

  // B is released and admitted → its receipt settles → the success arm
  // releases the owner and wakes both waiting surfaces. busy is already held
  // up by B, so both admitted rechecks silently yield: accept_plan /
  // edit_last_turn are still not called. Round-20 M2: once woken, each claims
  // the admission owner and holds it across the gate — this serializes the
  // queries (edit starts its own query only after accept's hold settles,
  // closing the concurrent-query seam on the same terms as the direct-send
  // surface).
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  chatQueue.shift()({});
  await tick(); await tick(); await tick();
  assert.equal(chatCalls, 2, 'precondition: B 已受理');
  assert.equal(rt.view().busy, true, 'precondition: B 已受理（busy 顶上）');
  // accept wakes first: it claims the owner and starts its own query; edit can
  // only wait while accept's hold is in flight.
  await tick(); await tick();
  assert.equal(gateCalls, 3, 'accept 的查询先行;edit 在 accept 的持有期间不得另起查询');
  assert.equal(acceptCalls, 0, 'accept 查询在途：accept_plan 不得先受理');
  assert.equal(editCalls, 0, 'accept 持有在途：editLastTurn 等待,不起 edit_last_turn');
  gateQueue.shift()({ mode: 'plan', multi_agent: false }); // accept's query → busy recheck yields + release
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 4, 'accept 释放后 edit 才起自己的查询（串行,不并发）');
  assert.equal(acceptCalls, 0, '队首已受理：acceptPlan 静默让位,不起 accept_plan');
  gateQueue.shift()({ mode: 'plan', multi_agent: false }); // edit's query → busy recheck yields
  await tick(); await tick(); await tick();
  await accept; await edit;
  assert.equal(editCalls, 0, '队首已受理：editLastTurn 静默让位,不起 edit_last_turn');
});

// ── Round-18 (review JensenChen28 5816589125 P2: direct-send reservation and queue-head yield)────────
test('web 排队受理 FIFO：并发双直发必须由直发预留串行化（后到者不得另起姿态查询）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A direct-sends: its query is pending → the admission owner is held by
  // doSendFor's direct-send reservation. Before the fix, direct sends did not
  // claim the owner, B's fence was all green, a second query started in
  // parallel, and with receipts settling out of order the later input was
  // admitted first (B,A out of order). After the fix, B is queued at the
  // fence (the owner is already claimed by A), and once A settles the queue
  // drains in order.
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
  const sendA = rt.flat.sendMessage('A');
  await tick();
  const sendB = rt.flat.sendMessage('B');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 1, 'A 的姿态查询在途时 B 不得另起查询（直发预留/入队）');
  assert.equal(rt.view().busy, false, 'A 未受理前 busy 未顶上');
  assert.equal(rt.view().queued.length, 1, 'B 入队等待拥有者落定');

  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'A 先受理');
  assert.equal(rt.view().busy, true, 'A 受理后 busy 顶上');

  rt.emit('chat:done', { session_id: 'chat-a', seq: 701 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'B 在 A 终态后才起自己的查询');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B'], '派发顺序必须 A 先 B 后（逆序回执不改序）');
  await sendA; await sendB;
});

test('web 排队受理 FIFO：受理窗口内 sendMessageToSession 不得绕过队首直发', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A admitted → B queued → A reaches its terminal state → flush takes B, its
  // query pending = the admission window (busy not yet raised, queue empty).
  // Inside the window, sendMessageToSession (same path as plan-stuck follow-up
  // instructions) must not start another query past the queue head; after the
  // fix it waits at the direct-send reservation. B settles and is admitted
  // (busy raised) → owner released → D is woken: since the round-19 review
  // minor-2, the post-wake recheck consults busy and requeues D instead of
  // direct-sending into web_session_turn_in_progress (the old pinned
  // assertion "D direct-sends immediately after B settles" is a message-loss
  // surface on the production backend, masked by the harness's web_access_chat
  // always returning ok); D waits for B's terminal-state flush to be carried
  // out in FIFO order.
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
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 801 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: B 的查询在途（owner held）');

  const toSession = rt.flat.sendMessageToSession('chat-a', 'D');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, '受理窗口内不得另起第二条查询（等待拥有者）');
  assert.equal(dispatchOrder.length, 1, '队首 B 未落定前不得派发 D');

  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.equal(dispatchOrder.length, 2, 'precondition: B 已受理（busy 顶上）');
  assert.equal(gateCalls, 2, '唤醒撞 busy 的 D 不得另起查询（改入队）');
  const viewAfterWake = rt.view();
  assert.equal(viewAfterWake.queued.length, 1, '唤醒撞 busy 的 D 必须改入队（不丢消息）');
  assert.equal(viewAfterWake.queued[0].text, 'D', '入队的是 D 本身');

  // B reaches its terminal state → the flush carries D out in FIFO order.
  rt.emit('chat:done', { session_id: 'chat-a', seq: 802 });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 3, 'B 落定释放后 D 的 flush 才起自己的查询');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B', 'D'], '派发顺序必须 A、B、D（队首优先）');
  await sendA;
  const completion = await toSession;
  assert.equal(completion.accepted, true, 'D 走直发面返回受理形状');
  if (completion.completion) {
    const inner = await completion.completion;
    assert.equal(inner.ok, true, '改入队路径对调用方等同成功受理');
  }
});

test('web 排队受理 FIFO：Auto 拒绝回队后 acceptPlan 必须让位队首（唤醒与新增点击一致）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Seed an acceptable plan card through the chat:plan_ready producer (the
  // fixture uses the production shape — the round-16 lesson).
  rt.emit('chat:plan_ready', { session_id: 'chat-a', plan_id: 'plan-1', plan_snapshot: { steps: [] } });
  await tick(); await tick();
  const planCard = rt.view().chatItems.find(item => item.type === 'plan_card');
  assert.ok(planCard && planCard.cardState === 'active', 'precondition: 计划卡已种');

  // A admitted → B queued → A reaches its terminal state → flush takes B →
  // the query lands on Auto → B is requeued, owner released → the
  // already-waiting acceptPlan is woken: its admitted recheck must yield to
  // the requeued queue head (triggering a re-send of the head under the new
  // mode and yielding to it), and must not admit accept_plan first.
  let gateCalls = 0;
  const gateQueue = [];
  const gateModes = ['plan', 'auto', 'plan', 'plan'];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    const mode = gateModes[gateCalls - 1] || 'plan';
    return new Promise(resolve => { gateQueue.push(() => resolve({ mode, multi_agent: false })); });
  });
  const dispatchOrder = [];
  rt.setHandler('web_access_chat', (args) => {
    dispatchOrder.push(args.message);
    return Promise.resolve({});
  });
  let acceptCalls = 0;
  rt.setHandler('accept_plan', () => { acceptCalls += 1; return Promise.resolve({ mode: 'plan' }); });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()();
  await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 901 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: B 的查询在途');
  const accept = rt.flat.acceptPlan(planCard.id, '# plan', 'go', 'plan-1');
  await tick(); await tick();
  assert.equal(acceptCalls, 0, 'owner 在途：acceptPlan 等待');

  gateQueue.shift()(); // B's query → auto → refused and requeued
  await tick(); await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, 'precondition: B 回队');
  assert.equal(dispatchOrder.length, 1, 'B 被拒不开轮');

  // acceptPlan is woken: its query lands on the new plan mode, and the
  // admitted recheck finds the queue head still present → it triggers a
  // re-send of the queue head (admissible under the new mode) and yields.
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 3, 'acceptPlan 的姿态查询先落（新档 plan）');
  assert.equal(acceptCalls, 0, '队首回队期间 accept_plan 不得先受理');
  gateQueue.shift()(); // acceptPlan's query is released → the admitted recheck yields and triggers the queue head
  await tick(); await tick();
  assert.equal(gateCalls, 4, '队首 B 重发的新查询（新档 plan）');
  assert.equal(acceptCalls, 0, '队首未落定前 accept_plan 不得受理');
  gateQueue.shift()(); // B's re-send is released → admitted
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B'], '队首 B 先于 accept_plan 落轮');
  assert.equal(rt.view().queued.length, 0, '队首已受理，队清空');
  assert.equal(rt.view().busy, true, 'B 的 turn 在途：acceptPlan 下一击由 busy 复查让位');
  assert.equal(acceptCalls, 0, 'accept_plan 全程未被调用（优先级让位）');
  await accept; await sendA;
});


test('web 排队受理 FIFO：队首被拒回队后 sendMessageToSession 唤醒必须尾随入队（round-20 P2）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A admitted → B queued → A reaches its terminal state → flush takes B,
  // query pending (owner held, busy not yet raised, queue empty). Inside the
  // window, sendMessageToSession('D') can only wait. B's query lands on Auto
  // → B is refused and requeued (unshift happens before the owner is
  // released, review 5812242993) → D is woken: round-20 review P2 — a
  // post-wake recheck of busy alone would bypass the queue head and
  // direct-send inside the "busy false, queue head already requeued" window
  // (D's query settling first means dispatch, changing the order to A,D).
  // After the fix, the recheck also consults the session queue, so D trails
  // into the queue (the queue head keeps priority) and is carried out in FIFO
  // order by the next trigger.
  let gateCalls = 0;
  const gateQueue = [];
  const gateModes = ['plan', 'auto', 'plan', 'plan', 'plan'];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    const mode = gateModes[gateCalls - 1] || 'plan';
    return new Promise(resolve => { gateQueue.push(() => resolve({ mode, multi_agent: false })); });
  });
  const dispatchOrder = [];
  rt.setHandler('web_access_chat', (args) => {
    dispatchOrder.push(args.message);
    return Promise.resolve({});
  });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()(); // A → plan
  await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1001 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: B 的查询在途（owner held）');

  const toSession = rt.flat.sendMessageToSession('chat-a', 'D');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, '受理窗口内不得另起第二条查询');
  assert.equal(dispatchOrder.length, 1, '队首 B 未落定前不得派发 D');

  gateQueue.shift()(); // B → auto → refused and requeued + owner released
  await tick(); await tick(); await tick();
  assert.equal(rt.view().busy, false, 'precondition: B 被拒，busy 落下');
  const viewAfterWake = rt.view();
  assert.equal(viewAfterWake.queued.length, 2, 'B 回队 + D 尾随入队（不得越过队首直发）');
  assert.equal(viewAfterWake.queued[0].text, 'B', '被拒回队的 B 仍是队首');
  assert.equal(viewAfterWake.queued[1].text, 'D', '唤醒的 D 排在队尾');
  assert.equal(gateCalls, 2, 'D 尾随入队不得另起姿态查询');
  assert.equal(dispatchOrder.length, 1, 'D 不得先于队首派发');

  // Next trigger (E goes through the explicit surface's enqueue arm, which
  // flushes on the way): the queue head B is re-sent under the new mode.
  await rt.flat.sendMessageToSession('chat-a', 'E');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 3, '队首 B 重发的新查询');
  gateQueue.shift()(); // B → plan → admitted
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B'], '队首 B 先于 D、E 派发');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1002 });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 4, 'D 的 flush 查询');
  gateQueue.shift()(); // D → plan
  await tick(); await tick(); await tick();
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1003 });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 5, 'E 的 flush 查询');
  gateQueue.shift()(); // E → plan
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B', 'D', 'E'], '最终派发序必须严格 FIFO');
  assert.equal(rt.view().queued.length, 0, '队列清空');
  await sendA;
  const completion = await toSession;
  assert.equal(completion.accepted, true, 'D 的直发面返回受理形状');
});

test('web 排队受理 FIFO：owner 释放按注册序唤醒等待者（LIFO 即红）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // Two tokenless waiters (W1 registered first, W2 second) are woken in turn
  // when the owner is released: forward iteration lets the earliest registrant
  // win the recheck window — W1 claims first, hits busy (H already admitted)
  // and is requeued; W2 then claims and is requeued on the same terms, so the
  // queue order [W1, W2] reflects registration order. If wakeups were
  // reversed (LIFO), W2 would claim and enqueue first, the queue order would
  // become [W2, W1], and the final dispatch order would be out of order —
  // this assertion goes red (behavior pin for the round-19 wakeup-order fix,
  // review round-20 minor).
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => { gateQueue.push(() => resolve({ mode: 'plan', multi_agent: false })); });
  });
  const dispatchOrder = [];
  rt.setHandler('web_access_chat', (args) => {
    dispatchOrder.push(args.message);
    return Promise.resolve({});
  });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()(); // A → plan
  await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'precondition: A 已受理');
  await rt.flat.sendMessage('H');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1101 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: H 的查询在途（owner held）');

  const w1 = rt.flat.sendMessageToSession('chat-a', 'W1');
  const w2 = rt.flat.sendMessageToSession('chat-a', 'W2');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, '两个等待者都不得另起查询');

  gateQueue.shift()(); // H → plan → admitted (busy raised) + owner released
  await tick(); await tick(); await tick();
  const viewAfterWake = rt.view();
  assert.equal(viewAfterWake.busy, true, 'precondition: H 已受理');
  assert.equal(viewAfterWake.queued.length, 2, '两个等待者都撞 busy 改入队');
  assert.equal(viewAfterWake.queued[0].text, 'W1', '先注册的 W1 先被唤醒认领 → 先入队');
  assert.equal(viewAfterWake.queued[1].text, 'W2', '后注册的 W2 后入队（LIFO 唤醒则此序反转）');

  rt.emit('chat:done', { session_id: 'chat-a', seq: 1102 });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 3, 'W1 的 flush 查询');
  gateQueue.shift()(); // W1 → plan
  await tick(); await tick(); await tick();
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1103 });
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 4, 'W2 的 flush 查询');
  gateQueue.shift()(); // W2 → plan
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'H', 'W1', 'W2'], '最终派发序必须反映注册唤醒序');
  assert.equal(rt.view().queued.length, 0, '队列清空');
  await sendA; await w1; await w2;
});


test('web 排队受理 FIFO：直发面改入队臂的 notify 抛出不得泄漏受理拥有者（round-20 M3）', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // After being woken, D hits "busy false + queue head in queue" and is
  // requeued: the notify() at the tail of the enqueue arm deliberately
  // rethrows the subscriber exception. Before the fix, this branch sat
  // outside the try/finally — the exception rejected doSendFor as a whole,
  // nobody released the admission owner, and later flushes yielded forever at
  // the owner guard (queue frozen, posture-query count no longer growing).
  // After the fix, the branch moved inside the try: finally releases the
  // owner, and trigger surfaces drain as usual. The core assertion = after
  // the exception settles, the next flush can still start the queue head's
  // posture query.
  let gateCalls = 0;
  const gateQueue = [];
  const gateModes = ['plan', 'auto', 'plan', 'plan', 'plan'];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    const mode = gateModes[gateCalls - 1] || 'plan';
    return new Promise(resolve => { gateQueue.push(() => resolve({ mode, multi_agent: false })); });
  });
  const dispatchOrder = [];
  rt.setHandler('web_access_chat', (args) => {
    dispatchOrder.push(args.message);
    return Promise.resolve({});
  });
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()(); // A → plan
  await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1201 });
  await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: B 的查询在途（owner held）');

  const toSession = rt.flat.sendMessageToSession('chat-a', 'D');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 2, 'precondition: D 在受理窗口内等待');

  // The subscriber throws exactly once, on the notification where D has just
  // been enqueued (at B's refusal notification the queue is still empty and
  // unaffected; after throwing, the subscriber unsubscribes, and later drain
  // surfaces proceed as usual).
  let armed = true;
  const unsubscribe = rt.flat.subscribe(function (snapshot) {
    const queued = (snapshot && snapshot.queued) || [];
    if (armed && queued.some(item => item && item.text === 'D')) {
      armed = false;
      throw new Error('subscriber boom on the requeue notify');
    }
  });

  gateQueue.shift()(); // B → auto → refused and requeued + D requeued + notify throws
  await tick(); await tick(); await tick();
  assert.equal(dispatchOrder.length, 1, '异常路径不得产生任何派发（无双重派发）');
  const viewAfterThrow = rt.view();
  assert.equal(viewAfterThrow.queued.length, 2, 'B 回队 + D 改入队均在');
  assert.equal(viewAfterThrow.queued[1].text, 'D', '改入队的是 D 本身');
  const completion = await toSession;
  assert.equal(completion.accepted, true, '直发面返回受理形状');
  assert.equal(completion.completion && (await completion.completion).ok, false, 'notify 抛出让 D 的直发面如实报错');

  // The owner has been released by finally: the next trigger starts the queue
  // head's posture query as usual (before the fix, flushQueued yielded
  // forever at the owner guard and gateCalls froze at 2).
  await rt.flat.sendMessageToSession('chat-a', 'E');
  await tick(); await tick(); await tick();
  assert.equal(gateCalls, 3, '异常落定后 owner 必须已释放——队首 B 的新查询照常在途');
  gateQueue.shift()(); // B → plan
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B'], '队首照常按 FIFO 排水');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1202 });
  await tick(); await tick(); await tick();
  gateQueue.shift()(); // D → plan
  await tick(); await tick(); await tick();
  rt.emit('chat:done', { session_id: 'chat-a', seq: 1203 });
  await tick(); await tick(); await tick();
  gateQueue.shift()(); // E → plan
  await tick(); await tick(); await tick();
  assert.deepEqual(dispatchOrder, ['A', 'B', 'D', 'E'], '最终派发序严格 FIFO,队列不冻结');
  unsubscribe();
  await sendA;
});


// ── Structural pins (review 5812242993: asymmetric red arms of the direct-send fence)────────────────────────
// The direct-send fence is defense in depth: deleting either of the two
// pre-/post-hydrate checks alone does not change observable behavior (the
// later one still intercepts); the behavior surface only goes red when the
// pair is deleted together. The source pins lock in that both fence sites,
// the admission release points, and the acceptPlan/editLastTurn waits each
// exist — deleting any one goes red, and a silent one-sided disappearance is
// no longer possible.
const bridgeSource = read('bridge.js');
assert.equal(
  (bridgeSource.match(/isBusyFor\(sid\) \|\| state\.queued\.length > 0 \|\| flushAdmissionSids\.has\(sid\)/g) || []).length,
  2,
  '直发栅栏必须在 hydrate 前后各咨询一次受理拥有者（两处,缺一即红）'
);
assert.equal(
  (bridgeSource.match(/releaseAdmission\(sid, admissionToken\)/g) || []).length,
  5,
  '受理释放点应为：flushQueued 链三处（成功臂 + 链尾兜底双臂,error 臂不得提前释放）+ acceptPlan/editLastTurn 的持有者 finally 各一处（round-20 M2）'
);
assert.equal(
  (bridgeSource.match(/await waitForAdmissionRelease\(sid\);/g) || []).length,
  4,
  '受理等待者应为：acceptPlan + editLastTurn + doSendFor 直发预留 + aux 派发（四处,缺一即红）'
);
// Round-18 (review JensenChen28 5816589125 P2): the direct-send surface
// (doSendFor) and the aux surface claim the same admission owner before the
// first await (claiming before the first await = later arrivals wait instead
// of querying in parallel), the admitted recheck yields to a requeued queue
// head, and finally releases once settled — drift at any one site goes red.
assert.equal(
  (bridgeSource.match(/ownerToken = claimAdmission\(sid\);/g) || []).length,
  2,
  '直发与 aux 面必须在首 await 前认领受理拥有者（两处,缺一即红）'
);
assert.equal(
  (bridgeSource.match(/const admissionToken = claimAdmission\(sid\);/g) || []).length,
  3,
  '受理持有者应为：flushQueued 队首派发 + acceptPlan + editLastTurn 跨门持有（round-20 M2,三处,缺一即红）'
);
assert.equal(
  (bridgeSource.match(/releaseAdmission\(sid, ownerToken\)/g) || []).length,
  2,
  '直发 finally 释放 + aux finally 释放（各一,缺一即红）'
);
assert.equal(
  (bridgeSource.match(/if \(parkedQueueHoldsPriority\(sid, admissionToken\)\) return;/g) || []).length,
  2,
  'acceptPlan 与 editLastTurn 的 admitted 复查必须各自让位被回队的队首,并携带持有 token 供触发队首前释放（round-20 M2,两处,缺一即红）'
);
// Round-20 (review JensenChen28 5821148893 P2): the post-wakeup recheck on
// the explicit surface must consult both busy and the session queue — after
// the queue head is refused and requeued, a waiter that only checks busy
// would bypass the queue head and direct-send.
assert.equal(
  (bridgeSource.match(/isBusyFor\(sid\) \|\| sidQueuedHeadHolds\(sid\)/g) || []).length,
  1,
  'doSendFor 唤醒后的复查必须覆盖 busy + 队列（缺队列臂即红）'
);
assert.equal(
  (bridgeSource.match(/releaseAdmission\(sid, admissionToken\);\s*flushQueued\(sid\);/g) || []).length,
  2,
  'acceptPlan/editLastTurn 的持有者 finally 必须先释放后补排空（持有窗口内的 turn 终态不再有别的触发点,缺补排空即红）'
);
// Round-31 (review zhuowp 5854578975 P2): the direct-entry top-up — after a
// tokenless (composer / explicit-session) send settles accepted with the
// turn surface already down, doSendFor itself resumes the missed idle drain.
// The two behavior tests above pin the interleaving; this pin keeps the
// release-then-drain pair from silently disappearing (deleting the drain
// leaves the queued head parked forever on the terminal-before-receipt
// interleaving, deleting the accepted guard turns a refused/requeued settle
// into an Auto-refusal spin).
assert.equal(
  (bridgeSource.match(/directAccepted && !isBusyFor\(sid\)\) flushQueued\(sid\);/g) || []).length,
  1,
  '直发受理成功落定后必须由 doSendFor 自身补排空（缺 drain 即红，终态先于回执的交错永不再触发）'
);

console.log('web_queue_admission_fifo: ok');
