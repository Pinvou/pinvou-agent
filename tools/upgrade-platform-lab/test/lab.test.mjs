import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, open, link, appendFile, utimes, readdir } from 'node:fs/promises';
import { fork } from 'node:child_process';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { runExperiment, summarize } from '../src/run.mjs';
import { workload } from '../src/workload.mjs';
import { createRun, childPath } from '../src/owned-files.mjs';
import { seal, verify, inventory } from '../src/archive.mjs';
import { startWriters, freezeWriters, joinWriters } from '../src/coordination.mjs';

// Tests keep their owned synthetic workspace for diagnosis; no recursive deletion.
async function scratch() { return await mkdtemp(path.join(tmpdir(), 'pinvou-platform-lab-test-')); }
test('actual children freeze SQLite/WAL, authenticate full encrypted bytes and join', async () => {
  const parent = await scratch();
  const result = await runExperiment(parent, { bytes: 1024 ** 2, files: 24 });
  assert.equal(result.report.freeze.syntheticWriters, 3);
  assert.ok(result.report.backup.bytes > 1024 ** 2);
  assert.equal(result.report.backup.files, 31); // payload + two files per writer + global freeze
  assert.equal(result.report.evidence.qualification, false);
  assert.equal(result.report.evidence.powerOffTime, 'unproven');
  assert.deepEqual(JSON.parse(await readFile(path.join(result.root, 'report.json'), 'utf8')), result.report);
  assert.equal((await summarize([path.join(result.root, 'report.json')])).certifiedBudget, null);
  await assert.rejects(summarize([path.join(result.root, 'report.json'), path.join(result.root, 'report.json')]));
});
test('unsavable real writer refuses freeze; no backup or successful report', async () => {
  const parent = await scratch();
  await assert.rejects(runExperiment(parent, { bytes: 4096, files: 6 }, { unsavable: true }), /lab_freeze_incomplete/);
  const { readdir } = await import('node:fs/promises');
  const [run] = await readdir(parent);
  assert.ok(run.startsWith('pinvou-lab-'));
  await assert.rejects(readFile(path.join(parent, run, 'backup.lab')));
  await assert.rejects(readFile(path.join(parent, run, 'report.json')));
});
test('frozen owner cannot restart its preparation; durable generation stays original', async () => {
  const parent = await scratch(), { root } = await createRun(parent, 0);
  const owners = await startWriters(root);
  try {
    const first = await freezeWriters(root, owners);
    await assert.rejects(freezeWriters(root, owners), /lab_owner_phase_invalid/);
    const retained = JSON.parse(await readFile(childPath(root, 'freeze.json'), 'utf8'));
    assert.equal(retained.epoch, owners.epoch);
    assert.deepEqual(retained.evidence, first.evidence);
  } finally { await joinWriters(owners); }
});
test('real AES-GCM tag/key/truncation and exact manifest mismatch all refuse', async () => {
  const parent = await scratch(), { root } = await createRun(parent, 0);
  await mkdir(childPath(root, 'data'));
  await writeFile(childPath(root, 'data/a.bin'), randomBytes(131072));
  await writeFile(childPath(root, 'freeze.json'), '{}');
  const entries = await inventory(root, ['data']), key = randomBytes(32), target = childPath(root, 'backup.lab');
  const expected = await seal(root, entries, target, key);
  assert.equal((await verify(target, expected, key)).files, 2);
  await assert.rejects(verify(target, expected, randomBytes(32)));
  await assert.rejects(verify(target, [{ ...expected[0], name: 'data/b.bin' }, expected[1]], key));
  await assert.rejects(verify(target, [{ ...expected[0], sha256: '0'.repeat(64) }, expected[1]], key));
  const original = await readFile(target);
  const badTag = Buffer.from(original); badTag[badTag.length - 1] ^= 1;
  await writeFile(target, badTag); await assert.rejects(verify(target, expected, key));
  await writeFile(target, original.subarray(0, -1)); await assert.rejects(verify(target, expected, key));
  await writeFile(target, original); assert.equal((await verify(target, expected, key)).files, 2);
  const source = await open(childPath(root, 'data/a.bin'), 'r+');
  try { await source.truncate(1); } finally { await source.close(); }
  await assert.rejects(seal(root, entries, childPath(root, 'second.lab'), key), /lab_source_changed/);
  await assert.rejects(seal(root, entries, target, key)); // exclusive output never overwrites
});
test('path escape, existing output, hardlink and unsupported workload refuse', async () => {
  const parent = await scratch(), { root } = await createRun(parent, 0);
  for (const invalid of ['../escape', 'a/../escape', '/absolute', 'a//b', 'a\\b', 'a/./b']) assert.throws(() => childPath(root, invalid));
  await mkdir(childPath(root, 'data'));
  await writeFile(childPath(root, 'data/a.bin'), 'synthetic');
  await link(childPath(root, 'data/a.bin'), childPath(root, 'data/b.bin'));
  await writeFile(childPath(root, 'freeze.json'), '{}');
  await assert.rejects(inventory(root, ['data']), /lab_hardlink_refused/);
  assert.throws(() => workload('unknown', 'many-small'));
  assert.throws(() => workload('small', 'unknown'));
  assert.deepEqual(workload('small', 'many-small'), { bytes: 1024 ** 3, files: 10000 });
  assert.deepEqual(workload('medium', 'many-small'), { bytes: 10 * 1024 ** 3, files: 100000 });
  assert.deepEqual(workload('large', 'many-small'), { bytes: 50 * 1024 ** 3, files: 500000 });
  assert.equal(workload('large', 'few-large').files, 12);
});
test('actual source append in stat/open window and inventory-time overwrite refuse', async () => {
  const parent = await scratch(), { root } = await createRun(parent, 0);
  await mkdir(childPath(root, 'data'));
  const source = childPath(root, 'data/a.bin');
  await writeFile(source, 'original'); await writeFile(childPath(root, 'freeze.json'), '{}');
  const entries = await inventory(root, ['data']), key = randomBytes(32);
  await assert.rejects(seal(root, entries, childPath(root, 'raced.lab'), key, async (pathname, flags) => {
    if (pathname === source) await appendFile(source, '!');
    return await open(pathname, flags);
  }), /lab_source_changed/);
  await writeFile(source, 'original');
  const baseline = await inventory(root, ['data']);
  await writeFile(source, 'replaced'); await utimes(source, 1, 1);
  await assert.rejects(seal(root, baseline, childPath(root, 'overwritten.lab'), key), /lab_source_changed/);
});
test('actual archive append in stat/open window refuses old authenticated prefix', async () => {
  const parent = await scratch(), { root } = await createRun(parent, 0);
  await mkdir(childPath(root, 'data'));
  await writeFile(childPath(root, 'data/a.bin'), 'synthetic'); await writeFile(childPath(root, 'freeze.json'), '{}');
  const key = randomBytes(32), target = childPath(root, 'backup.lab');
  const manifest = await seal(root, await inventory(root, ['data']), target, key);
  await assert.rejects(verify(target, manifest, key, async (pathname, flags) => {
    await appendFile(pathname, '!'); return await open(pathname, flags);
  }), /lab_archive_changed/);
});
test('real failed spawn closes without hanging and cannot publish a report', async () => {
  const parent = await scratch(), closed = [];
  await assert.rejects(runExperiment(parent, { bytes: 4096, files: 6 }, { spawn: (file, args, options) => {
    const child = fork(file, args, { ...options, execPath: path.join(parent, 'no-such-node.exe') });
    child.once('close', () => closed.push(true)); return child;
  } }));
  assert.equal(closed.length, 1);
  const [run] = await readdir(parent);
  await assert.rejects(readFile(path.join(parent, run, 'report.json')));
});
test('real abnormal child close after freeze is not a successful sample', async () => {
  const parent = await scratch(), closed = [];
  await assert.rejects(runExperiment(parent, { bytes: 4096, files: 6 }, { spawn: (file, args, options) => {
    const child = fork(file, args, options);
    child.once('close', () => closed.push(true));
    const send = child.send.bind(child);
    child.send = (message, ...args) => message.type === 'stop' ? child.kill() : send(message, ...args);
    return child;
  } }), /lab_child_close_failed/);
  assert.equal(closed.length, 3);
  const [run] = await readdir(parent);
  await assert.rejects(readFile(path.join(parent, run, 'report.json')));
});
test('aggregation separates host, storage class, volume and prototype identities', async () => {
  const parent = await scratch(), context = { labHostId: 'test-host', storageId: 'test-volume', storageClass: 'unknown' };
  const first = await runExperiment(parent, { bytes: 4096, files: 6 }, { context });
  const second = await runExperiment(parent, { bytes: 4096, files: 6 }, { context: { storageClass: context.storageClass, storageId: context.storageId, labHostId: context.labHostId } });
  const paths = [first, second].map(item => path.join(item.root, 'report.json'));
  assert.equal((await summarize(paths)).freeze.samples, 2);
  for (const field of ['labHostId', 'storageId', 'storageClass', 'observedDevice', 'observedFilesystemType']) {
    const changed = structuredClone(second.report); changed.context[field] += '-other';
    await writeFile(paths[1], JSON.stringify(changed)); await assert.rejects(summarize(paths));
  }
  const changed = structuredClone(second.report); changed.prototypeSHA256 = '0'.repeat(64);
  await writeFile(paths[1], JSON.stringify(changed)); await assert.rejects(summarize(paths));
  await writeFile(paths[1], JSON.stringify(second.report)); assert.equal((await summarize(paths)).backup.samples, 2);
});
test('full report write followed by actual sync/close failure never publishes success', async () => {
  for (const boundary of ['sync', 'close']) {
    const parent = await scratch();
    await assert.rejects(runExperiment(parent, { bytes: 4096, files: 6 }, { reportOpen: async (pathname, flags) => {
      const file = await open(pathname, flags), original = file[boundary].bind(file);
      file[boundary] = async () => { await original(); throw new Error('injected acknowledged-boundary failure'); };
      return file;
    } }));
    const [run] = await readdir(parent), root = path.join(parent, run);
    // The failed pending file really contains the complete JSON, not a shortcut
    // failure before write. Its absence from the final slot is what matters.
    assert.equal(JSON.parse(await readFile(path.join(root, 'pending-report.json'), 'utf8')).experiment, 'platform-v0');
    await assert.rejects(readFile(path.join(root, 'report.json')));
    await assert.rejects(summarize([path.join(root, 'pending-report.json')]), /lab_report_not_published/);
  }
});
