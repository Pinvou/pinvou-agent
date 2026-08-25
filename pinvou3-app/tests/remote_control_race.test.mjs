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

test('a stale relay-setting mutation does not start a status readback', async () => {
  for (const [command, startMutation] of [
    ['web_access_set_relay', rt => rt.api.setWebRelayAddress('relay.example')],
    ['web_access_reset_relay', rt => rt.api.resetWebRelayAddress()],
  ]) {
    const rt = loadRemoteControlFeature({ active: true, status: 'connected' });
    const mutation = rt.defer(command);
    const pendingMutation = startMutation(rt);

    await rt.api.stopRemoteControl();
    mutation.resolve({ relay_url: 'stale' });
    await pendingMutation;

    assert.equal(rt.calls.filter(call => call.name === 'web_access_status').length, 0);
    assert.equal(rt.state.webAccess.active, false);
    assert.equal(rt.state.webAccess.status, 'stopped');
  }
});

test('a late relay-setting status readback cannot overwrite a newer stop intent', async () => {
  const rt = loadRemoteControlFeature({ active: true, status: 'connected' });
  const status = rt.defer('web_access_status');
  const pendingMutation = rt.api.setWebRelayAddress('relay.example');

  for (let attempt = 0; attempt < 4
    && !rt.calls.some(call => call.name === 'web_access_status'); attempt += 1) {
    await Promise.resolve();
  }
  assert.equal(rt.calls.some(call => call.name === 'web_access_status'), true);
  await rt.api.stopRemoteControl();
  status.resolve({ active: true, status: 'connected', endpoint_id: 'stale' });
  await pendingMutation;

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
