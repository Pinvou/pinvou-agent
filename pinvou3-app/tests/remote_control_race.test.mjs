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

function loadRemoteControlFeature() {
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
  const state = { webAccess: {} };
  const api = factory({
    state,
    notify() {},
    bt(key) { return key; },
    listen() { return Promise.resolve(function () {}); },
    invoke(name) {
      if (deferreds[name] && deferreds[name].promise) return deferreds[name].promise;
      return Promise.resolve({});
    },
  });
  return {
    api,
    state,
    defer(name) {
      const d = {};
      d.promise = new Promise((resolve, reject) => { d.resolve = resolve; d.reject = reject; });
      deferreds[name] = d;
      return d;
    },
  };
}

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
