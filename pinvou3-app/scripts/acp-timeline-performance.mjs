import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { appendAcpEvent, projectAcpTimeline } from '../src/features/codex/acp-state.js';
import { createAcpTimelineProjector } from '../src/features/codex/acp-timeline-projector.js';
import { acpEvent, acpHistory } from '../tests/helpers/acp-timeline-fixture.mjs';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const arguments_ = process.argv.slice(2);
const options = { '--baseline-ref': 'HEAD', '--samples': '30', '--output': 'tmp/acp-timeline-performance.json' };
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
const featurePath = 'pinvou3-app/src/features/codex/';
const baselineSource = git('show', `${baselineCommit}:${featurePath}acp-state.js`);
const modelPath = 'pinvou3-app/src/features/conversation/conversation-model.js';
const baselineModel = git('show', `${baselineCommit}:${modelPath}`);
const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(source).toString('base64')}`;
const baseline = await import(moduleUrl(baselineSource.replace(
  "'../conversation/conversation-model.js'", () => JSON.stringify(moduleUrl(baselineModel)),
)));
const factories = { before: () => baseline.projectAcpTimeline, after: createAcpTimelineProjector };
const appenders = { before: baseline.appendAcpEvent, after: appendAcpEvent };
const sha256 = source => createHash('sha256').update(source).digest('hex');
const currentPaths = { currentState: `${featurePath}acp-state.js`, currentModel: modelPath,
  projector: `${featurePath}acp-timeline-projector.js` };
const sourceHashes = { baselineState: sha256(baselineSource), baselineModel: sha256(baselineModel),
  ...Object.fromEntries(Object.entries(currentPaths).map(([key, file]) => [key, sha256(readFileSync(path.join(repo, file)))])) };
const percentile = (values, p) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * p) - 1];
const summarize = values => ({ p50Ms: percentile(values, 0.5), p95Ms: percentile(values, 0.95) });
const updates = 20;
const samples = [];
const cases = [];
for (const turns of [1, 200, 1000, 2000]) {
  const history = acpHistory(turns);
  const deltas = Array.from({ length: updates }, (_, index) => acpEvent(history.length + index + 1,
    'agent_message_chunk', { content: { type: 'text', text: `chunk-${index} **sample**\n` } }));
  const finalEvents = deltas.reduce(appendAcpEvent, history);
  const expected = projectAcpTimeline(finalEvents);
  assert.deepEqual(expected, baseline.projectAcpTimeline(finalEvents));
  function replay(variant) {
    const project = factories[variant]();
    const coldStart = performance.now();
    const initial = project(history);
    const coldMs = performance.now() - coldStart;
    let events = history;
    let projection;
    const started = performance.now();
    for (const incoming of deltas) {
      events = appenders[variant](events, incoming);
      projection = project(events);
    }
    const appendAndProjectMs = (performance.now() - started) / updates;
    assert.deepEqual(projection, expected);
    if (variant === 'after') assert.equal(projection.turns[0], initial.turns[0]);
    return { coldMs, appendAndProjectMs };
  }
  for (let warmup = 0; warmup < 5; warmup++) for (const variant of Object.keys(factories)) replay(variant);
  for (let round = 0; round < sampleCount; round++) {
    const order = round % 2 ? ['after', 'before'] : ['before', 'after'];
    for (const variant of order) samples.push({ turns, variant, round, ...replay(variant) });
  }
  for (const variant of Object.keys(factories)) {
    const rows = samples.filter(row => row.turns === turns && row.variant === variant);
    cases.push({ turns, events: history.length, variant, samples: rows.length,
      cold: summarize(rows.map(row => row.coldMs)),
      appendAndProject: summarize(rows.map(row => row.appendAndProjectMs)) });
  }
}
for (const [key, file] of Object.entries(currentPaths)) {
  assert.equal(sha256(readFileSync(path.join(repo, file))), sourceHashes[key], 'Sources changed during measurement; rerun');
}
const report = {
  date: new Date().toISOString(), baselineCommit, node: process.version, platform: process.platform,
  cpu: os.cpus()[0]?.model,
  scope: 'Synthetic local ACP event append and projection, not transport, disk, React, native device or model time. Five warm-ups; 30 alternating samples by default; each sample averages 20 append/projection updates. Initial hydration measured separately. Final output equals full replay.',
  updatesPerSample: updates,
  sourceHashes,
  cases, samples,
};
const output = path.resolve(repo, options['--output']);
mkdirSync(path.dirname(output), { recursive: true });
writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ output, baselineCommit, cases }, null, 2));
