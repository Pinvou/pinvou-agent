import { randomBytes, createHash } from 'node:crypto';
import { platform, release, arch, cpus, totalmem } from 'node:os';
import { readFile, statfs, lstat } from 'node:fs/promises';
import { createRun, childPath, publishReport, directoryDurability, requireThat } from './owned-files.mjs';
import path from 'node:path';
import { startWriters, freezeWriters, joinWriters, writerNames } from './coordination.mjs';
import { generate } from './workload.mjs';
import { inventory, seal, verify } from './archive.mjs';

async function prototypeIdentity() {
  const hash = createHash('sha256');
  for (const name of ['../lab.mjs', 'owned-files.mjs', 'writer.mjs', 'coordination.mjs', 'archive.mjs', 'workload.mjs', 'run.mjs']) {
    const bytes = await readFile(new URL(name, import.meta.url));
    hash.update(name); hash.update('\0'); hash.update(bytes); hash.update('\0');
  }
  return hash.digest('hex');
}
export async function runExperiment(parent, plan, options = {}) {
  const prototypeSHA256 = await prototypeIdentity();
  const { root, runId } = await createRun(parent, plan.bytes * 2 + plan.files * 4096);
  const key = randomBytes(32);
  let owners;
  try {
    const tags = options.context ?? { labHostId: `unidentified-${runId}`, storageId: `unidentified-${runId}`, storageClass: 'unknown' };
    requireThat(tags && Object.keys(tags).sort().join(',') === 'labHostId,storageClass,storageId' &&
      Object.values(tags).every(value => typeof value === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(value)), 'lab_context_invalid');
    const device = await lstat(root, { bigint: true }), filesystem = await statfs(root, { bigint: true });
    const context = { labHostId: tags.labHostId, storageId: tags.storageId, storageClass: tags.storageClass,
      observedDevice: String(device.dev), observedFilesystemType: String(filesystem.type) };
    owners = await startWriters(root, options.unsavable === true, options.spawn);
    await generate(root, plan);
    const freeze = await freezeWriters(root, owners);
    const start = process.hrtime.bigint();
    const entries = await inventory(root, writerNames);
    const backup = childPath(root, 'backup.lab');
    const manifest = await seal(root, entries, backup, key);
    const verified = await verify(backup, manifest, key);
    const milliseconds = Number(process.hrtime.bigint() - start) / 1e6;
    const durability = await directoryDurability(root);
    const report = {
      experiment: 'platform-v0', prototypeSHA256, runId, context, observedAt: new Date().toISOString(),
      environment: { os: platform(), release: release(), arch: arch(), node: process.version, cpu: cpus()[0]?.model, logicalCpus: cpus().length, memoryBytes: totalmem() },
      workload: plan,
      freeze: { milliseconds: freeze.milliseconds, syntheticWriters: freeze.evidence.length },
      backup: { milliseconds, ...verified, manifestSHA256: createHash('sha256').update(JSON.stringify(manifest)).digest('hex') },
      evidence: {
        syntheticSQLiteAndPayloadOnly: true, fileFsyncObserved: true, directoryDurability: durability,
        sameBootMonotonicMeasurementOnly: true, powerOffTime: 'unproven', machineWideWriterAdmission: 'not-implemented',
        distinctOSUsersAndServices: 'not-exercised', protectedKeyCustody: 'not-exercised',
        parentCrashKeyRecovery: 'unsupported-ephemeral-key', productionBackupFormat: false, qualification: false,
      },
    };
    // Publication is after observed close of every child, never merely IPC ACK.
    await joinWriters(owners); owners = undefined;
    await publishReport(root, report, options.reportOpen);
    return { root, report };
  } finally {
    key.fill(0);
    if (owners) await joinWriters(owners);
  }
}
export async function summarize(files) {
  requireThat(files.length > 0 && files.length <= 1000);
  const reports = [];
  for (const filename of files) {
    requireThat(path.basename(filename) === 'report.json', 'lab_report_not_published');
    reports.push(JSON.parse(await readFile(filename, 'utf8')));
  }
  // Repeated copies of one result are never additional measurement samples.
  requireThat(new Set(reports.map(report => report.runId)).size === reports.length);
  for (const report of reports) requireThat(report.experiment === 'platform-v0' && report.evidence?.qualification === false &&
    JSON.stringify(report.workload) === JSON.stringify(reports[0].workload) &&
    JSON.stringify(report.environment) === JSON.stringify(reports[0].environment) &&
    JSON.stringify(report.context) === JSON.stringify(reports[0].context) &&
    /^[a-f0-9]{64}$/.test(report.prototypeSHA256) && report.prototypeSHA256 === reports[0].prototypeSHA256 &&
    [report.freeze?.milliseconds, report.backup?.milliseconds].every(value => Number.isFinite(value) && value >= 0));
  const distribution = values => {
    values.sort((a, b) => a - b);
    return { samples: values.length, medianMilliseconds: values[Math.floor((values.length - 1) / 2)], p95Milliseconds: values[Math.ceil(values.length * .95) - 1], maximumMilliseconds: values.at(-1) };
  };
  return { experiment: 'platform-v0', prototypeSHA256: reports[0].prototypeSHA256, context: reports[0].context, workload: reports[0].workload, environment: reports[0].environment,
    freeze: distribution(reports.map(report => report.freeze.milliseconds)),
    backup: distribution(reports.map(report => report.backup.milliseconds)),
    certifiedBudget: null, cacheState: 'not-controlled', qualification: false };
}
