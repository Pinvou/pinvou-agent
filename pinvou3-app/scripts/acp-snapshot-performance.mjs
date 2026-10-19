import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { mergeAcpTimelineSnapshot, projectAcpTimeline } from '../src/features/codex/acp-state.js';
import { snapshotEvent, snapshotHistory } from '../tests/helpers/acp-snapshot-fixture.mjs';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const arguments_ = process.argv.slice(2);
const options = { '--baseline-ref': 'HEAD', '--samples': '30', '--output': 'tmp/acp-snapshot-performance.json' };
for (let index = 0; index < arguments_.length; index += 2) {
  const key = arguments_[index];
  if (!Object.hasOwn(options, key) || !arguments_[index + 1]) throw new Error(`Invalid argument: ${key}`);
  options[key] = arguments_[index + 1];
}
const sampleCount = Number(options['--samples']);
assert.ok(Number.isInteger(sampleCount) && sampleCount >= 5 && sampleCount <= 200, '--samples must be 5..200');
function git(...args) {
  const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8', maxBuffer: 10 * 1024 * 1024 });
  if (result.status !== 0) throw new Error(result.stderr || 'git failed');
  return result.stdout;
}
const baselineCommit = git('rev-parse', '--verify', '--end-of-options', `${options['--baseline-ref']}^{commit}`).trim();
assert.match(baselineCommit, /^[0-9a-f]{40,64}$/);
const statePath = 'pinvou3-app/src/features/codex/acp-state.js';
const modelPath = 'pinvou3-app/src/features/conversation/conversation-model.js';
const baselineState = git('show', `${baselineCommit}:${statePath}`);
const baselineModel = git('show', `${baselineCommit}:${modelPath}`);
const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const baseline = await import(moduleUrl(baselineState.replace(
  "'../conversation/conversation-model.js'", () => JSON.stringify(moduleUrl(baselineModel)),
)));
const variants = { before: baseline.mergeAcpTimelineSnapshot, after: mergeAcpTimelineSnapshot };
const sha256 = source => createHash('sha256').update(source).digest('hex');
const currentPaths = { state: statePath, model: modelPath, script: 'pinvou3-app/scripts/acp-snapshot-performance.mjs',
  fixture: 'pinvou3-app/tests/helpers/acp-snapshot-fixture.mjs' };
const sourceHashes = { baselineState: sha256(baselineState), baselineModel: sha256(baselineModel),
  ...Object.fromEntries(Object.entries(currentPaths).map(([key, file]) => [key, sha256(readFileSync(path.join(repo, file)))])) };
const percentile = (values, p) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * p) - 1];
const cases = [];
const samples = [];
for (const turns of [200, 1000, 2000]) {
  const history = snapshotHistory(turns);
  const snapshot = structuredClone(history);
  const tail = Array.from({ length: 20 }, (_, index) => snapshotEvent(history.length + index + 1,
    'agent_message_chunk', { content: { type: 'text', text: 'synthetic tail' } }));
  const scenarios = { emptyCurrent: [], overlap: history, overlapWithTail: [...history, ...tail], smallLocalTail: tail };
  for (const [scenario, current] of Object.entries(scenarios)) {
    const expected = baseline.mergeAcpTimelineSnapshot(snapshot, current, 'synthetic');
    const actual = mergeAcpTimelineSnapshot(snapshot, current, 'synthetic');
    assert.deepEqual(projectAcpTimeline(actual), baseline.projectAcpTimeline(expected));
    function run(variant) {
      const started = performance.now();
      const merged = variants[variant](snapshot, current, 'synthetic');
      const ms = performance.now() - started;
      assert.deepEqual(merged, expected);
      for (let index = 0; index < expected.length; index++) assert.equal(merged[index], expected[index]);
      if (expected === snapshot) assert.equal(merged, snapshot);
      return ms;
    }
    for (let warmup = 0; warmup < 5; warmup++) for (const variant of Object.keys(variants)) run(variant);
    for (let round = 0; round < sampleCount; round++) {
      const order = round % 2 ? ['after', 'before'] : ['before', 'after'];
      for (const variant of order) samples.push({ turns, scenario, variant, round, ms: run(variant) });
    }
    const summaries = Object.keys(variants).map(variant => {
      const values = samples.filter(row => row.turns === turns && row.scenario === scenario && row.variant === variant).map(row => row.ms);
      return { turns, events: snapshot.length, scenario, variant, samples: values.length,
        p50Ms: percentile(values, 0.5), p95Ms: percentile(values, 0.95) };
    });
    cases.push(...summaries);
    console.log(JSON.stringify({ completed: { turns, scenario }, cases: summaries }));
  }
}
for (const [key, file] of Object.entries(currentPaths)) {
  assert.equal(sha256(readFileSync(path.join(repo, file))), sourceHashes[key], 'Sources changed during measurement; rerun');
}
const report = { date: new Date().toISOString(), baselineCommit, node: process.version, platform: process.platform,
  cpu: os.cpus()[0]?.model, sourceHashes,
  scope: 'Synthetic local ACP snapshot merge only; excludes setup/assertions, transport, disk, projection, DOM, native device and model time. Five warm-ups, alternating before/after, single-merge samples, nearest-rank P95. Complete values and event references are asserted.',
  cases, samples };
const output = path.resolve(repo, options['--output']);
mkdirSync(path.dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ output, baselineCommit, cases }, null, 2));
