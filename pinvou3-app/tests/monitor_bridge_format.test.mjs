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
const bridges = {
  tauri: read('src', 'platform', 'tauri', 'bridge', 'monitor.js'),
  web: read('src', 'platform', 'web', 'bridge.js'),
};

const TTFT_FALLBACK_PATTERN =
  /vllmTtft: sadj && sadj\.ttft_count > 0\s*\?\s*\(sadj\.ttft_sum_s \/ sadj\.ttft_count\)\.toFixed\(2\) \+ " s" : "—"/;
const TPS_FALLBACK_PATTERN =
  /vllmTps: sadj && sadj\.tps_time_s > 0\s*\?\s*\(sadj\.tps_tokens \/ sadj\.tps_time_s\)\.toFixed\(1\) \+ " tok\/s" : "—"/;

for (const [name, source] of Object.entries(bridges)) {
  assert.match(
    source,
    TTFT_FALLBACK_PATTERN,
    `${name} bridge TTFT 无数据必须回退为 —`,
  );
  assert.match(
    source,
    TPS_FALLBACK_PATTERN,
    `${name} bridge TPS 无数据必须回退为 —`,
  );
  assert.doesNotMatch(
    source,
    /vllmTtft:[\s\S]*?toFixed\(2\) \+ " s" : "0 s"/,
    `${name} bridge TTFT 无数据不得回退为字面 "0 s"`,
  );
  assert.doesNotMatch(
    source,
    /vllmTps:[\s\S]*?toFixed\(1\) \+ " tok\/s" : "0 tok\/s"/,
    `${name} bridge TPS 无数据不得回退为字面 "0 tok/s"`,
  );
}

console.log('monitor bridge 无数据 TTFT/TPS fallback 在 tauri/web 两份 bridge 中一致为 —');
