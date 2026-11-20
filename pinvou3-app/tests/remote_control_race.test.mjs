/**
 * 远程控制意图序号回归测试（PR #260 审计补充）：
 * start 在途被 stop 顶掉（陈旧 start 不写状态）后，stop 的写入必须清掉
 * starting 标志——否则 UI 永卡「启动中」（Rust 事件 payload 不含 starting，
 * 无人兜底收敛）。
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const bridgeDir = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge');

function loadRemoteControlFeature(initialWebAccess = {}) {
  const root = {};
  const src = fs.readFileSync(path.join(bridgeDir, 'remote-control.js'), 'utf8');
  vm.runInNewContext(src, {
    window: root,
    globalThis: root,
    setTimeout,
    clearTimeout,
  });
  const factory = root.__PINVOU_TAURI_BRIDGE_FEATURES__['remote-control'];
  const deferreds = {};
  const calls = [];
  const state = { webAccess: { ...initialWebAccess } };
  const api = factory({
    state,
    notify() {},
    bt(key) { return key; },
    listen() { return Promise.resolve(function () {}); },
    invoke(name, args) {
      calls.push({ name, args });
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      return Promise.resolve({});
    },
  });
  return {
    api,
    calls,
    state,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

test('start forwards host-workspace consent and restores an existing endpoint on failure', async () => {
  const rt = loadRemoteControlFeature({
    active: true,
    web_client_connected: true,
    host_workspace_authorized: true,
    status: 'connected',
  });
  const dEnable = rt.defer('web_access_enable');
  const pending = rt.api.startRemoteControl({ allowHostWorkspace: true });

  const enableArgs = rt.calls.find(call => call.name === 'web_access_enable')?.args;
  assert.equal(enableArgs?.allowHostWorkspace, true, 'explicit desktop consent must reach the backend command');
  assert.deepEqual(Object.keys(enableArgs || {}), ['allowHostWorkspace'], 'the enable payload stays narrow');

  dEnable.reject(new Error('enable failed'));
  await assert.rejects(pending, /enable failed/);
  assert.equal(rt.state.webAccess.active, true, 'a failed restart must preserve the active endpoint');
  assert.equal(rt.state.webAccess.web_client_connected, true, 'a failed restart must preserve connection state');
  assert.equal(rt.state.webAccess.host_workspace_authorized, true, 'a failed restart must preserve prior authorization');
  assert.equal(rt.state.webAccess.starting, false, 'a failed restart must clear starting');
  assert.equal(rt.state.webAccess.status, 'error', 'the fresh failure remains visible');
});

test('stop clears host-workspace authorization together with terminal state', async () => {
  const rt = loadRemoteControlFeature({
    active: true,
    web_client_connected: true,
    host_workspace_authorized: true,
    starting: true,
  });

  await rt.api.stopRemoteControl();

  assert.equal(rt.state.webAccess.active, false);
  assert.equal(rt.state.webAccess.web_client_connected, false);
  assert.equal(rt.state.webAccess.host_workspace_authorized, false);
  assert.equal(rt.state.webAccess.starting, false);
  assert.equal(rt.state.webAccess.status, 'stopped');
});

test('a late status refresh cannot overwrite a newer stop intent', async () => {
  const rt = loadRemoteControlFeature({ active: true, status: 'connected' });
  const status = rt.defer('web_access_status');
  const pendingRefresh = rt.api.refreshRemoteControlStatus();

  await rt.api.stopRemoteControl();
  status.resolve({ active: true, status: 'connected', endpoint_id: 'stale' });
  await pendingRefresh;

  assert.equal(rt.state.webAccess.active, false);
  assert.equal(rt.state.webAccess.status, 'stopped');
  assert.equal(rt.state.webAccess.endpoint_id, null);
});

test('stop 顶掉在途 start 后 starting 必须被清除（不残留启动中）', async () => {
  const rt = loadRemoteControlFeature();
  // 用户点启动：starting:true 置位，enable 在途（seq=1）。
  const dEnable = rt.defer('web_access_enable');
  const pStart = rt.api.startRemoteControl();
  // 启动在途时用户点停止（seq=2）：disable 先完成并写终态。
  const dDisable = rt.defer('web_access_disable');
  const pStop = rt.api.stopRemoteControl();
  dDisable.resolve({});
  await pStop;
  assert.equal(rt.state.webAccess.starting, false, 'stop 的成功写入必须清掉 starting');
  assert.equal(rt.state.webAccess.active, false, 'stop 正常写入终态');
  // 迟到的 enable 此刻才返回（seq 已陈旧）：不写任何状态。
  dEnable.resolve({ url: 'https://example.test' });
  const info = await pStart;
  assert.equal(rt.state.webAccess.active, false, '陈旧 start 不得把 stopped 写回 active');
  assert.equal(rt.state.webAccess.starting, false, '陈旧 start 返回后 starting 不得复活');
  assert.deepEqual(info, { url: 'https://example.test' }, '陈旧 start 仍返回原始结果');
});

test('stop 新鲜失败必须清除 starting（start 被顶掉后无人兜底）', async () => {
  const rt = loadRemoteControlFeature();
  // 用户点启动：starting:true 置位，enable 在途（seq=1）。
  const dEnable = rt.defer('web_access_enable');
  const pStart = rt.api.startRemoteControl();
  // 启动在途时用户点停止（seq=2 顶掉 start），disable 失败（新鲜失败）。
  const dDisable = rt.defer('web_access_disable');
  const pStop = rt.api.stopRemoteControl();
  dDisable.reject(new Error('disable failed'));
  await pStop.catch(() => { /* 已知 reject */ });
  assert.equal(rt.state.webAccess.starting, false, 'stop 失败写入必须清掉 starting');
  assert.equal(rt.state.webAccess.status, 'error', 'stop 失败写入 error 终态');
  // 迟到的 enable 此刻才返回（陈旧）：不写任何状态，starting 不得复活。
  dEnable.resolve({ url: 'https://example.test' });
  await pStart;
  assert.equal(rt.state.webAccess.starting, false, '陈旧 start 返回后 starting 不得复活');
  assert.equal(rt.state.webAccess.active, undefined, '陈旧 start 不写任何状态');
});

test('#876 P2: stop 带 deferred_full_queue 处置必须落到 revocation_warning（本地完成≠远端撤销）', async () => {
  const rt = loadRemoteControlFeature({ active: true, status: 'connected' });
  const dDisable = rt.defer('web_access_disable');
  const pStop = rt.api.stopRemoteControl();
  dDisable.resolve({ revocation: 'deferred_full_queue' });
  await pStop;
  assert.equal(rt.state.webAccess.status, 'stopped', '本地停止已完成：status 保持 stopped');
  assert.equal(rt.state.webAccess.active, false, '本地停止已完成：端点拆除');
  assert.equal(rt.state.webAccess.last_error, null, '这不是错误：不得走 last_error 通道');
  assert.equal(
    rt.state.webAccess.revocation_warning,
    'deferred_full_queue',
    'queue-full 处置必须以持久警告呈现，不得吞成无声的干净停止'
  );

  // 状态刷新不得抹除警告（刷新 payload 不携带 revocation_warning）。
  await rt.api.refreshRemoteControlStatus();
  assert.equal(
    rt.state.webAccess.revocation_warning,
    'deferred_full_queue',
    '警告必须跨状态刷新存续'
  );

  // 后续 stop 落了 queued（意图已持久）：警告清除。
  const d2 = rt.defer('web_access_disable');
  const p2 = rt.api.stopRemoteControl();
  d2.resolve({ revocation: 'queued' });
  await p2;
  assert.equal(rt.state.webAccess.revocation_warning, null, 'queued 处置清除警告');
});

test('#876 P2: stop 无处置（not_needed/无旧凭据）不得留下警告', async () => {
  const rt = loadRemoteControlFeature({ revocation_warning: 'deferred_full_queue' });
  const dDisable = rt.defer('web_access_disable');
  const pStop = rt.api.stopRemoteControl();
  dDisable.resolve({ revocation: 'not_needed' });
  await pStop;
  assert.equal(rt.state.webAccess.revocation_warning, null, 'not_needed 清除警告');
});

test('#876 P2: rotate 的 deferred_full_queue 处置同样落警告，queued 清除', async () => {
  const rt = loadRemoteControlFeature({ active: true, status: 'connected' });
  const dRotate = rt.defer('web_access_rotate');
  const pRotate = rt.api.refreshRemoteControlQr();
  dRotate.resolve({
    url: 'https://example.test/new',
    revocation_disposition: 'deferred_full_queue',
  });
  await pRotate;
  assert.equal(
    rt.state.webAccess.revocation_warning,
    'deferred_full_queue',
    'rotate 的 queue-full 处置必须落警告'
  );

  const d2 = rt.defer('web_access_rotate');
  const p2 = rt.api.refreshRemoteControlQr();
  d2.resolve({
    url: 'https://example.test/newer',
    revocation_disposition: 'queued',
  });
  await p2;
  assert.equal(rt.state.webAccess.revocation_warning, null, 'rotate 的 queued 清除警告');
});

test('#876 P3: 三语 i18n 都携带 revocationHints.deferred_full_queue 恢复指引', async () => {
  const i18nDir = path.join(here, '..', 'src', 'shared', 'i18n');
  for (const loc of ['zh', 'en', 'ja']) {
    const names = { zh: 'dictZh', en: 'dictEn', ja: 'dictJa' };
    const mod = await import(path.join(i18nDir, `${loc}.js`) + `?v=${Date.now()}`);
    const dict = mod[names[loc]] ?? mod.default;
    const hint = dict?.uiRemote?.revocationHints?.deferred_full_queue;
    assert.equal(typeof hint, 'string', `${loc}.uiRemote.revocationHints.deferred_full_queue 必须是字符串`);
    assert.ok(hint.length > 20, `${loc} 的恢复指引不得为占位符`);
  }
});

test('rotate 新鲜失败必须清除 starting', async () => {
  const rt = loadRemoteControlFeature();
  const dEnable = rt.defer('web_access_enable');
  const pStart = rt.api.startRemoteControl();        // seq=1，starting:true
  const dRotate = rt.defer('web_access_rotate');
  const pRotate = rt.api.refreshRemoteControlQr();   // seq=2 顶掉 start
  dRotate.reject(new Error('rotate failed'));        // rotate 新鲜失败
  await pRotate.catch(() => { /* 已知 reject */ });
  assert.equal(rt.state.webAccess.starting, false, 'rotate 失败写入必须清掉 starting');
  dEnable.resolve({});
  await pStart;                                      // 陈旧 start：不写任何状态
  assert.equal(rt.state.webAccess.starting, false, '陈旧 start 返回后 starting 不得复活');
});
