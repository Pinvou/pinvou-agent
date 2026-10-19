import { createHash } from 'node:crypto';
import { platform, arch } from 'node:os';
import { requireThat } from './owned-files.mjs';
import { profiles } from './workload.mjs';

export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
export const encode = value => JSON.stringify(value);
export function keys(value, expected) {
  requireThat(value && typeof value === 'object' && !Array.isArray(value) &&
    Object.keys(value).sort().join(',') === [...expected].sort().join(','), 'lab_series_shape_invalid');
}
function label(value) { requireThat(typeof value === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(value), 'lab_series_label_invalid'); }
export function validatePlan(input) {
  keys(input, ['repetitions', 'workload', 'shape', 'context', 'experiment']);
  requireThat(Number.isSafeInteger(input.repetitions) && input.repetitions >= 1 && input.repetitions <= 1000);
  keys(input.workload, ['bytes', 'files']);
  const { bytes, files } = input.workload;
  requireThat(Number.isSafeInteger(bytes) && Number.isSafeInteger(files) && bytes >= files && files > 0 &&
    files <= profiles.large.files && bytes <= profiles.large.bytes);
  keys(input.context, ['labHostId', 'storageId', 'storageClass']);
  Object.values(input.context).forEach(label);
  keys(input.experiment, ['targetKey', 'hostKind', 'cacheState', 'mode']);
  Object.values(input.experiment).forEach(label);
  requireThat(['developer', 'physical', 'virtual'].includes(input.experiment.hostKind));
  requireThat(['uncontrolled', 'cold', 'warm'].includes(input.experiment.cacheState));
  requireThat(['synthetic-freeze-backup-v0', 'preinstall', 'direct-install', 'activation'].includes(input.experiment.mode));
  requireThat(['many-small', 'few-large', 'custom-smoke'].includes(input.shape));
  const baseline = Object.values(profiles).some(profile => profile.bytes === bytes &&
    (input.shape === 'many-small' ? files === profile.files : input.shape === 'few-large' && files === 12));
  requireThat(input.shape === 'custom-smoke' || baseline, 'lab_series_shape_workload_mismatch');
  // Canonical field order prevents a caller's JSON property order from creating
  // accidental new groups. Detach every mutable caller-owned object.
  return {
    repetitions: input.repetitions, workload: { bytes, files }, shape: input.shape,
    context: { labHostId: input.context.labHostId, storageId: input.context.storageId, storageClass: input.context.storageClass },
    experiment: { targetKey: input.experiment.targetKey, hostKind: input.experiment.hostKind,
      cacheState: input.experiment.cacheState, mode: input.experiment.mode },
  };
}
export function unsupportedReason(plan, observed = { os: platform(), arch: arch() }) {
  keys(observed, ['os', 'arch']);
  label(observed.os); label(observed.arch);
  const host = `${observed.os}-${observed.arch}`;
  const targets = {
    'windows-x86_64-nsis-perMachine': ['win32-x64'],
    'linux-x86_64-deb-perMachine': ['linux-x64'],
    'linux-arm64-deb-perMachine': ['linux-arm64'],
    'macos-universal-dmg-perMachine': ['darwin-arm64', 'darwin-x64'],
  };
  if (!Object.hasOwn(targets, plan.experiment.targetKey) || !targets[plan.experiment.targetKey].includes(host)) return 'target-host-not-supported';
  if (plan.experiment.mode !== 'synthetic-freeze-backup-v0') return 'production-mode-not-implemented';
  if (plan.experiment.cacheState !== 'uncontrolled') return 'cache-controller-not-implemented';
  return null;
}
export function distribution(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return { samples: sorted.length, medianMilliseconds: sorted[Math.floor((sorted.length - 1) / 2)],
    p95Milliseconds: sorted[Math.ceil(sorted.length * .95) - 1], maximumMilliseconds: sorted.at(-1) };
}
export function summary(plan, attempts, blockedReason = null) {
  const counts = { successful: 0, failed: 0, unsupported: 0, aborted: 0, unattempted: plan.repetitions - attempts.length };
  for (const attempt of attempts) counts[({ success: 'successful', failed: 'failed', unsupported: 'unsupported', aborted: 'aborted' })[attempt.status]]++;
  const complete = attempts.length === plan.repetitions && counts.successful === plan.repetitions;
  return { status: complete ? 'complete' : blockedReason ? 'unsupported' : counts.failed ? 'failed' : 'incomplete', counts,
    statistics: complete ? { freeze: distribution(attempts.map(item => item.sample.freeze.milliseconds)),
      backup: distribution(attempts.map(item => item.sample.backup.milliseconds)) } : null,
    baselineWorkload: plan.shape !== 'custom-smoke',
    sampleRequirementMet: complete && counts.successful >= 30 && plan.shape !== 'custom-smoke',
    proposal: { certifiedBudget: null, approvedBudget: null, qualification: false,
      cacheControl: 'not-implemented', actualUsersAndServices: 'not-exercised',
      productionModes: 'not-implemented', protectedTime: 'unproven',
      decision: 'owner-review-required-after-complete-platform-evidence' } };
}
