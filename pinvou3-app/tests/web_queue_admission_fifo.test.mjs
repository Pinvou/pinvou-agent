// Web 排队受理 FIFO（评审 JensenChen28 5804488461 P2）：flushQueued 在
// doSendFor 的 get_mode_state 姿态门往返期间 busy 仍为 false，mode_changed
// 等异步触发可在该窗口内重入并 shift 走下一条；若第二条的查询先决，它先于
// 队首受理——FIFO 失序且后端占用栅栏无法还原顺序。修复 = 按 sid 持有受理
// 拥有者跨 await，落定（受理成功/拒绝回队）即释放。本测试用 per-command
// deferred 复现重叠触发 + 逆序查询完成（评审要求的形态），断言窗口内：
// 队列不动、只有一次姿态查询；队首受理后剩余消息仍按队列序逐条发出。
// 第二轮（评审 JensenChen28 5810381950 P2）：受理在途窗口内的新输入不得走
// 直发路径——队首被受理 owner shift 走后 queued.length===0 且 busy===false，
// 直发检查若不咨询受理拥有者，新输入会绕过队首起第二条查询（D,A 失序）。
// 运行时回归与 web_send_gate_consumed_state_restore 同款 vm harness。
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

  // 第一轮：A 直发并被受理（web_access_chat 挂起保持 busy 面）。
  // 姿态门：A 的查询即时放行；此后（B 的查询）挂起，直到 gateRelease。
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

  // B、C 排队。
  await rt.flat.sendMessage('B');
  await rt.flat.sendMessage('C');
  assert.equal(rt.view().queued.length, 2, 'precondition: B、C 入队');

  // turn 终态 → flush 队首 B，其姿态门查询挂起（受理在途窗口）。
  const doneSeq = 100 + 1; // payload 只需 terminal 序可比的形状；见 chat:done 消费
  rt.emit('chat:done', { session_id: 'chat-a', seq: doneSeq });
  await tick(); await tick();
  assert.equal(rt.view().busy, false, 'A 轮终态后 busy 落下');
  assert.equal(gateCalls, 2, 'B 的姿态门查询已起');
  assert.equal(rt.view().queued.length, 1, '仅队首 B 出队（C 仍在队）');

  // 窗口内重入触发（mode_changed 非 Auto → flushQueued）：修复前这里会
  // shift 走 C 并起第二条查询；修复后让路。
  rt.emit('chat:mode_changed', { session_id: 'chat-a', mode: 'plan' });
  await tick(); await tick();
  assert.equal(gateCalls, 2, '受理在途时重入触发不得再起姿态查询');
  assert.equal(rt.view().queued.length, 1, '重入触发不得 shift 走 C');
  assert.equal(rt.view().busy, false, '重入触发不得伪造 busy 面');

  // B 的查询（非 Auto）放行 → B 受理。
  gateRelease({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(rt.view().busy, true, 'B 受理后 busy 顶上');
  const bPosition = JSON.stringify(rt.view().messages).indexOf('"B"');
  assert.ok(bPosition >= 0, 'B 的乐观提交已入 transcript');

  // B 轮终态 → flush C，C 的查询放行 → C 受理。顺序必须 B 先 C 后。
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

  // 姿态门可控：A 直发放行；B 的 flush 命中 Auto；离开 Auto 后再放行。
  let autoNow = false;
  rt.setHandler('get_mode_state', () => Promise.resolve({ mode: autoNow ? 'auto' : 'plan', multi_agent: false }));
  rt.setHandler('web_access_chat', () => Promise.resolve({}));
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');
  await rt.flat.sendMessage('B');
  assert.equal(rt.view().queued.length, 1, 'precondition: B 入队');

  autoNow = true; // 队首 B 的重试命中 Auto 姿态门
  rt.emit('chat:done', { session_id: 'chat-a', seq: 201 });
  await tick(); await tick();
  // B 被 Auto 门拒绝：web_access_chat 不被调用，B 原位回队，受理拥有者释放。
  assert.equal(rt.view().queued.length, 1, 'Auto 拒绝后队首回队（原行为不变）');
  assert.equal(rt.view().busy, false, '拒绝不开 turn');
  // 离开 Auto 的 mode_changed 触发重试：这次查询放行 → B 受理。
  autoNow = false;
  rt.emit('chat:mode_changed', { session_id: 'chat-a', mode: 'plan' });
  await tick(); await tick();
  assert.equal(rt.view().busy, true, '离开 Auto 后重试受理由 mode_changed 触发照常完成');
  assert.equal(rt.view().queued.length, 0, '重试消费队首');
});

test('web 排队受理 FIFO：受理在途窗口的新输入必须入队，不得直发绕过队首', async () => {
  const rt = bootWebBridge();
  await primeSessionA(rt);

  // A 直发并受理（查询 1 即时放行）；后续查询挂起直到 gateRelease。
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

  // B 入队；turn 终态 → flush 取走 B，其姿态门查询挂起（受理在途窗口）。
  await rt.flat.sendMessage('B');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 301 });
  await tick(); await tick();
  assert.equal(rt.view().busy, false, 'B 在姿态门窗口：busy 未顶上');
  assert.equal(rt.view().queued.length, 0, 'B 已被受理 owner shift 出队');
  assert.equal(gateCalls, 2, 'B 的姿态门查询在途');

  // 窗口内新输入 D：修复前两个直发检查都不咨询受理拥有者，D 直发并起第二
  // 条查询挂住（D 先决即 D,A 失序）；修复后 D 入队排在 B 后，不起新查询。
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

  // B 放行受理 → B 轮终态 → flush D，放行 → D 受理。顺序必须 B 先 D 后。
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

  // 姿态门与回执全部可控：A 即时放行即时回执；B 放行后回执挂起；C 随后放行。
  let gateCalls = 0;
  const gateQueue = [];
  rt.setHandler('get_mode_state', () => {
    gateCalls += 1;
    return new Promise(resolve => gateQueue.push(resolve));
  });
  let chatCalls = 0;
  const chatQueue = [];
  rt.setHandler('web_access_chat', () => {
    chatCalls += 1;
    return new Promise(resolve => chatQueue.push(resolve));
  });
  // 第一轮 A：直发，查询与回执按序放行。
  const sendA = rt.flat.sendMessage('A');
  await tick(); await tick();
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  chatQueue.shift()({});
  await tick(); await tick();
  await sendA;
  assert.equal(rt.view().busy, true, 'precondition: A 已受理');

  // B、C 入队 → A 终态 → flush 取 B，B 的查询放行受理，回执挂起。
  await rt.flat.sendMessage('B');
  await rt.flat.sendMessage('C');
  rt.emit('chat:done', { session_id: 'chat-a', seq: 401 });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, 'precondition: 仅队首 B 出队（C 在队）');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 2, 'precondition: B 的 web_access_chat 回执在途');

  // B 的终态先于回执：busy 落下，done 触发的 flush 被在途拥有者挡回，C 不动。
  rt.emit('chat:done', { session_id: 'chat-a', seq: 402 });
  await tick(); await tick();
  assert.equal(rt.view().queued.length, 1, '拥有者在途：终态触发的 flush 不得动 C');

  // B 的回执到达：受理成功落定处释放并补排空——C 自动拉起（无任何外部触发）。
  chatQueue.shift()({});
  await tick(); await tick();
  assert.equal(gateCalls, 3, '补排空必须拉起 C 的姿态查询');
  gateQueue.shift()({ mode: 'plan', multi_agent: false });
  await tick(); await tick();
  assert.equal(chatCalls, 3, '补排空必须发出 C');
  const orderText = JSON.stringify(rt.view().messages);
  assert.ok(
    orderText.indexOf('"B"') >= 0 && orderText.indexOf('"C"') > orderText.indexOf('"B"'),
    'B 必须先于 C 入 transcript（FIFO）'
  );
  assert.equal(rt.view().queued.length, 0, '队列清空');
});

console.log('web_queue_admission_fifo: ok');
