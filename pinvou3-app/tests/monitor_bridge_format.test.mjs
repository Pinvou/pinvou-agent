import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.join(here, '..');
const read = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

// 两份 bridge(tauri 桌面 / web)的 TTFT/TPS 无数据回退必须统一为「—」,
// 与 MonitorView.jsx 的 MonitorMetricCard(值为 — 时不渲染 unit)配套。
// 防止某一侧单独回退成字面 "0 s"/"0 tok/s" 造成桌面/网页显示漂移。
// Wave 1 moved pollMonitor (vllmTtft/vllmTps formatting) into the shared
// base; both lanes forward to it, so the shared body is the single source to
// pin (the lane files no longer carry the formatted literals) — one read
// covers both lanes.
const sharedBridgeSource = read('src', 'shared', 'bridge-shared-helpers.js');

// pollBackendStatus 仍是两条 lane 各自声明的手抄体(round-9 review R2:仅
// 对齐语义,未单源)。钉住在飞守卫必须同时存在于两侧 —— get_backend_status
// 挂起时,10 秒间隔的重入会在慢响应上叠轮次;守卫缺失曾只发生在桌面侧。
const pollBackendStatusGuard = /backendStatusPollInFlight = true/;
assert.match(
  read('src', 'platform', 'tauri', 'bridge', 'monitor.js'),
  pollBackendStatusGuard,
  'desktop pollBackendStatus must keep the in-flight reentrancy guard',
);
assert.match(
  read('src', 'platform', 'web', 'bridge.js'),
  pollBackendStatusGuard,
  'web pollBackendStatus must keep the in-flight reentrancy guard',
);

const TTFT_FALLBACK_PATTERN =
  /vllmTtft: sadj && sadj\.ttft_count > 0\s*\?\s*\(sadj\.ttft_sum_s \/ sadj\.ttft_count\)\.toFixed\(2\) \+ " s" : "—"/;
const TPS_FALLBACK_PATTERN =
  /vllmTps: sadj && sadj\.tps_time_s > 0\s*\?\s*\(sadj\.tps_tokens \/ sadj\.tps_time_s\)\.toFixed\(1\) \+ " tok\/s" : "—"/;

assert.match(
  sharedBridgeSource,
  TTFT_FALLBACK_PATTERN,
  'bridge TTFT 无数据必须回退为 —',
);
assert.match(
  sharedBridgeSource,
  TPS_FALLBACK_PATTERN,
  'bridge TPS 无数据必须回退为 —',
);
assert.doesNotMatch(
  sharedBridgeSource,
  /vllmTtft:[\s\S]*?toFixed\(2\) \+ " s" : "0 s"/,
  'bridge TTFT 无数据不得回退为字面 "0 s"',
);
assert.doesNotMatch(
  sharedBridgeSource,
  /vllmTps:[\s\S]*?toFixed\(1\) \+ " tok\/s" : "0 tok\/s"/,
  'bridge TPS 无数据不得回退为字面 "0 tok/s"',
);

console.log('monitor bridge 无数据 TTFT/TPS fallback 断言通过(共享 bridge 源单次读取)');
