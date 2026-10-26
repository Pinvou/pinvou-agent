// Isolated module probes, not an installed application/update journey.
import { randomBytes } from 'node:crypto';
import { readFile, writeFile, appendFile } from 'node:fs/promises';
import { platform, arch } from 'node:os';
import { createRun, childPath, requireThat } from '../upgrade-platform-lab/src/owned-files.mjs';
import { startWriters, freezeWriters, joinWriters, writerNames } from '../upgrade-platform-lab/src/coordination.mjs';
import { generate } from '../upgrade-platform-lab/src/workload.mjs';
import { inventory, seal, verify } from '../upgrade-platform-lab/src/archive.mjs';
import { prototypeIdentity } from '../upgrade-platform-lab/src/run.mjs';
import { checkObservation, unsupported } from '../../specs/upgrade/v1/platforms/contract.mjs';
import { vectors } from '../../specs/upgrade/v1/platforms/vectors.mjs';

function actualContext() {
  const os = { win32: 'windows', linux: 'linux', darwin: 'macos' }[platform()];
  const hostArch = { x64: 'x86_64', arm64: 'arm64' }[arch()];
  const targetKey = os === 'windows' ? 'windows-x86_64-nsis-perMachine'
    : os === 'macos' ? 'macos-universal-dmg-perMachine'
      : hostArch === 'arm64' ? 'linux-arm64-deb-perMachine' : 'linux-x86_64-deb-perMachine';
  return { os, hostArch, targetKey, upgradeType: 'normal', installationScopeId: 'synthetic-installation' };
}
// All production ports really refuse: the lab cannot manufacture protected
// ownership, admission fencing or backup registration from synthetic observations.
export class ExperimentalAdapter {
  invoke(operation, request) { return unsupported(actualContext(), operation, request); }
}
async function rejected(action, expectedCode) {
  try { await action(); } catch (error) {
    if (expectedCode !== undefined) requireThat(error.code === expectedCode, 'conformance_wrong_refusal');
    return;
  }
  throw new Error('CONFORMANCE_NEGATIVE_ACCEPTED');
}
export async function observeLab(parent) {
  const prototypeSHA256 = await prototypeIdentity();
  const adapter = new ExperimentalAdapter();
  const operations = vectors().map(({ record }) => {
    const observation = adapter.invoke(record.operation, record.request);
    return { operation: observation.operation, status: observation.result.kind, reason: observation.result.reason };
  });
  const { root, runId } = await createRun(parent, 1024 ** 2), key = randomBytes(32);
  const cases = [];
  const passed = id => cases.push({ id, provenance: 'experimental-primitive', status: 'passed' });
  let owners;
  try {
    owners = await startWriters(root);
    await generate(root, { bytes: 65536, files: 6 });
    const frozen = await freezeWriters(root, owners);
    requireThat(frozen.evidence.length === writerNames.length && frozen.evidence.every(item => item.epoch === owners.epoch));
    passed('lab.same-account-writers-saved-stopped-and-SQLite-readback');
    const model = vectors().find(({ record }) => record.operation === 'freezeAffectedScopes').record;
    model.context = actualContext();
    model.request.freezeEpoch = owners.epoch; model.request.expectedWriterIds = [...writerNames];
    model.result.facts.freezeEpoch = owners.epoch;
    model.result.facts.writers = frozen.evidence.map(item => ({ writerId: item.writer, epoch: item.epoch,
      saved: true, stopped: true, consistent: true }));
    model.result.facts.admissionFenced = false; // No actual machine-wide fence.
    await rejected(() => checkObservation(model), 'PLATFORM_FACT_INVALID');
    passed('lab.synthetic-freeze-cannot-prove-production-admission');
    await rejected(() => freezeWriters(root, owners), 'lab_owner_phase_invalid');
    const retained = JSON.parse(await readFile(childPath(root, 'freeze.json'), 'utf8'));
    requireThat(retained.epoch === owners.epoch);
    passed('lab.repeat-freeze-refuses-with-original-epoch-retained');
    const entries = await inventory(root, writerNames), output = childPath(root, 'backup.lab');
    const manifest = await seal(root, entries, output, key), original = await readFile(output);
    const complete = await verify(output, manifest, key);
    requireThat(complete.files === entries.length && complete.bytes >= 65536);
    passed('lab.experimental-AEAD-complete-manifest-readback');
    await rejected(() => verify(output, manifest, randomBytes(32)));
    passed('lab.wrong-key-refuses');
    await rejected(() => verify(output, [...manifest].reverse(), key), 'lab_archive_manifest_mismatch');
    passed('lab.reordered-manifest-refuses');
    for (const [name, changed] of [
      ['bad-tag.lab', Buffer.from(original)], ['truncated.lab', original.subarray(0, -1)],
      ['unknown-format.lab', Buffer.from(original)],
    ]) {
      if (name === 'bad-tag.lab') changed[changed.length - 1] ^= 1;
      if (name === 'unknown-format.lab') changed[0] ^= 1;
      const destination = childPath(root, name); await writeFile(destination, changed, { flag: 'wx', mode: 0o600 });
      await rejected(() => verify(destination, manifest, key)); passed(`lab.${name}-refuses`);
    }
    await rejected(() => seal(root, entries, output, key), 'EEXIST');
    await appendFile(childPath(root, entries[0].name), Buffer.from([1]));
    await rejected(() => seal(root, entries, childPath(root, 'changed-source.lab'), key), 'lab_source_changed');
    requireThat((await verify(output, manifest, key)).bytes === complete.bytes);
    passed('lab.exclusive-output-source-change-and-next-readback');
  } finally {
    key.fill(0);
    if (owners) await joinWriters(owners);
  }
  passed('lab.actual-writer-close-before-report');
  const second = await createRun(parent, 0);
  let unsavable;
  try {
    unsavable = await startWriters(second.root, true);
    await rejected(() => freezeWriters(second.root, unsavable), 'lab_freeze_incomplete');
    await rejected(() => readFile(childPath(second.root, 'backup.lab')), 'ENOENT');
  } finally { if (unsavable) await joinWriters(unsavable); }
  passed('lab.unsavable-writer-refuses-before-backup');
  requireThat(await prototypeIdentity() === prototypeSHA256, 'conformance_source_changed');
  return { prototypeSHA256, context: actualContext(), runIds: [runId, second.runId], operations, cases,
    syntheticBytes: 65536, productionBackupFormat: false, qualification: false,
    keyCustody: 'ephemeral-key-wiped-no-crash-recovery', scratch: 'small-owned-runs-retained-for-diagnosis' };
}
