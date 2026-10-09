import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { selectNextHop, candidateHit, assertForcedPathWindow } from '../models/selection.mjs';
import { objectHash } from '../models/inputs.mjs';
import { HASH, NOW } from './fixtures.mjs';

const vectors = JSON.parse(await readFile(new URL('../../vectors/selection/paths.json', import.meta.url), 'utf8'));
function inputFor(patch) {
  const node = (id, version) => ({ deploymentId: id, channel: 'stable', version, minimumSourceVersion: '1.0.0',
    hopKind: 'ordinary', deploymentState: 'active', releaseState: 'closed', releaseTargetState: 'approved', artifactState: 'valid',
    supplyChainState: 'approved', supplyChainExpiresAt: null, releaseVisibleAt: NOW, installNotBefore: NOW, installNotAfter: null,
    isBaseline: id === 'baseline', ordinaryPathApproval: 'approved', ownRolloutState: id === 'candidate' ? 'running' : null,
    bridgeState: null, releaseTargetId: `target-${id}`, releaseTargetRevision: 1, packageId: `package-${id}`,
    packageSha256: HASH, transformId: `transform-${id}`, transformRevision: 1, upgradeType: 'normal', activationMode: 'directInstall', certificationState: 'certified' });
  const baseline = node('baseline', '3.0.0'); const candidate = node('candidate', patch.candidateVersion ?? '4.0.0'); const hop = node('hop', '2.0.0');
  baseline.minimumSourceVersion = patch.baselineMin ?? '1.0.0'; candidate.minimumSourceVersion = patch.candidateMin ?? '1.0.0';
  hop.minimumSourceVersion = patch.hopMinimum ?? '1.0.0';
  if (patch.baselinePaused) baseline.deploymentState = 'paused';
  if (patch.hopPaused) hop.ownRolloutState = 'paused';
  if (patch.bridge) Object.assign(hop, { hopKind: 'bridge', deploymentState: 'superseded', bridgeState: patch.bridgeDisabled ? 'disabled' : 'enabled' });
  const nodes = [baseline, candidate, hop];
  if (patch.secondHop) nodes.push(node('hop-high', '2.5.0'));
  const profile = { sourceProfileId: HASH, stableFactsSha256: HASH, state: patch.sourceState ?? 'selectable', policy: null };
  if (profile.state === 'forward-only') {
    const edge = { fromSourceProfileId: HASH, releaseTargetId: baseline.releaseTargetId, releaseTargetRevision: 1,
      toCanonicalAppVersion: baseline.version, packageId: baseline.packageId, packageSha256: HASH,
      transformId: baseline.transformId, transformRevision: 1, allowedChannels: patch.policy === 'wrong-channel' ? ['beta'] : ['stable'] };
    const edges = patch.policy === 'duplicate' ? [edge, structuredClone(edge)] : [edge];
    profile.policy = { policyId: 'policy-1', revision: 1, sha256: objectHash(edges), edges };
  }
  return { currentVersion: patch.current ?? '1.0.0', supportFloorVersion: '1.0.0', channel: 'stable', now: NOW,
    stableFactsSha256: HASH, baselineId: 'baseline', candidateId: patch.candidate ? 'candidate' : null,
    rollout: patch.candidate ? { percentage: 100, snPresent: true, included: false, excluded: false, bucketBasisPoints: 0 } : null,
    nodes, profiles: patch.profiles === 'none' ? [] : patch.profiles === 'duplicate' ? [profile, structuredClone(profile)] : [profile] };
}
for (const vector of vectors.cases) test(`selection fixed vector: ${vector.id}`, () => {
  const input = inputFor(vector.patch);
  if (vector.error !== undefined) assert.throws(() => selectNextHop(input), { code: vector.error });
  else assert.deepEqual(selectNextHop(input), vector.expected.reason === undefined
    ? { ...vector.expected, sourceProfileId: HASH } : vector.expected);
});
for (const { id, hit, ...input } of vectors.buckets) test(`private bucket vector: ${id}`, () => assert.equal(candidateHit(input), hit));
test('forced path certification and entire endpoint window are mandatory', () => {
  const endpoint = { upgradeType: 'forced', installNotBefore: NOW, installNotAfter: NOW + 100 };
  const hop = { activationMode: 'directInstall', certificationState: 'certified', releaseVisibleAt: NOW,
    installNotBefore: NOW, installNotAfter: NOW + 100 };
  assertForcedPathWindow(endpoint, [hop]);
  for (const invalid of [{ ...hop, installNotAfter: NOW + 99 }, { ...hop, installNotBefore: NOW + 1 },
    { ...hop, activationMode: 'stagedRestart' }, { ...hop, certificationState: 'unverified' }])
    assert.throws(() => assertForcedPathWindow(endpoint, [invalid]), { code: 'MODEL_FORCED_PATH_INVALID' });
  assert.throws(() => assertForcedPathWindow({ ...endpoint, installNotAfter: null }, [hop]));
  assertForcedPathWindow({ ...endpoint, installNotAfter: null }, [{ ...hop, installNotAfter: null }]);
});
