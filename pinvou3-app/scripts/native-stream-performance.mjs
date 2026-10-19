import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { marked } from 'marked';
import { createNativeStreamHarness, nativeStreamSources } from '../tests/helpers/native-stream-harness.mjs';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const arguments_ = process.argv.slice(2);
const options = { '--baseline-ref': 'fcb700f853a32b2900af73451814589af8e9fe79', '--samples': '30', '--output': 'tmp/native-stream-performance.json' };
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
const before = {
  events: git('show', `${baselineCommit}:pinvou3-app/src/platform/tauri/bridge/chat-events.js`),
  host: git('show', `${baselineCommit}:pinvou3-app/src/platform/tauri/bridge.js`),
};
const variants = { before, after: nativeStreamSources };
const chunk = 'Synthetic stream **bold** and `code`.\n';
const chunks = 200;

function replay(variant, count, staggerMs = 0) {
  let parserMs = 0;
  const harness = createNativeStreamHarness({ count, ...variants[variant], recordSnapshots: false,
    render(text) {
      const started = performance.now();
      const html = marked.parse(text);
      parserMs += performance.now() - started;
      return html;
    },
  });
  const started = performance.now();
  const cpuStarted = process.cpuUsage();
  for (let index = 0; index < chunks; index++) {
    for (let session = 0; session < count; session++) {
      if (staggerMs) harness.advance(index * 20 + session * staggerMs);
      harness.emit(`s${session}`, 'chat:delta', { text: chunk });
    }
    harness.advance((index + 1) * 20);
  }
  harness.advance(harness.now + 1100);
  const wallMs = performance.now() - started;
  const cpu = process.cpuUsage(cpuStarted);
  for (let index = 0; index < count; index++) {
    const item = harness.view(`s${index}`).chatItems[0];
    assert.equal(item.text, chunk.repeat(chunks));
    assert.equal(item.html, marked.parse(chunk.repeat(chunks)));
  }
  assert.equal(harness.errors.length, 0);
  return { wallMs, parserMs, cpuMs: (cpu.user + cpu.system) / 1000,
    parses: harness.renders.length, notifications: harness.notificationCount };
}

function activation(variant) {
  const harness = createNativeStreamHarness({ ...variants[variant] });
  harness.emit('s1', 'chat:delta', { text: 'first' });
  harness.advance(40);
  harness.emit('s1', 'chat:delta', { text: ' latest' });
  harness.advance(50);
  harness.activate('s1');
  const expected = marked.parse('first latest');
  const firstSnapshotCurrent = harness.snapshots.at(-1).html === expected;
  const activatedAt = harness.now;
  while (harness.state.chatItems[0].html !== expected && harness.now - activatedAt < 1100) {
    harness.advance(harness.now + 1);
  }
  const htmlCatchupMs = harness.now - activatedAt;
  if (variant === 'after') assert.equal(firstSnapshotCurrent, true, 'activation must publish current HTML');
  return { firstSnapshotCurrent, htmlCatchupMs };
}

const samples = [];
for (const sessions of [1, 5, 10]) {
  for (let warmup = 0; warmup < 5; warmup++) for (const variant of Object.keys(variants)) replay(variant, sessions);
  for (let round = 0; round < sampleCount; round++) {
    const order = round % 2 ? ['after', 'before'] : ['before', 'after'];
    for (const variant of order) samples.push({ sessions, variant, round, ...replay(variant, sessions) });
  }
}
const percentile = (values, p) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * p) - 1];
const cases = [];
for (const sessions of [1, 5, 10]) for (const variant of Object.keys(variants)) {
  const rows = samples.filter(row => row.sessions === sessions && row.variant === variant);
  const summary = key => ({ p50Ms: percentile(rows.map(row => row[key]), 0.5), p95Ms: percentile(rows.map(row => row[key]), 0.95) });
  cases.push({ sessions, variant, samples: rows.length, parses: rows[0].parses, notifications: rows[0].notifications,
    wall: summary('wallMs'), parser: summary('parserMs'), cpu: summary('cpuMs') });
}
const sha256 = value => createHash('sha256').update(value).digest('hex');
const report = {
  date: new Date().toISOString(), baselineCommit, node: process.version, platform: process.platform,
  cpu: os.cpus()[0]?.model,
  scope: 'Local handler/marked/host-notify VM replay, not Tauri/DOM/model/task time. Real shared router and host notify; isolated buffers, storage and lightweight subscribers. Timings exclude setup and virtual waiting, include trailing rendering. Five warm-ups, alternating before/after, nearest-rank P95.',
  sourceHashes: Object.fromEntries(Object.entries(variants).map(([variant, source]) => [variant, {
    events: sha256(source.events), host: sha256(source.host),
  }])),
  workload: { chunksPerSession: chunks, intervalMs: 20, streamVirtualMs: 4000, chunkBytes: Buffer.byteLength(chunk) },
  activation: Object.fromEntries(Object.keys(variants).map(variant => [variant, activation(variant)])),
  cases, samples,
  staggered: [1, 5, 10].flatMap(sessions => Object.keys(variants).map(variant => ({ sessions, variant, ...replay(variant, sessions, 1) }))),
};
const output = path.resolve(repo, options['--output']);
mkdirSync(path.dirname(output), { recursive: true });
writeFileSync(output, JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify({ output, baselineCommit, activation: report.activation, cases }, null, 2));
