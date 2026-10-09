import { sha256 } from './digests.mjs';
import { requireCondition } from './errors.mjs';
import { assertOpening, sameJson } from './semantics.mjs';
import { assertReference, envelopeReference, verifyEnvelope } from './signatures.mjs';

/** Verify a complete public component snapshot, including all its Targets.
 * Release/package membership is derived from those public baseline/path
 * references. Additional catalogs or unreferenced candidate objects are fatal.
 * No networking, publication or version selection is performed here.
 */
export function verifyPublicChain({ trustedRoot, product, component, now,
  timestampBytes, snapshotBytes, members, deniedKeyIds = [] }) {
  const expected = (role, scope = {}) => ({ role, product, component, scope });
  const verify = (bytes, role, scope) => verifyEnvelope(bytes,
    { trustedRoot, expected: expected(role, scope), now, deniedKeyIds });
  const timestamp = verify(timestampBytes, 'timestamp', {}).signed;
  assertReference(timestamp.snapshot, snapshotBytes);
  const snapshot = verify(snapshotBytes, 'snapshot', {}).signed;
  const available = new Map();
  for (const bytes of members) {
    const hash = sha256(bytes);
    requireCondition(!available.has(hash), 'MEMBER_DUPLICATE');
    available.set(hash, bytes);
  }
  const publicObjects = new Map();
  for (const reference of snapshot.entries) {
    const bytes = available.get(reference.envelopeSha256);
    requireCondition(bytes !== undefined, 'PUBLIC_MEMBER_MISSING');
    assertReference(reference, bytes);
    publicObjects.set(reference.envelopeSha256, verify(bytes, reference.role, reference.scope).signed);
  }
  requireCondition(available.size === publicObjects.size, 'PUBLIC_MEMBER_UNREFERENCED');
  const permitted = new Set();
  const targets = [];
  const releases = [];
  for (const [hash, signed] of publicObjects) {
    if (signed.role !== 'target') continue;
    targets.push(signed);
    permitted.add(hash);
    for (const reference of [signed.baselineRelease, ...signed.ordinaryPaths.map((path) => path.release),
      ...signed.bridgePaths.map((path) => path.release)]) {
      const release = publicObjects.get(reference.envelopeSha256);
      requireCondition(release?.role === 'release', 'PUBLIC_RELEASE_MISSING');
      requireCondition(release.targets.some((target) => target.targetKey === signed.scope.targetKey), 'PUBLIC_TARGET_MISMATCH');
      assertReference(reference, available.get(reference.envelopeSha256));
      permitted.add(reference.envelopeSha256);
      releases.push(release);
    }
  }
  requireCondition(targets.length > 0, 'PUBLIC_TARGET_MISSING');
  for (const release of releases) {
    for (const target of release.targets) {
      const reference = target.packageManifest;
      const packageManifest = publicObjects.get(reference.envelopeSha256);
      requireCondition(packageManifest?.role === 'package', 'PUBLIC_PACKAGE_MISSING');
      assertReference(reference, available.get(reference.envelopeSha256));
      assertReleasePackageBinding(release, target, packageManifest);
      permitted.add(reference.envelopeSha256);
    }
  }
  requireCondition([...publicObjects.keys()].every((hash) => permitted.has(hash)), 'PUBLIC_CANDIDATE_LEAK');
  return { timestamp, snapshot, targets, releases, packages:
    [...publicObjects.values()].filter((signed) => signed.role === 'package') };
}

function assertReleasePackageBinding(release, target, packageManifest) {
  requireCondition(packageManifest.scope.targetKey === target.targetKey
    && packageManifest.appVersion === release.appVersion
    && sameJson(packageManifest.launcher, target.launcher)
    && target.activationModes.every((mode) => packageManifest.activationModes.includes(mode)),
  'RELEASE_PACKAGE_MISMATCH');
}

export function verifyReleasePackage({ trustedRoot, product, component, release,
  packageBytes, targetKey, now, deniedKeyIds = [] }) {
  const target = release.targets.find((entry) => entry.targetKey === targetKey);
  requireCondition(target !== undefined, 'RELEASE_TARGET_MISSING');
  assertReference(target.packageManifest, packageBytes);
  const packageManifest = verifyEnvelope(packageBytes, { trustedRoot,
    expected: { role: 'package', product, component, scope: { targetKey } }, now, deniedKeyIds }).signed;
  assertReleasePackageBinding(release, target, packageManifest);
  return packageManifest;
}

export function verifyCandidateRelease({ trustedRoot, product, component, targetBytes, channel, targetKey,
  releaseBytes, expectedReleaseId, opening, now, deniedKeyIds = [] }) {
  const target = verifyEnvelope(targetBytes, { trustedRoot,
    expected: { role: 'target', product, component, scope: { channel, targetKey } }, now, deniedKeyIds }).signed;
  requireCondition(target.baselineRelease.envelopeSha256 !== sha256(releaseBytes), 'CANDIDATE_IS_BASELINE');
  assertOpening(target, sha256(releaseBytes), opening);
  const release = verifyEnvelope(releaseBytes, { trustedRoot,
    expected: { role: 'release', product, component, scope: { releaseId: expectedReleaseId } }, now, deniedKeyIds }).signed;
  requireCondition(release.targets.some((entry) => entry.targetKey === target.scope.targetKey), 'CANDIDATE_TARGET_MISSING');
  return release;
}

export { envelopeReference };
