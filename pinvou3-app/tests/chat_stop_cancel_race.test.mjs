#!/usr/bin/env node
// PR #532 审阅回归：停止按钮的 cancel_generation 必须带传输层超时竞速。卡死的
// 引擎（turn_lock 被占、不 drain）永远不会 settle 这个 invoke；没有竞速时
// cancelGeneration 永不返回，ChatView 的 cancellingSessionIds 里该 sid 永远
// 不被清除——用户唯一的恢复动作（停止按钮）就此永久禁用。竞速 + finally 里的
// clearTimeout 保证：超时路径按时返回；正常路径定时器先被清除、不会产生
// unhandledrejection；晚到的 cancel 拒绝被预挂的 catch 吞掉（cancel 幂等，
// 晚到的 settle/reject 无害）。Tauri 桥与 Web 桥共享 ChatView，两个平台必须
// 有同样的兜底。
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const tauriChatPath = path.join(here, '..', 'src', 'platform', 'tauri', 'bridge', 'chat.js');
const webBridgePath = path.join(here, '..', 'src', 'platform', 'web', 'bridge.js');
const tauriChatSource = fs.readFileSync(tauriChatPath, 'utf8');
const webBridgeSource = fs.readFileSync(webBridgePath, 'utf8');

// ── Tauri 桥：cancelGeneration 与中断路径共用 STEER_INVOKE_TIMEOUT_MS 竞速 ──

function extractFunction(source, name) {
  const start = source.indexOf(`async function ${name}()`);
  assert.notEqual(start, -1, `${name} must be declared in the source`);
  const end = source.indexOf('\n  }', start);
  assert.notEqual(end, -1, `${name} body must be closable`);
  return source.slice(start, end);
}

const tauriCancel = extractFunction(tauriChatSource, 'cancelGeneration');

// 1. 走 Promise.race 竞速，且用与中断路径相同的中断预算。
assert.match(
  tauriCancel,
  /await Promise\.race\(\[cancelPromise, cancelTimeout\]\)/,
  'tauri cancelGeneration must race the invoke against a timeout',
);
assert.match(
  tauriChatSource,
  /const cancelTimeout = new Promise\(function \(_, reject\) \{[\s\S]*?STEER_INVOKE_TIMEOUT_MS\)/,
  'tauri cancelGeneration must reuse STEER_INVOKE_TIMEOUT_MS (same budget as the interrupt path)',
);

// 2. 预挂 catch 吞掉超时后晚到的 cancel 拒绝（cancel 幂等，晚到无害）。
assert.match(
  tauriCancel,
  /cancelPromise\.catch\(function \(\) \{ \/\* swallow the late rejection after a timeout \*\/ \}\)/,
  'the late cancel rejection must be swallowed, not left unhandled',
);

// 3. finally 里清除定时器：正常路径定时器永不触发，cancelTimeout 不会无人处理地 reject。
assert.match(
  tauriCancel,
  /finally \{\s*clearTimeout\(cancelTimeoutId\);\s*\}/,
  'the timeout timer must be cleared in a finally so it can never fire after a normal settle',
);

// ── Web 桥：同一 wedged-engine 场景需要同样的兜底 ──

const webCancel = extractFunction(webBridgeSource, 'cancelGeneration');
assert.match(
  webCancel,
  /await Promise\.race\(\[cancelPromise, cancelTimeout\]\)/,
  'web cancelGeneration must race the invoke against a timeout too (shared ChatView single-flight flag)',
);
assert.match(
  webCancel,
  /cancelPromise\.catch\(/,
  'web bridge must also swallow the late cancel rejection',
);
assert.match(
  webCancel,
  /finally \{\s*clearTimeout\(cancelTimeoutId\);\s*\}/,
  'web bridge must clear the timeout timer in a finally',
);

console.log('chat_stop_cancel_race: all assertions passed');
