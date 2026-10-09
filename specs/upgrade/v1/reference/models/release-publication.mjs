import { canonicalize } from '../canonical-json.mjs';
import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { verifyEnvelope, verifyArchivedPackage, verifyArchivedRelease, assertReleaseRenewal, envelopeReference } from '../signatures.mjs';
import { verifyReleasePackage } from '../metadata-chain.mjs';
import { rolloutCommitment } from '../digests.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { nextRecord, captureRecord } from './atomic.mjs';
import { rootLineageReads } from './root-lineage.mjs';
import { releaseBusinessHash, deniedKeys } from './metadata-inputs.mjs';

/** Complete future public/private references define current, not an append-only
 * candidate catalog. Historical verification bytes leave current atomically.
 */
export function releasePublicationParts(records, proposal, verified, now) {
  const head = readRecord(records, proposal.headKey); const root = readRecord(records, proposal.rootKey);
  const inventory = readRecord(records, proposal.inventoryKey); const reads = []; const updates = new Map();
  const openings = new Map();
  requireCondition([proposal.candidateOpenings ?? [], proposal.privateReleaseUpdates ?? []]
    .every((items) => Array.isArray(items) && items.length <= 4096), 'MODEL_PUBLICATION_INVALID');
  for (const item of proposal.candidateOpenings ?? []) {
    assertApiShape('candidate-publication-opening', item);
    requireCondition(inventory.scopeKeys.includes(item.scopeKey) && !openings.has(item.scopeKey), 'MODEL_OPENING_INVALID');
    openings.set(item.scopeKey, item.opening);
  }
  for (const item of proposal.privateReleaseUpdates ?? []) {
    assertApiShape('private-release-update', item); const release = readRecord(records, item.releaseKey);
    requireCondition(!updates.has(release.releaseId) && item.envelope.signed.scope.releaseId === release.releaseId
      && release.metadataHeadKey === proposal.headKey, 'MODEL_MANIFEST_BINDING_INVALID');
    updates.set(release.releaseId, item.envelope); reads.push(item.releaseKey);
  }
  function accept(envelope) {
    const candidates = inventory.requiredEntityKeys.filter((key) => records[key].recordKind === 'release'
      && records[key].releaseId === envelope.signed.scope.releaseId);
    requireCondition(candidates.length === 1, 'MODEL_MANIFEST_BINDING_INVALID');
    const release = readRecord(records, candidates[0]); reads.push(candidates[0]);
    requireCondition(release.metadataHeadKey === proposal.headKey && release.state === 'closed'
      && release.appVersion === envelope.signed.appVersion
      && release.frozenBusinessSha256 === releaseBusinessHash(envelope.signed), 'MODEL_MANIFEST_BINDING_INVALID');
    verifyEnvelope(canonicalize(envelope), { trustedRoot: root.body, now, deniedKeyIds: deniedKeys(records, proposal.denyKey, 'release'),
      expected: { role: 'release', product: head.product, component: head.component, scope: { releaseId: release.releaseId } } });
    const previous = head.releaseEnvelopes[release.releaseId];
    if (previous !== undefined) {
      if (envelope.signed.revision === previous.signed.revision) requireCondition(objectHash(envelope) === objectHash(previous), 'MODEL_METADATA_FORK');
      else assertReleaseRenewal(previous.signed, envelope.signed);
    }
    return release;
  }
  const current = Object.create(null);
  for (const envelope of proposal.bundle.members.filter((item) => item.signed.role === 'release')) {
    accept(envelope); current[envelope.signed.scope.releaseId] = structuredClone(envelope);
  }
  const usedUpdates = new Set(); const resolvedOpenings = new Map();
  for (const target of verified.targets) {
    const scopeKey = inventory.scopeKeys.find((key) => records[key].channel === target.scope.channel && records[key].targetKey === target.scope.targetKey);
    const scope = readRecord(records, scopeKey); reads.push(scopeKey);
    const opening = target.rolloutSetCommitment === rolloutCommitment(null) ? null
      : openings.get(scopeKey) ?? scope.currentCandidateOpening;
    requireCondition(opening !== undefined && target.rolloutSetCommitment === rolloutCommitment(opening), 'MODEL_OPENING_INVALID');
    if (proposal.mode === 'refresh' || !proposal.affectedScopeKeys.includes(scopeKey)) requireCondition(
      sameJson(opening, scope.currentCandidateOpening ?? null), 'MODEL_SELECTION_CHANGED');
    resolvedOpenings.set(scopeKey, opening);
    if (opening === null) { requireCondition(!openings.has(scopeKey), 'MODEL_OPENING_INVALID'); continue; }
    const deploymentKeys = inventory.requiredEntityKeys.filter((key) => records[key].recordKind === 'deployment'
      && records[key].deploymentId === opening.deploymentId && records[key].channel === scope.channel && records[key].targetKey === scope.targetKey);
    requireCondition(deploymentKeys.length === 1, 'MODEL_OPENING_INVALID');
    const deploymentKey = deploymentKeys[0]; const deployment = readRecord(records, deploymentKey);
    const rollout = readRecord(records, deployment.rolloutKey); const release = readRecord(records, deployment.releaseKey);
    reads.push(deploymentKey, deployment.rolloutKey, deployment.releaseKey);
    requireCondition(opening.targetKey === scope.targetKey && opening.rolloutId === rollout.rolloutId
      && rollout.deploymentKey === deploymentKey && rollout.scopeKey === scopeKey
      && [deployment.revision, deployment.revision + 1].includes(opening.deploymentRevision)
      && [rollout.revision, rollout.revision + 1].includes(opening.rolloutRevision), 'MODEL_OPENING_INVALID');
    const envelope = updates.get(release.releaseId) ?? head.releaseEnvelopes[release.releaseId];
    requireCondition(envelope !== undefined && objectHash(envelope) === opening.releaseEnvelopeSha256, 'MODEL_OPENING_INVALID');
    accept(envelope);
    requireCondition(current[release.releaseId] === undefined || objectHash(current[release.releaseId]) === objectHash(envelope),
      'MODEL_MANIFEST_BINDING_INVALID');
    current[release.releaseId] = structuredClone(envelope);
    if (updates.has(release.releaseId)) usedUpdates.add(release.releaseId);
  }
  requireCondition([...openings].every(([key, value]) => resolvedOpenings.has(key) && sameJson(value, resolvedOpenings.get(key)))
    && usedUpdates.size === updates.size, 'MODEL_PUBLICATION_INVALID');
  const retired = Object.values(head.releaseEnvelopes).filter((old) => current[old.signed.scope.releaseId] === undefined
    || objectHash(current[old.signed.scope.releaseId]) !== objectHash(old));
  const writes = [];
  if (retired.length > 0) {
    const archive = readRecord(records, head.verificationArchiveKey); assertApiShape('verification-archive', archive);
    requireCondition(archive.product === head.product && archive.component === head.component, 'MODEL_SCOPE_INVALID');
    reads.push(head.verificationArchiveKey); const entries = [...archive.entries];
    reads.push(...rootLineageReads(records, root.rootLineageKey, root.body));
    for (const envelope of retired) {
      verifyArchivedRelease(canonicalize(envelope), { archivedRoot: root.body,
        expected: { role: 'release', product: head.product, component: head.component, scope: envelope.signed.scope } });
      const packages = envelope.signed.targets.map((target) => {
        const publicPackage = head.bundle?.members.find((item) => item.signed.role === 'package' && objectHash(item) === target.packageManifest.envelopeSha256);
        const targetKey = publicPackage !== undefined ? null : inventory.requiredEntityKeys.find((key) => records[key].recordKind === 'releaseTarget'
          && records[key].releaseTargetId === target.releaseTargetId && records[key].packageEnvelope !== undefined
          && objectHash(records[key].packageEnvelope) === target.packageManifest.envelopeSha256);
        requireCondition(targetKey !== undefined, 'MODEL_ARCHIVE_INCOMPLETE');
        if (targetKey !== null) reads.push(targetKey);
        const item = publicPackage ?? records[targetKey].packageEnvelope;
        requireCondition(sameJson(envelopeReference(canonicalize(item)), target.packageManifest), 'MODEL_ARCHIVE_INCOMPLETE');
        verifyReleasePackage({ trustedRoot: root.body, product: head.product, component: head.component, now,
          release: envelope.signed, targetKey: target.targetKey, packageBytes: canonicalize(item) });
        return item;
      });
      for (const item of packages) verifyArchivedPackage(canonicalize(item), { archivedRoot: root.body, now,
        expected: { role: 'package', product: head.product, component: head.component, scope: item.signed.scope } });
      if (!entries.some((entry) => objectHash(entry.release) === objectHash(envelope))) entries.push({ archivedAt: now,
        rootHead: captureRecord(records, proposal.rootKey), lineage: structuredClone(records[root.rootLineageKey]),
        root: structuredClone(root.body), release: structuredClone(envelope), packages: structuredClone(packages) });
    }
    const next = nextRecord(archive, { entries }); assertApiShape('verification-archive', next);
    writes.push({ key: head.verificationArchiveKey, value: next });
  }
  requireCondition(reads.every((key) => proposal.readSet.some((read) => read.key === key)), 'MODEL_READ_SET_INCOMPLETE');
  return { reads: [...new Set(reads)], writes, releaseEnvelopes: current, candidateOpenings: resolvedOpenings };
}
