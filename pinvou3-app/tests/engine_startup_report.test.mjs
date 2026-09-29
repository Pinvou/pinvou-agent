import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  collectEngineStartupSamples,
  formatEngineStartupReport,
  parseArgs,
  summarizeEngineStartup,
} from '../scripts/engine-startup-report.mjs';

function turnEvents(turn, kind, total, start = 1_000) {
  return [
    { event: 'user_start', turn_id: turn, timestamp: start },
    {
      event: 'assistant_done',
      turn_id: turn,
      timestamp: start + total + 100,
      engine_startup: {
        engine_ready_timestamp: start + total + 5,
        first_output_timestamp: start + total + 45,
        engine_acquire: {
          kind,
          total_ms: total,
          runtime_lock_ms: 1,
          prepare_model_ms: 2,
          spawn_engine_ms: kind === 'reused' ? 0 : total - 3,
        },
      },
    },
  ].map(event => JSON.stringify(event)).join('\n');
}

test('collects fixed startup phases without exposing turn identifiers', async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), 'pinvou-engine-report-'));
  try {
    const session = path.join(root, 'private-session-id');
    await mkdir(session);
    await writeFile(path.join(session, 'timing_events.jsonl'), [
      turnEvents('secret-turn', 'cold', 420),
      '{broken json',
      turnEvents('warm-turn', 'reused', 20, 2_000),
    ].join('\n'));

    const samples = await collectEngineStartupSamples(root);
    assert.deepEqual(samples.map(sample => sample.kind), ['cold', 'reused']);
    assert.equal(samples[0].send_to_ready_ms, 425);
    assert.equal(samples[0].ready_to_first_output_ms, 40);
    assert.equal(JSON.stringify(samples).includes('secret-turn'), false);
    assert.equal(JSON.stringify(samples).includes('private-session-id'), false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test('requires enough cold and reused samples before confirming an issue', () => {
  const sample = (kind, total_ms) => ({
    kind,
    total_ms,
    send_to_ready_ms: total_ms,
    ready_to_first_output_ms: 50,
    send_to_first_output_ms: total_ms + 50,
    stages: {},
  });
  const insufficient = summarizeEngineStartup([
    sample('cold', 500),
    sample('reused', 20),
  ], { minSamples: 2 });
  assert.equal(insufficient.verdict, 'insufficient_samples');

  const confirmed = summarizeEngineStartup([
    sample('cold', 350),
    sample('cold', 500),
    sample('reused', 10),
    sample('reused', 20),
  ], { minSamples: 2, coldP95ThresholdMs: 300 });
  assert.equal(confirmed.verdict, 'cold_start_issue_confirmed');
  assert.equal(confirmed.groups.cold.acquisition.p50_ms, 350);
  assert.equal(confirmed.groups.cold.acquisition.p95_ms, 500);
  assert.match(formatEngineStartupReport(confirmed), /Issue confirmed/u);

  const weakCeiling = summarizeEngineStartup([
    sample('cold', 400),
    sample('cold', 500),
    sample('reused', 390),
    sample('reused', 410),
  ], { minSamples: 2, coldP95ThresholdMs: 300, minPotentialImprovementPercent: 20 });
  assert.equal(weakCeiling.verdict, 'prewarm_ceiling_below_target');
  assert.match(formatEngineStartupReport(weakCeiling), /Prewarming not recommended/u);
});

test('rejects missing and non-finite numeric CLI arguments', () => {
  assert.throws(() => parseArgs(['--cold-p95-ms', 'NaN']), /requires a number/u);
  assert.throws(() => parseArgs(['--cold-p95-ms']), /requires a number/u);
  assert.throws(() => parseArgs(['--min-samples', '1.5']), /requires an integer/u);
  assert.throws(() => parseArgs(['--min-samples', '0']), /not less than 1/u);
  assert.throws(() => parseArgs(['--min-improvement-percent', '101']), /0 to 100/u);
  assert.throws(() => parseArgs(['--input', '--json']), /requires a path/u);
  assert.equal(parseArgs(['--min-samples', '42']).minSamples, 42);
});

test('computes max without spreading an unbounded sample array', () => {
  const samples = Array.from({ length: 150_000 }, (_, index) => ({
    kind: 'cold',
    total_ms: index,
    send_to_ready_ms: index,
    ready_to_first_output_ms: 1,
    send_to_first_output_ms: index + 1,
    stages: {},
  }));
  const report = summarizeEngineStartup(samples);
  assert.equal(report.groups.cold.acquisition.max_ms, 149_999);
});
