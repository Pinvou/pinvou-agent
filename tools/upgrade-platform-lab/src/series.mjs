import { readFile, mkdir, readdir, lstat, open } from 'node:fs/promises';
import path from 'node:path';
import { platform, arch } from 'node:os';
import { createRun, childPath, durableFile, publishReport, requireThat, ordinary } from './owned-files.mjs';
import { prototypeIdentity, runExperiment } from './run.mjs';
import { validatePlan, unsupportedReason, summary, encode, sha256, keys } from './series-records.mjs';

const attemptName = index => `attempt-${String(index).padStart(4, '0')}`;
async function identity() {
  const files = ['../series.mjs', 'series.mjs', 'series-records.mjs'];
  return sha256(encode([await prototypeIdentity(), ...await Promise.all(files.map(async name =>
    [name, sha256(await readFile(new URL(name, import.meta.url)))]))]));
}
async function stableSource(expected) { requireThat(await identity() === expected, 'lab_series_source_changed'); }
async function readRecord(filename) {
  // Published reports intentionally have a second hard-link name. Reject other
  // file types and aliases without applying ordinary()'s single-link rule.
  const info = await lstat(filename, { bigint: true });
  requireThat(info.isFile() && !info.isSymbolicLink() && info.size <= 4n * 1024n ** 2n, 'lab_series_record_invalid');
  const file = await open(filename, 'r');
  try {
    const before = await file.stat({ bigint: true });
    const fields = ['dev', 'ino', 'size', 'mtimeNs', 'ctimeNs'];
    requireThat(before.isFile() && fields.every(field => info[field] === before[field]), 'lab_series_record_changed');
    const bytes = Buffer.alloc(Number(before.size));
    let offset = 0;
    while (offset < bytes.length) {
      const result = await file.read(bytes, offset, bytes.length - offset);
      requireThat(result.bytesRead > 0, 'lab_series_record_changed'); offset += result.bytesRead;
    }
    const trailing = await file.read(Buffer.alloc(1), 0, 1);
    const after = await file.stat({ bigint: true });
    requireThat(trailing.bytesRead === 0 && fields.every(field => before[field] === after[field]), 'lab_series_record_changed');
    return { bytes, value: JSON.parse(bytes.toString('utf8')) };
  } finally { await file.close(); }
}
async function readJSON(filename) { return (await readRecord(filename)).value; }
export async function readSeriesPlan(filename) { return validatePlan(await readJSON(filename)); }
async function optionalJSON(filename) {
  try { return { found: true, value: await readJSON(filename) }; }
  catch (error) { if (error.code === 'ENOENT') return { found: false }; throw error; }
}
export async function runSeries(parent, input, options = {}) {
  const plan = validatePlan(input), sourceSHA256 = await identity(), prototypeSHA256 = await prototypeIdentity();
  const host = { os: platform(), arch: arch() }, blockedReason = unsupportedReason(plan, host);
  const total = plan.repetitions * (plan.workload.bytes * 2 + plan.workload.files * 4096);
  requireThat(Number.isSafeInteger(total));
  // All retained attempts must fit at preflight. This observation is not a disk
  // reservation; each existing run performs its own current-space check too.
  const { root, runId } = await createRun(parent, blockedReason ? 0 : total);
  const manifest = { experiment: 'platform-series-v0', seriesId: runId, sourceSHA256, prototypeSHA256, host, plan, blockedReason };
  const manifestSHA256 = sha256(encode(manifest));
  await durableFile(childPath(root, 'series.json'), encode(manifest));
  for (let index = 1; index <= plan.repetitions; index++) {
    if (options.signal?.aborted) break;
    await stableSource(sourceSHA256);
    const directory = childPath(root, attemptName(index));
    await mkdir(directory, { mode: 0o700 });
    const binding = { seriesId: runId, index, manifestSHA256 };
    await durableFile(childPath(directory, 'started.json'), encode({ index, sourceSHA256, manifestSHA256, startedAt: new Date().toISOString() }));
    let outcome;
    if (blockedReason) outcome = { index, manifestSHA256, status: 'unsupported', code: blockedReason };
    else {
      try {
        const result = await runExperiment(directory, plan.workload, { context: plan.context, seriesBinding: binding });
        await stableSource(sourceSHA256);
        requireThat(result.report.prototypeSHA256 === prototypeSHA256, 'lab_series_source_changed');
        const relative = `${path.basename(result.root)}/report.json`;
        const { bytes } = await readRecord(childPath(directory, relative));
        outcome = { index, manifestSHA256, status: 'success', reportPath: relative, reportSHA256: sha256(bytes), sample: result.report };
      } catch (error) {
        // Raw provider, path, account and key diagnostics never become reports.
        outcome = { index, manifestSHA256, status: 'failed', code: error?.code?.startsWith('lab_') ? error.code : 'lab_experiment_failed' };
      }
    }
    await publishReport(directory, outcome);
    // Never discard a failed sample and continue until 30 convenient successes.
    if (outcome.status !== 'success' || options.signal?.aborted) break;
  }
  if (options.signal?.aborted) await durableFile(childPath(root, 'stopped.json'), encode({ reason: 'operator-stop-requested' }));
  await stableSource(sourceSHA256);
  // Re-open the exact persisted artifacts through the same validation path used
  // by later inspection, so a mixed or mutated sample cannot be published.
  const inspected = await inspectSeries(root);
  const report = inspected.report;
  await publishReport(root, report);
  return { root, report };
}

export async function inspectSeries(root) {
  requireThat(path.isAbsolute(root));
  await ordinary(root, true);
  const manifest = await readJSON(childPath(root, 'series.json'));
  keys(manifest, ['experiment', 'seriesId', 'sourceSHA256', 'prototypeSHA256', 'host', 'plan', 'blockedReason']);
  const manifestSHA256 = sha256(encode(manifest));
  requireThat(manifest.experiment === 'platform-series-v0' && /^[a-f0-9]{64}$/.test(manifest.sourceSHA256) &&
    /^[a-f0-9]{64}$/.test(manifest.prototypeSHA256) && /^[a-f0-9-]{36}$/.test(manifest.seriesId));
  const owner = await readJSON(childPath(root, 'owner.json'));
  requireThat(owner.experiment === 'platform-v0' && owner.runId === manifest.seriesId &&
    path.basename(root) === `pinvou-lab-${manifest.seriesId}`);
  requireThat(manifest.blockedReason === null || ['target-host-not-supported', 'production-mode-not-implemented',
    'cache-controller-not-implemented'].includes(manifest.blockedReason));
  const plan = validatePlan(manifest.plan), attempts = [], runIds = new Set();
  // Inspect recorded host facts, not the host on which this report is reopened.
  requireThat(manifest.blockedReason === unsupportedReason(plan, manifest.host), 'lab_series_experiment_mismatch');
  let interrupted = false;
  const names = await readdir(root);
  for (let index = 1; index <= plan.repetitions; index++) {
    const directory = childPath(root, attemptName(index));
    if (!names.includes(attemptName(index))) break;
    await ordinary(directory, true);
    const startRecord = await optionalJSON(childPath(directory, 'started.json'));
    if (!startRecord.found) { attempts.push({ index, status: 'aborted', code: 'start-not-published' }); interrupted = true; break; }
    const started = startRecord.value;
    keys(started, ['index', 'sourceSHA256', 'manifestSHA256', 'startedAt']);
    requireThat(started.index === index && started.sourceSHA256 === manifest.sourceSHA256 &&
      started.manifestSHA256 === manifestSHA256, 'lab_series_plan_changed');
    const outcomeRecord = await optionalJSON(childPath(directory, 'report.json'));
    if (!outcomeRecord.found) { attempts.push({ index, status: 'aborted', code: 'completion-not-published' }); interrupted = true; break; }
    const attempt = outcomeRecord.value;
    requireThat(attempt && typeof attempt === 'object' && !Array.isArray(attempt), 'lab_series_shape_invalid');
    requireThat(attempt.index === index && attempt.manifestSHA256 === manifestSHA256 &&
      ['success', 'failed', 'unsupported'].includes(attempt.status), 'lab_series_plan_changed');
    if (attempt.status === 'success') {
      keys(attempt, ['index', 'manifestSHA256', 'status', 'reportPath', 'reportSHA256', 'sample']);
      requireThat(/^pinvou-lab-[a-f0-9-]{36}\/report\.json$/.test(attempt.reportPath));
      const reportFile = childPath(directory, attempt.reportPath);
      await ordinary(path.dirname(reportFile), true);
      const { bytes: raw, value: sample } = await readRecord(reportFile);
      requireThat(sha256(raw) === attempt.reportSHA256 && encode(sample) === encode(attempt.sample), 'lab_series_sample_changed');
      requireThat(sample.prototypeSHA256 === manifest.prototypeSHA256 && sample.experiment === 'platform-v0' &&
        sample.evidence?.qualification === false && encode(sample.workload) === encode(plan.workload) &&
        !runIds.has(sample.runId) && path.basename(path.dirname(reportFile)) === `pinvou-lab-${sample.runId}` &&
        manifest.blockedReason === null, 'lab_series_sample_mismatch');
      keys(sample.series, ['seriesId', 'index', 'manifestSHA256']);
      requireThat(sample.series.seriesId === manifest.seriesId && sample.series.index === index &&
        sample.series.manifestSHA256 === manifestSHA256, 'lab_series_plan_changed');
      requireThat(sample.environment?.os === manifest.host.os && sample.environment?.arch === manifest.host.arch,
        'lab_series_experiment_mismatch');
      runIds.add(sample.runId);
      requireThat(encode(sample.context) === encode({ ...plan.context, observedDevice: sample.context.observedDevice,
        observedFilesystemType: sample.context.observedFilesystemType }));
      if (attempts.length) requireThat(encode(sample.context) === encode(attempts[0].sample.context) &&
        encode(sample.environment) === encode(attempts[0].sample.environment), 'lab_series_context_mismatch');
      requireThat([sample.freeze?.milliseconds, sample.backup?.milliseconds].every(value => Number.isFinite(value) && value >= 0));
    } else {
      keys(attempt, ['index', 'manifestSHA256', 'status', 'code']);
      requireThat(typeof attempt.code === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(attempt.code));
      requireThat(attempt.status === 'unsupported' ? attempt.code === manifest.blockedReason : manifest.blockedReason === null);
    }
    attempts.push(attempt);
    if (attempt.status !== 'success') break;
  }
  const expectedDirectories = new Set(attempts.map(item => attemptName(item.index)));
  requireThat(names.filter(name => name.startsWith('attempt-')).every(name => expectedDirectories.has(name)), 'lab_series_attempt_gap');
  const stopped = await optionalJSON(childPath(root, 'stopped.json'));
  if (stopped.found) { keys(stopped.value, ['reason']); requireThat(stopped.value.reason === 'operator-stop-requested'); }
  const report = { ...manifest, ...summary(plan, attempts, manifest.blockedReason), stopRequested: stopped.found, attempts };
  const published = await optionalJSON(childPath(root, 'report.json'));
  if (published.found) {
    keys(published.value, Object.keys(report));
    requireThat(!interrupted && encode(published.value) === encode(report), 'lab_series_final_mismatch');
  }
  return { published: published.found, report };
}
