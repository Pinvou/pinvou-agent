/**
 * saveModel 链尾裁定(review P2×2,晚到完成):单行保存的复合刷新链
 * (save_model → loadModels → …)是 upsert **之后**发起的读取,序号后发者胜
 * ——链尾成功列表缺席本身份 ⇒ 记录在刷新尾窗内被删,调用方不得入 committed
 * 簿(否则簿键永无成功列表可确认,合法同身份重加被永久拦死)。本文件锁定
 * 两个平台桥返回的裁定契约:deferred(批量,无刷新尾)/ failed(刷新失败,
 * 无证据照常入簿)/ ok(附链尾列表内容)。
 * tauri lane 复用 tests/settings_race.test.mjs 的 feature 装载器;web lane
 * 复用 tests/memory_personas_race_web.test.mjs 的整体装载(web 桥是独立
 * 文本镜像,单独锁定防只同步一侧)。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const tauriBridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');
const webBridgeDir = path.join(here, '..', 'src', 'platform', 'web');

// ── tauri lane:settings.js feature 装载(同 settings_race.test.mjs)──

function loadTauriSettings(state, contextOverrides) {
  const root = {};
  const src = fs.readFileSync(path.join(tauriBridgeDir, 'settings.js'), 'utf8');
  vm.runInNewContext(
    fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8') + '\n' + src,
    { window: root, globalThis: root, setTimeout, clearTimeout },
  );
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__['settings'];
  const deferreds = {};
  const calls = { invoke: [] };
  const api = factory(Object.assign({
    state,
    listen() {},
    notify() { calls.notify = (calls.notify || 0) + 1; },
    bt(key) { return key; },
    addSystemItem() {},
    addChatItem() {},
    timeStr() { return ''; },
    invoke(name) {
      calls.invoke.push(name);
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      return Promise.resolve({});
    },
  }, contextOverrides || {}));
  return {
    api,
    state,
    calls,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

const SAVED_RECORD = {
  id: 'm1', model: 'glm-5', base_url: 'https://api.example.com/v1', name: 'GLM',
};

function makeState() {
  return { settings: {} };
}

test('tauri saveModel ok: the outcome carries the tail list so the caller can reconcile presence', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  // 刷新尾的 list_models 返回含该身份的权威列表(upsert 后的读取)。
  d.resolve({ models: [SAVED_RECORD, { id: 'm0', model: 'other', base_url: 'https://x/v1' }], active_model_id: 'm0' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '成功刷新给 ok 裁定');
  assert.ok(Array.isArray(outcome.models), 'ok 裁定必须携带链尾列表');
  assert.equal(outcome.models.length, 2, '列表元素与成功读取一致');
  assert.equal(outcome.models[0].id, 'm1', '列表首元素为刚落库记录(逐元素断言,跨 realm 不用 deepEqual)');
  // 链跑全:save_model + list_models + get_settings + 有效配置读取。
  for (const cmd of ['save_model', 'list_models']) {
    assert.ok(rt.calls.invoke.includes(cmd), `链必须包含 ${cmd}`);
  }
});

test('tauri saveModel ok with the identity absent: the tail list reflects a mid-tail deletion (verdict data)', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  // 刷新尾窗内的删除赢下序号:链尾列表不再含该身份——调用方按裁定跳过入簿。
  d.resolve({ models: [{ id: 'm0', model: 'other', base_url: 'https://x/v1' }], active_model_id: 'm0' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '列表读取本身成功,裁定仍是 ok');
  assert.equal(outcome.models.some(m => m && m.id === 'm1'), false, '链尾列表缺席该身份——入簿与否由调用方按此裁定');
});

test('tauri saveModel failed: the swallowed list read still resolves with the failed verdict (no evidence)', async () => {
  const rt = loadTauriSettings(makeState());
  const d = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  d.reject(new Error('list read failure')); // loadModels 吞错清列表(r3 lane G 形态)
  const outcome = await p;
  assert.equal(outcome.refresh, 'failed', '刷新失败给 failed 裁定');
  assert.equal(outcome.models, undefined, 'failed 裁定不携带列表——调用方无证据,照常入簿');
  assert.equal(rt.state.modelsListLoadFailed, true, '失败标记已置(镜像 effect 冻结源)');
});

test('tauri saveModel deferred: no reload tail runs and the outcome carries no list evidence', async () => {
  const rt = loadTauriSettings(makeState());
  const outcome = await rt.api.saveModel(SAVED_RECORD, { deferRefresh: true });
  assert.equal(outcome.refresh, 'deferred', '批量跳过刷新尾给 deferred 裁定');
  assert.equal(outcome.models, undefined, 'deferred 无列表证据——调用方入簿且不带缺席地板');
  assert.deepEqual(rt.calls.invoke, ['save_model'], 'deferred 只发 save_model,三个 reload 一个都不跑');
});

test('tauri saveModel superseded: a read discarded by a newer pending request degrades the verdict (r10 P2)', async () => {
  const rt = loadTauriSettings(makeState());
  const d1 = rt.defer('list_models');              // 先挂 N 的 defer(链 invoke 时捕获)
  const p = rt.api.saveModel(SAVED_RECORD);
  // 推进到链自己的 list_models invoke(微任务清空),确保 N 已链上 d1——
  // 之后 defer 同名替换只影响后续调用。
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');              // 并发更新请求 N+1(仍 pending)
  const p2 = rt.api.loadModels();
  // N 携带含 A 的列表后解析——但 N+1 已领取更大序号,N 被后发者胜作废,
  // 不发布任何东西;链尾 state 仍是 upsert 之前的旧世界。
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'superseded', '被作废的读取不得当确认观察——裁定退化为 superseded');
  assert.equal(outcome.models, undefined, 'superseded 无列表证据——调用方照常入簿(缺席地板,后继受理观察退休)');
  // 清尾:N+1 最终受理(无论 payload),不留挂起句柄。
  d2.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  await p2;
  assert.equal(rt.state.savedModels.length, 1, '后继受理读取正常写入');
});

// ── web lane:整体装载(platform/web/bridge.js + domain-adapter)────────

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
  const handlers = {};
  const deferreds = {};
  const windowObject = {
    PinvouPlatform: { kind: 'web', isWeb: true, capabilities: {}, can: () => false, canInvoke: () => false },
    __TAURI__: {
      core: { invoke: (name) => {
        calls.invoke.push(name);
        if (handlers[name]) return handlers[name]();
        if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
        return Promise.resolve(null);
      } },
      event: { listen: async () => function () {} },
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
  vm.runInContext(fs.readFileSync(path.join(here, '..', 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'), context, { filename: 'shared/bridge-shared-helpers.js' });
  vm.runInContext(fs.readFileSync(path.join(webBridgeDir, 'bridge.js'), 'utf8'), context, { filename: 'platform/web/bridge.js' });
  const flat = windowObject.TauriBridge;
  return {
    flat,
    calls,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
    setHandler(name, fn) { handlers[name] = fn; },
  };
}

test('web saveModel ok/failed/deferred mirror the tauri verdict contract', async () => {
  const rt = bootWebBridge();
  // ok:刷新尾读取返回含该身份的列表。
  rt.setHandler('list_models', () => Promise.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' }));
  const okOutcome = await rt.flat.saveModel(SAVED_RECORD);
  assert.equal(okOutcome.refresh, 'ok', 'web lane 成功刷新同样给 ok 裁定');
  assert.ok(Array.isArray(okOutcome.models), 'ok 裁定携带链尾列表');
  assert.equal(okOutcome.models[0].id, 'm1', '列表元素与成功读取一致(逐元素断言)');
  // ok 但列表缺席(刷新尾窗内被删):裁定数据交调用方跳过入簿。
  rt.setHandler('list_models', () => Promise.resolve({ models: [], active_model_id: null }));
  const absentOutcome = await rt.flat.saveModel({ ...SAVED_RECORD, id: 'm2' });
  assert.equal(absentOutcome.refresh, 'ok', '读取成功裁定仍 ok');
  assert.equal(absentOutcome.models.length, 0, '缺席裁定:调用方按列表内容自行跳过入簿');
  // failed:loadModels 吞错 → failed 裁定无列表证据。
  rt.setHandler('list_models', () => Promise.reject(new Error('list read failure')));
  const failedOutcome = await rt.flat.saveModel({ ...SAVED_RECORD, id: 'm3' });
  assert.equal(failedOutcome.refresh, 'failed', 'web lane 刷新失败同样给 failed 裁定');
  assert.equal(failedOutcome.models, undefined, 'failed 不携带列表');
  // deferred:批量只发 save_model。
  const deferredOutcome = await rt.flat.saveModel(SAVED_RECORD, { deferRefresh: true });
  assert.equal(deferredOutcome.refresh, 'deferred', 'web lane 批量同样给 deferred 裁定');
  assert.equal(deferredOutcome.models, undefined, 'deferred 无列表证据');
});

test('tauri saveModel superseded with a newer accepted publication: the published generation upgrades the verdict (r11 P2)', async () => {
  const rt = loadTauriSettings(makeState());
  // 预置受理世界 [B]:先跑一次受理读取(publishedSeq=1)。
  const d0 = rt.defer('list_models');
  const p0 = rt.api.loadModels();
  d0.resolve({ models: [{ id: 'b0', model: 'b-model', base_url: 'https://b/v1' }], active_model_id: 'b0' });
  await p0;
  // 保存 A:链读取 N(seq=2)挂起;并发更新 N+1(seq=3)挂起。
  const d1 = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  // N+1 先落地 [B,A](受理,publishedSeq=3>2)——链尾已发布的观察是
  // upsert 之后的确认证据;再解析 N(作废)。裁定应按已发布观察升级为
  // ok:A 在场(评审日程一:present→带地板入簿)。
  d2.resolve({ models: [{ id: 'b0', model: 'b-model', base_url: 'https://b/v1' }, SAVED_RECORD], active_model_id: 'm1' });
  // 生产时序:N+1 的响应先抵达(发布完成),原始保存尾在真实间隔后才收尾
  // ——对齐之,避免微任务跳数差异把发布排到作废读取之后。
  await p2;
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '已发布的受理观察(refresh)后于 upsert → ok 裁定');
  assert.ok(Array.isArray(outcome.models) && outcome.models.length === 2, '证据用已发布列表');
  assert.equal(outcome.models[1].id, 'm1', '元素逐位一致');
  await p2;
  // 评审日程二:N+1 落地后用户删除 A,新受理观察缺席 A → ok+缺席=已删。
  const d3 = rt.defer('list_models');
  const p3 = rt.api.saveModel({ ...SAVED_RECORD, id: 'm9' });
  await new Promise(resolve => { setTimeout(resolve, 0); });
  d3.resolve({ models: [{ id: 'b0', model: 'b-model', base_url: 'https://b/v1' }], active_model_id: 'b0' });
  const outcome2 = await p3;
  assert.equal(outcome2.refresh, 'ok', '本轮链读取直接受理,裁定 ok');
  assert.equal(outcome2.models.some(m => m && m.id === 'm9'), false, '缺席语义照常');
});

test('tauri saveModel superseded with a failed newer publication: stays superseded (r11 evidence invalid)', async () => {
  const rt = loadTauriSettings(makeState());
  const d1 = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  d2.reject(new Error('newer read failed'));          // 受理失败:发布失败标记,无列表证据
  await p2.catch(() => {});
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' }); // N 作废
  const outcome = await p;
  assert.equal(outcome.refresh, 'superseded', '失败的新近发布不能当确认观察——维持 superseded 保护');
  assert.equal(outcome.models, undefined, '无列表证据');
  await p2;
});

const B_REC = { id: 'b0', model: 'b-model', base_url: 'https://b/v1' };

test('tauri loadModels degraded read: a fallback success is failure-equivalent and publishes no generation (r13 P2)', async () => {
  const rt = loadTauriSettings(makeState());
  // 预置受理世界 [B](publishedSeq=1)。
  const d0 = rt.defer('list_models');
  const p0 = rt.api.loadModels();
  d0.resolve({ models: [B_REC], active_model_id: 'b0' });
  await p0;
  // 降级读取:成功响应 + fallback 标记(settings.json 读不出/损坏 → 安全
  // 默认值)。按失败等价:清显示列表、冻结守卫、不发布代际。
  const d1 = rt.defer('list_models');
  const p1 = rt.api.loadModels();
  d1.resolve({ models: [], active_model_id: null, fallback: true });
  const r1 = await p1;
  assert.equal(r1.accepted, true, '降级读取仍算受理(链跑过)');
  assert.equal(r1.failed, true, 'fallback 标记按失败等价处理');
  assert.equal(rt.state.savedModels.length, 0, '显示列表清空(r3 lane G)');
  assert.equal(rt.state.modelsListLoadFailed, true, '失败标记置位(r7 冻结)');
  assert.equal(rt.state.lastConfirmedModels.length, 1, 'lastConfirmedModels 不被降级读取覆盖(跨 realm 逐元素比较)');
  assert.equal(rt.state.lastConfirmedModels[0] && rt.state.lastConfirmedModels[0].id, 'b0', '快照保留首个受理读取的记录');
  // 代际未发布:superseded 分类在该窗口内维持旧世界(superseded),不会把
  // 降级缺席当删除证据升级 ok。
  const d2 = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d3 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  d3.resolve({ models: [B_REC], active_model_id: 'b0', fallback: true }); // N+1 也降级:无发布
  await p2.catch(() => {});
  d2.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' }); // N 作废
  const outcome = await p;
  assert.equal(outcome.refresh, 'superseded', '降级发布不携带代际——无证据维持 superseded 保护');
  assert.equal(outcome.models, undefined, '无列表证据');
  // 恢复正常受理读取后,冻结解冻、代际前进、守卫移交列表。
  const d4 = rt.defer('list_models');
  const p3 = rt.api.loadModels();
  d4.resolve({ models: [B_REC], active_model_id: 'b0' });
  await p3;
  assert.equal(rt.state.modelsListLoadFailed, false, '下一次真实成功读取清失败标记');
  await p2;
});

test('web loadModels degraded read mirrors the failure-equivalent contract (r13 P2)', async () => {
  const rt = bootWebBridge();
  // flat 不暴露 loadModels——走保存链观察:降级读取 ⇒ failed 裁定(入簿无证据)。
  rt.setHandler('list_models', () => Promise.resolve({ models: [], active_model_id: null, fallback: true }));
  const outcome = await rt.flat.saveModel(SAVED_RECORD);
  assert.equal(outcome.refresh, 'failed', 'web lane 降级读取同样按失败等价:failed 裁定');
  assert.equal(outcome.models, undefined, '无列表证据');
});



test('tauri saveModel: publications landing during the remaining refresh are read live at completion (r12 P2)', async () => {
  const rt = loadTauriSettings(makeState());
  // 预置受理世界 [B](publishedSeq=1)。
  const d0 = rt.defer('list_models');
  const p0 = rt.api.loadModels();
  d0.resolve({ models: [B_REC], active_model_id: 'b0' });
  await p0;
  // 保存 A:链读取 N(seq=2)挂 d1;并发更新 N+1(seq=3)挂 d2,**先不落地**
  // ——与 r11 日程的区别就在这里:N 作废先返回,此刻证据仍停在 upsert 之前。
  const d1 = rt.defer('list_models');
  const p = rt.api.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');
  const p2 = rt.api.loadModels();
  // N 作废返回(快照 publishedSeq=1);链随即进入剩余刷新——挂起
  // get_settings 打开尾部窗口。
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  const dSettings = rt.defer('get_settings');
  // 窗口内:N+1 受理落地 [B,A](publishedSeq=3);用户删除 A,删除链受理
  // [B](publishedSeq=4,缺席 A=upsert 之后的确认删除)。
  d2.resolve({ models: [B_REC, SAVED_RECORD], active_model_id: 'm1' });
  await p2;
  const dDel = rt.defer('delete_model');
  const p3 = rt.api.deleteModel('m1');
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d3 = rt.defer('list_models');
  dDel.resolve({});
  await new Promise(resolve => { setTimeout(resolve, 0); });
  d3.resolve({ models: [B_REC], active_model_id: 'b0' });
  // 放行剩余刷新:链尾分类读取的是**完成时点**的活体证据(publishedSeq=4),
  // 而非 N 返回时点的快照(publishedSeq=1)——缺席升级为 ok,预订被跳过。
  dSettings.resolve({});
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', '尾部窗口内的受理发布对快照不可见——活体证据在链完成时点升级裁定');
  assert.ok(Array.isArray(outcome.models) && outcome.models.length === 1, '证据用活体列表');
  assert.equal(outcome.models.some(m => m && m.id === 'm1'), false, '确认删除:调用方按裁定跳过入簿');
  await p3;
});

test('web saveModel mirrors the superseded verdict (r10 P2)', async () => {
  const rt = bootWebBridge();
  const d1 = rt.defer('list_models');              // 先挂 N 的 defer(链 invoke 时捕获)
  const p = rt.flat.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');              // 并发更新请求 N+1(仍 pending)
  // flat 不暴露 loadModels——用 deleteModel(带 loadModels 链,facade 有)触发。
  const p2 = rt.flat.deleteModel('trigger-n1');
  // deleteModel 链到它自己的 list_models invoke 还差一跳,先推进再作废 N。
  await new Promise(resolve => { setTimeout(resolve, 0); });
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' }); // N 后解析→作废
  const outcome = await p;
  assert.equal(outcome.refresh, 'superseded', 'web lane 同样退化 superseded');
  assert.equal(outcome.models, undefined, 'superseded 无列表证据');
  d2.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  await p2;
});

test('web saveModel: publications during the remaining refresh are read live (r12 P2 mirror)', async () => {
  const rt = bootWebBridge();
  // 预置受理世界 [B](publishedSeq=1):一次性 handler 让 deleteModel 链受理。
  rt.setHandler('list_models', () => Promise.resolve({ models: [B_REC], active_model_id: 'b0' }));
  await rt.flat.deleteModel('seed-b0');
  // 保存 A:N(seq=2)挂 d1;N+1(seq=3)由 deleteModel 链触发挂 d2,先不落地。
  rt.setHandler('list_models', null);
  const d1 = rt.defer('list_models');
  const p = rt.flat.saveModel(SAVED_RECORD);
  await new Promise(resolve => { setTimeout(resolve, 0); });
  const d2 = rt.defer('list_models');
  const p2 = rt.flat.deleteModel('trigger-n1');
  await new Promise(resolve => { setTimeout(resolve, 0); });
  // N 作废先返回(快照停在 upsert 前世界);web 剩余刷新只有有效配置读取
  // ——挂起 get_effective_model_config 打开尾部窗口。
  d1.resolve({ models: [SAVED_RECORD], active_model_id: 'm1' });
  const dCfg = rt.defer('get_effective_model_config');
  // 窗口内:N+1 受理 [B,A](publishedSeq=3);再删 A:handler 放行 [B]
  // (publishedSeq=4,确认删除)。两条删除链的链尾同样挂在 dCfg 上。
  d2.resolve({ models: [B_REC, SAVED_RECORD], active_model_id: 'm1' });
  await new Promise(resolve => { setTimeout(resolve, 0); });
  rt.setHandler('list_models', () => Promise.resolve({ models: [B_REC], active_model_id: 'b0' }));
  const p3 = rt.flat.deleteModel('m1');
  await new Promise(resolve => { setTimeout(resolve, 0); });
  // 放行:链尾分类读活体证据(publishedSeq=4)而非快照(publishedSeq=1)。
  dCfg.resolve({});
  const outcome = await p;
  assert.equal(outcome.refresh, 'ok', 'web lane 同样在链完成时点读取活体证据');
  assert.ok(Array.isArray(outcome.models) && outcome.models.length === 1, '证据用活体列表');
  assert.equal(outcome.models.some(m => m && m.id === 'm1'), false, '确认删除:跳过入簿');
  await p2;
  await p3;
});
