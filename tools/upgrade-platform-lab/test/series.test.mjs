import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, readdir, writeFile, mkdir, open, rename, unlink } from 'node:fs/promises';
import { tmpdir, platform, arch } from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { runSeries, inspectSeries } from '../src/series.mjs';
import { validatePlan, distribution } from '../src/series-records.mjs';

const target = ({ 'win32-x64': 'windows-x86_64-nsis-perMachine', 'linux-x64': 'linux-x86_64-deb-perMachine',
  'linux-arm64': 'linux-arm64-deb-perMachine', 'darwin-arm64': 'macos-universal-dmg-perMachine',
  'darwin-x64': 'macos-universal-dmg-perMachine' })[`${platform()}-${arch()}`];
const plan = () => ({ repetitions: 2, workload: { bytes: 16384, files: 6 }, shape: 'custom-smoke',
  context: { labHostId: 'series-test', storageId: 'test-volume', storageClass: 'unknown' },
  experiment: { targetKey: target, hostKind: 'developer', cacheState: 'uncontrolled', mode: 'synthetic-freeze-backup-v0' } });
const scratch = () => mkdtemp(path.join(tmpdir(), 'pinvou-lab-series-test-'));
const waitFor = async action => {
  for (let index = 0; index < 200; index++) {
    const result = await action(); if (result) return result;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  throw new Error('test observation deadline');
};
async function activeRoot(parent) {
  return await waitFor(async () => {
    const [series] = await readdir(parent); if (!series) return;
    const attempt = path.join(parent, series, 'attempt-0001');
    try { const run = (await readdir(attempt)).find(name => name.startsWith('pinvou-lab-')); if (run) return path.join(attempt, run); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  });
}
test('actual repeated freeze/encrypt/verify produces distinct raw samples and review-only statistics', { skip: !target }, async () => {
  const parent = await scratch(), input = plan();
  const result = await runSeries(parent, input);
  assert.equal(result.report.status, 'complete');
  assert.deepEqual(result.report.counts, { successful: 2, failed: 0, unsupported: 0, aborted: 0, unattempted: 0 });
  assert.equal(result.report.statistics.freeze.samples, 2);
  assert.equal(result.report.proposal.approvedBudget, null);
  assert.equal(result.report.proposal.qualification, false);
  assert.equal(result.report.sampleRequirementMet, false);
  assert.equal(new Set(result.report.attempts.map(item => item.sample.runId)).size, 2);
  assert.deepEqual((await inspectSeries(result.root)).report, result.report);
  const second = result.report.attempts[1];
  const file = path.join(result.root, 'attempt-0002', second.reportPath);
  const original = await readFile(file);
  await writeFile(file, Buffer.concat([original, Buffer.from(' ')]));
  await assert.rejects(inspectSeries(result.root), /lab_series_sample_changed/);
  await writeFile(file, original);
  assert.equal((await inspectSeries(result.root)).published, true);
  const outcomePath = path.join(result.root, 'attempt-0002', 'report.json');
  const originalOutcome = await readFile(outcomePath);
  for (const change of [sample => sample.context.observedDevice += '-other', sample => sample.environment.node += '-other']) {
    const mutated = structuredClone(second); change(mutated.sample);
    const bytes = Buffer.from(JSON.stringify(mutated.sample));
    mutated.reportSHA256 = createHash('sha256').update(bytes).digest('hex');
    await writeFile(file, bytes); await writeFile(outcomePath, JSON.stringify(mutated));
    await assert.rejects(inspectSeries(result.root), /lab_series_context_mismatch/);
  }
  await writeFile(file, original); await writeFile(outcomePath, originalOutcome);
  // A second run owns a new directory and cannot replace the first final report.
  const single = plan(); single.repetitions = 1;
  const other = await runSeries(parent, single);
  assert.notEqual(other.root, result.root);
  assert.deepEqual((await inspectSeries(result.root)).report, result.report);
});
test('unsupported target/cache/mode cannot silently execute a substitute', async () => {
  for (const [field, value] of [['targetKey', 'unknown-target'], ['targetKey', 'toString'], ['cacheState', 'cold'], ['mode', 'activation']]) {
    const parent = await scratch(), input = plan(); input.experiment[field] = value;
    if (!target && field !== 'targetKey') input.experiment.targetKey = 'unknown-target';
    const result = await runSeries(parent, input);
    assert.equal(result.report.status, 'unsupported');
    assert.equal(result.report.counts.unsupported, 1);
    assert.equal(result.report.counts.unattempted, 1);
    assert.equal(result.report.statistics, null);
    assert.equal((await readdir(path.join(result.root, 'attempt-0001'))).some(name => name.startsWith('pinvou-lab-')), false);
    assert.equal((await inspectSeries(result.root)).published, true);
  }
});
test('active stop waits for real writer closure and preserves successful attempt without admitting another', { skip: !target }, async () => {
  const parent = await scratch(), controller = new AbortController(), input = plan();
  input.workload = { bytes: 1024 ** 2, files: 120 };
  const pending = runSeries(parent, input, { signal: controller.signal });
  await activeRoot(parent); controller.abort();
  const result = await pending;
  assert.equal(result.report.stopRequested, true);
  assert.equal(result.report.counts.successful, 1);
  assert.equal(result.report.counts.unattempted, 1);
  assert.equal(result.report.statistics, null);
  assert.equal((await inspectSeries(result.root)).published, true);
});
test('real exclusive archive collision records failure, retains evidence and refuses replacement samples', { skip: !target }, async () => {
  const parent = await scratch(), input = plan(); input.workload = { bytes: 1024 ** 2, files: 120 };
  const pending = runSeries(parent, input), root = await activeRoot(parent);
  const file = await open(path.join(root, 'backup.lab'), 'wx');
  await file.writeFile('intentional-owned-test-collision'); await file.close();
  const result = await pending;
  assert.equal(result.report.status, 'failed');
  assert.equal(result.report.counts.failed, 1);
  assert.equal(result.report.counts.unattempted, 1);
  assert.equal(result.report.statistics, null);
  assert.equal(await readFile(path.join(root, 'backup.lab'), 'utf8'), 'intentional-owned-test-collision');
  await assert.rejects(readFile(path.join(root, 'report.json')));
  assert.equal((await inspectSeries(result.root)).published, true);
});
test('interrupted directory has explicit aborted/unattempted accounting and no complete statistics', { skip: !target }, async () => {
  const controller = new AbortController(); controller.abort();
  const result = await runSeries(await scratch(), plan(), { signal: controller.signal });
  // Simulate an interrupted next attempt using exclusively owned test artifacts.
  await mkdir(path.join(result.root, 'attempt-0001'));
  const final = await readFile(path.join(result.root, 'report.json'));
  await assert.rejects(inspectSeries(result.root), /lab_series_final_mismatch/);
  // Keep the published report immutable: inspect a separate owned incomplete copy
  // with only its manifest/owner and a start directory, never mutate final slots.
  const { createRun, durableFile } = await import('../src/owned-files.mjs');
  const incomplete = await createRun(await scratch(), 0);
  const manifest = JSON.parse(await readFile(path.join(result.root, 'series.json'), 'utf8'));
  manifest.seriesId = incomplete.runId;
  await durableFile(path.join(incomplete.root, 'series.json'), JSON.stringify(manifest));
  await mkdir(path.join(incomplete.root, 'attempt-0001'));
  const inspected = await inspectSeries(incomplete.root);
  assert.equal(inspected.published, false);
  assert.equal(inspected.report.counts.aborted, 1);
  assert.equal(inspected.report.counts.unattempted, 1);
  assert.equal(inspected.report.statistics, null);
  assert.deepEqual(await readFile(path.join(result.root, 'report.json')), final);
});
test('plan isolation, shape bounds and nearest-rank statistics do not infer budgets', () => {
  const input = plan(), detached = validatePlan(input);
  input.context.labHostId = 'changed'; assert.equal(detached.context.labHostId, 'series-test');
  for (const mutate of [item => item.repetitions = 0, item => item.repetitions = 1001,
    item => item.workload.files = 0, item => item.shape = 'many-small', item => item.context.extra = 'x',
    item => item.experiment.cacheState = 'invented', item => item.extra = true]) {
    const invalid = plan(); mutate(invalid); assert.throws(() => validatePlan(invalid));
  }
  assert.deepEqual(distribution([4, 1, 3, 2]), { samples: 4, medianMilliseconds: 2, p95Milliseconds: 4, maximumMilliseconds: 4 });
});

test('present falsey JSON records are invalid, never missing or successful publication', { skip: !target }, async () => {
  const input = plan(); input.repetitions = 1;
  const result = await runSeries(await scratch(), input);
  for (const relative of ['report.json', 'attempt-0001/started.json', 'attempt-0001/report.json']) {
    const file = path.join(result.root, relative), original = await readFile(file);
    for (const value of [false, 0, null, '']) {
      await writeFile(file, JSON.stringify(value));
      await assert.rejects(inspectSeries(result.root), /lab_series_shape_invalid/);
      await writeFile(file, original);
      assert.equal((await inspectSeries(result.root)).published, true);
    }
  }
  const stopped = path.join(result.root, 'stopped.json');
  for (const value of [false, 0, null, '']) {
    await writeFile(stopped, JSON.stringify(value), { flag: 'wx' });
    try { await assert.rejects(inspectSeries(result.root), /lab_series_shape_invalid/); }
    finally { await unlink(stopped); }
  }
  assert.deepEqual((await inspectSeries(result.root)).report, result.report);
});

test('unpublished samples bind the original full plan and recorded experiment semantics', { skip: !target }, async () => {
  const input = plan(), controller = new AbortController();
  input.workload = { bytes: 1024 ** 2, files: 120 };
  const parent = await scratch(), pending = runSeries(parent, input, { signal: controller.signal });
  await activeRoot(parent); controller.abort();
  const result = await pending;
  assert.equal(result.report.counts.successful, 1);
  assert.equal(result.report.counts.unattempted, 1);
  // Preserve all actual raw files. Removing only the final name models the
  // supported crash window after an attempt completed but before publication.
  const final = path.join(result.root, 'report.json'), held = path.join(result.root, 'held-final.json');
  await rename(final, held);
  const manifestFile = path.join(result.root, 'series.json'), originalManifest = await readFile(manifestFile);
  const startedFile = path.join(result.root, 'attempt-0001', 'started.json');
  const outcomeFile = path.join(result.root, 'attempt-0001', 'report.json');
  const sampleFile = path.join(result.root, 'attempt-0001', result.report.attempts[0].reportPath);
  const originals = await Promise.all([startedFile, outcomeFile, sampleFile].map(file => readFile(file)));
  for (const mutate of [value => value.plan.repetitions = 1,
    value => value.plan.experiment.mode = 'activation', value => value.plan.experiment.cacheState = 'cold',
    value => value.plan.experiment.targetKey = 'unknown-target', value => value.plan.context.storageId = 'another-volume']) {
    const manifest = JSON.parse(originalManifest); mutate(manifest);
    await writeFile(manifestFile, JSON.stringify(manifest));
    await assert.rejects(inspectSeries(result.root), /lab_series_(plan_changed|experiment_mismatch)/);
    for (const [index, file] of [startedFile, outcomeFile, sampleFile].entries()) {
      assert.deepEqual(await readFile(file), originals[index]);
    }
  }
  // Even coherently relabelled bindings cannot contradict the actual native
  // environment retained in a successful raw sample.
  const manifest = JSON.parse(originalManifest);
  manifest.host = manifest.host.os === 'win32' ? { os: 'linux', arch: 'x64' } : { os: 'win32', arch: 'x64' };
  manifest.plan.experiment.targetKey = manifest.host.os === 'linux' ? 'linux-x86_64-deb-perMachine' : 'windows-x86_64-nsis-perMachine';
  const manifestBytes = JSON.stringify(manifest), digest = createHash('sha256').update(manifestBytes).digest('hex');
  const started = JSON.parse(originals[0]), outcome = JSON.parse(originals[1]), sample = JSON.parse(originals[2]);
  started.manifestSHA256 = digest; outcome.manifestSHA256 = digest; sample.series.manifestSHA256 = digest;
  const sampleBytes = JSON.stringify(sample);
  outcome.sample = sample; outcome.reportSHA256 = createHash('sha256').update(sampleBytes).digest('hex');
  await writeFile(manifestFile, manifestBytes); await writeFile(startedFile, JSON.stringify(started));
  await writeFile(outcomeFile, JSON.stringify(outcome)); await writeFile(sampleFile, sampleBytes);
  await assert.rejects(inspectSeries(result.root), /lab_series_experiment_mismatch/);
  await writeFile(manifestFile, originalManifest);
  for (const [index, file] of [startedFile, outcomeFile, sampleFile].entries()) await writeFile(file, originals[index]);
  const inspected = await inspectSeries(result.root);
  assert.equal(inspected.published, false);
  assert.equal(inspected.report.counts.successful, 1);
  assert.equal(inspected.report.counts.unattempted, 1);
  assert.equal(inspected.report.statistics, null);
  await rename(held, final);
  assert.deepEqual((await inspectSeries(result.root)).report, result.report);
});
test('actual CLI runs and closes before emitting a complete series; diagnostics redact provider paths', { skip: !target }, async () => {
  const parent = await scratch(), input = plan(); input.repetitions = 1;
  const file = path.join(parent, 'plan.json'); await writeFile(file, JSON.stringify(input));
  const cli = fileURLToPath(new URL('../series.mjs', import.meta.url));
  async function command(args) {
    const child = spawn(process.execPath, [cli, ...args], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '', stderr = ''; child.stdout.on('data', data => stdout += data); child.stderr.on('data', data => stderr += data);
    const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', resolve); });
    return { code, stdout, stderr };
  }
  const run = await command(['run', parent, file]); assert.equal(run.code, 0, run.stderr);
  const result = JSON.parse(run.stdout); assert.equal(result.report.counts.successful, 1);
  const inspect = await command(['inspect', result.root]); assert.equal(inspect.code, 0, inspect.stderr);
  assert.equal(JSON.parse(inspect.stdout).published, true);
  const failed = await command(['run', parent, path.join(parent, 'private-missing-input')]);
  assert.equal(failed.code, 1); assert.equal(failed.stdout, '');
  assert.deepEqual(JSON.parse(failed.stderr), { error: 'lab_series_failed' });
});
