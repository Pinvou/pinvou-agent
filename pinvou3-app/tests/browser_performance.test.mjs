import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  BROWSER_PERFORMANCE_THRESHOLDS_MS,
  browserPerformanceSnapshot,
  percentile,
  recordBrowserPerformance,
  resetBrowserPerformance,
} from '../src/features/browser/browser-performance.mjs';
import { runVisiblePageOperation } from '../src-tauri/resources/common/bundle/mcp-servers/browser-wrapper-protocol.mjs';

test('browser performance samples are bounded and gate on P95 once enough samples exist', () => {
  resetBrowserPerformance();
  for (let i = 1; i <= 35; i += 1) recordBrowserPerformance('tab_switch_ms', i);
  const snapshot = browserPerformanceSnapshot();
  assert.equal(snapshot.metrics.tab_switch_ms.count, 35);
  assert.equal(snapshot.metrics.tab_switch_ms.p95Ms, 34);
  assert.equal(snapshot.metrics.tab_switch_ms.passes, true);
  for (let i = 0; i < 250; i += 1) recordBrowserPerformance('bounded_metric', i);
  assert.equal(browserPerformanceSnapshot().metrics.bounded_metric.count, 200);
  assert.equal(percentile([], 0.95), null);
});

test('target-page alignment records app attachment latency before execute starts', async () => {
  const pageTokens = new Map([[7, '0123456789abcdef']]);
  const timeline = [];
  let now = 10;
  const result = await runVisiblePageOperation({
    pageId: 7,
    pageTokens,
    now: () => now,
    ensureActive: () => { now += 2; },
    activateTab: () => { now += 10; return { lease: 'ok' }; },
    assertLease: () => { now += 1; },
    selectPage: () => { now += 12; },
    verify: () => { now += 8; },
    recordAlignment: (durationMs) => timeline.push(['metric', durationMs]),
    execute: () => timeline.push(['execute']),
  });
  assert.equal(result.pageId, 7);
  assert.deepEqual(timeline[0], ['metric', 40]);
  assert.deepEqual(timeline[1], ['execute']);
});

test('the JSONL reporter returns a gate-ready exit code from minimum samples and P95', () => {
  const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const input = Array.from({ length: 30 }, (_, index) => (
    `[browser-perf] ${JSON.stringify({ metric: 'agent_target_alignment_ms', durationMs: 50 + index })}`
  )).join('\n');
  const result = spawnSync(
    process.execPath,
    [path.join(projectRoot, 'scripts', 'browser-performance-report.mjs'), '--min-samples=30'],
    { input, encoding: 'utf8' },
  );
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.metrics.agent_target_alignment_ms.count, 30);
  assert.equal(
    report.metrics.agent_target_alignment_ms.thresholdMs,
    BROWSER_PERFORMANCE_THRESHOLDS_MS.agent_target_alignment_ms,
  );
  assert.equal(report.metrics.agent_target_alignment_ms.passes, true);
});
