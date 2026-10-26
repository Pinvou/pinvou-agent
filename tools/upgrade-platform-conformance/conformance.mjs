import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { platform, arch, release } from 'node:os';
import { pathToFileURL } from 'node:url';
import { revision } from '../../specs/upgrade/v1/platforms/contract.mjs';
import { runModelCases } from './model.mjs';

export async function sourceIdentity() {
  const files = ['../../specs/upgrade/v1/platforms/values.mjs', '../../specs/upgrade/v1/platforms/identity.mjs',
    '../../specs/upgrade/v1/platforms/ownership.mjs', '../../specs/upgrade/v1/platforms/storage.mjs',
    '../../specs/upgrade/v1/platforms/time.mjs', '../../specs/upgrade/v1/platforms/execution.mjs',
    '../../specs/upgrade/v1/backup/operations.mjs', '../../specs/upgrade/v1/platforms/contract.mjs',
    '../../specs/upgrade/v1/platforms/vectors.mjs', './model.mjs', './lab-adapter.mjs', './conformance.mjs'];
  const digest = createHash('sha256'), sources = [];
  for (const name of files) {
    const bytes = await readFile(new URL(name, import.meta.url));
    const sha256 = createHash('sha256').update(bytes).digest('hex');
    sources.push({ name, sha256 }); digest.update(name); digest.update('\0'); digest.update(bytes); digest.update('\0');
  }
  return { sha256: digest.digest('hex'), sources };
}
export async function conformance(mode, parent) {
  if (!['model', 'lab-primitives'].includes(mode) || (mode === 'model' ? parent !== undefined : parent === undefined))
    throw new Error('CONFORMANCE_USAGE');
  const source = await sourceIdentity();
  const cases = runModelCases();
  let lab = null;
  if (mode === 'lab-primitives') {
    const { observeLab } = await import('./lab-adapter.mjs');
    lab = await observeLab(parent); cases.push(...lab.cases);
  }
  return { contract: revision, mode, source, environment: { os: platform(), hostArch: arch(), release: release(), node: process.version },
    cases, lab, qualification: false, productionOperations: 'unimplemented',
    limits: ['Observation DATA is not native authority.', 'Model cases do not exercise production platform operations.',
      'Lab primitives use one OS account, ephemeral keys and an experimental container.', 'No E2E, real installer, service or user data is exercised.'] };
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.length < 3 || process.argv.length > 4) throw new Error('CONFORMANCE_USAGE');
    process.stdout.write(`${JSON.stringify(await conformance(process.argv[2], process.argv[3]), null, 2)}\n`);
  } catch { process.stderr.write('CONFORMANCE_FAILED\n'); process.exitCode = 1; }
}
