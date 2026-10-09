import { canonicalize } from '../canonical-json.mjs';
import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { verifyRootSuccessor, verifyEnvelope, assertReleaseRenewal } from '../signatures.mjs';
import { captureRecord, nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { deniedKeyOptions, denyRecord, deniedKeys } from './metadata-inputs.mjs';
import { keyRetentionReads } from './key-retention.mjs';
import { rootChangeBody, signingKeyDenyBody, keyApprovalReads } from './key-approval.mjs';
import { parseJson } from '../canonical-json.mjs';
import { releasePublicationParts } from './release-publication.mjs';
import { rootLineageReads } from './root-lineage.mjs';
import { targetConfiguration, reconciliationPathReads } from './target-policy.mjs';

const scopeIdentity = (scope) => `${scope.channel}:${scope.targetKey}`;
const releaseBusinessReference = ({ role, scope }) => ({ role, scope });
export function targetSelection({ version, issuedAt, expiresAt, ...selection }) {
  return { ...selection, baselineRelease: releaseBusinessReference(selection.baselineRelease),
    ordinaryPaths: selection.ordinaryPaths.map((path) => ({ ...path, release: releaseBusinessReference(path.release) })),
    bridgePaths: selection.bridgePaths.map((path) => ({ ...path, release: releaseBusinessReference(path.release) })) };
}
function compareReads(records, reads) {
  requireCondition(Array.isArray(reads) && reads.length > 0 && new Set(reads.map((read) => read.key)).size === reads.length,
    'MODEL_READ_SET_INCOMPLETE');
  for (const read of reads) requireCondition(sameJson(captureRecord(records, read.key), read), 'MODEL_CAS_CONFLICT');
}

/** Shared publication boundary used by entity and rollout compositions. The
 * owning service supplies signed artifacts, not a client 'publicationPassed'.
 * Component inventory is complete, and every referenced input joins the CAS.
 */
export function publicationParts(records, proposal, now) {
  const { rootKey, headKey, inventoryKey, denyKey, mode, affectedScopeKeys, readSet, bundle } = proposal;
  requireCondition(['selection', 'refresh', 'reconcile'].includes(mode), 'MODEL_PUBLICATION_INVALID');
  const root = readRecord(records, rootKey); const head = readRecord(records, headKey);
  const inventory = readRecord(records, inventoryKey);
  requireCondition(root.published === true && root.revision > 0 && readSet.some((read) => read.key === denyKey), 'MODEL_ROOT_NOT_PUBLISHED');
  const actualScopeKeys = Object.entries(records).filter(([, record]) => record.recordKind === 'selectionScope'
    && record.product === head.product && record.component === head.component).map(([key]) => key).sort();
  requireCondition(sameJson(actualScopeKeys, [...inventory.scopeKeys].sort()), 'MODEL_SNAPSHOT_INCOMPLETE');
  compareReads(records, readSet);
  requireCondition(sameJson(proposal.baseRoot, captureRecord(records, rootKey))
    && sameJson(proposal.baseHead, captureRecord(records, headKey))
    && inventory.product === head.product && inventory.component === head.component && root.product === head.product
    && Array.isArray(inventory.scopeKeys) && Array.isArray(inventory.requiredEntityKeys)
    && [...inventory.scopeKeys, ...inventory.requiredEntityKeys, rootKey, headKey, inventoryKey]
      .every((key) => readSet.some((read) => read.key === key)), 'MODEL_READ_SET_INCOMPLETE');
  requireCondition(new Set(affectedScopeKeys).size === affectedScopeKeys.length
    && affectedScopeKeys.every((key) => inventory.scopeKeys.includes(key))
    && (mode === 'refresh' ? affectedScopeKeys.length === 0 : affectedScopeKeys.length > 0), 'MODEL_PUBLICATION_INVALID');
  const verified = verifyPublicChain({ trustedRoot: root.body, product: head.product, component: head.component, now,
    timestampBytes: canonicalize(bundle.timestamp), snapshotBytes: canonicalize(bundle.snapshot),
    members: bundle.members.map(canonicalize), ...deniedKeyOptions(records, denyKey) });
  const current = head.published ? head.bundle : null;
  if (current !== null) {
    requireCondition(verified.timestamp.version > current.timestamp.signed.version
      && verified.snapshot.version > current.snapshot.signed.version, 'MODEL_METADATA_ROLLBACK');
    for (const release of verified.releases) {
      const previous = current.members.find((member) => member.signed.role === 'release'
        && sameJson(member.signed.scope, release.scope));
      if (previous !== undefined && release.revision !== previous.signed.revision) assertReleaseRenewal(previous.signed, release);
      else if (previous !== undefined) requireCondition(objectHash(previous) === objectHash(bundle.members.find((member) =>
        member.signed.role === 'release' && sameJson(member.signed.scope, release.scope))), 'MODEL_METADATA_FORK');
    }
  } else requireCondition(mode === 'selection' && head.revision === 0, 'MODEL_GENESIS_INVALID');
  const expectedTargets = inventory.scopeKeys.filter((key) => records[key].state === 'active' || affectedScopeKeys.includes(key))
    .map((key) => scopeIdentity(records[key])).sort();
  requireCondition(sameJson(expectedTargets, verified.targets.map((target) => scopeIdentity(target.scope)).sort()),
    'MODEL_SNAPSHOT_INCOMPLETE');
  const writes = [];
  for (const scopeKey of inventory.scopeKeys) {
    const scope = readRecord(records, scopeKey); const affected = affectedScopeKeys.includes(scopeKey);
    const target = verified.targets.find((item) => scopeIdentity(item.scope) === scopeIdentity(scope));
    if (target === undefined) continue;
    const generation = mode === 'selection' && affected ? scope.selectionGeneration + 1 : scope.selectionGeneration;
    requireCondition(target.selectionGeneration === generation && target.supportFloorVersion === scope.supportFloorVersion,
      'MODEL_GENERATION_MISMATCH');
    const previous = current?.members.find((member) => member.signed.role === 'target'
      && scopeIdentity(member.signed.scope) === scopeIdentity(scope));
    if (previous !== undefined) {
      requireCondition(sameJson(targetConfiguration(target), targetConfiguration(previous.signed)), 'MODEL_SELECTION_CHANGED');
      const proposedEnvelope = bundle.members.find((member) => member.signed.role === 'target'
        && scopeIdentity(member.signed.scope) === scopeIdentity(scope));
      requireCondition(target.version > previous.signed.version
        || target.version === previous.signed.version && objectHash(proposedEnvelope) === objectHash(previous), 'MODEL_METADATA_FORK');
      if (!affected) {
        requireCondition(sameJson(targetSelection(target), targetSelection(previous.signed)), 'MODEL_SELECTION_CHANGED');
        // Public-chain verification resolves every full reference; the Release
        // loop above permits only immutable-business higher revisions. All three
        // locations may therefore refresh byte/time references without changing
        // the selection or its generation.
      }
      if (affected && scope.hasCandidate === true) requireCondition(target.rolloutSetCommitment !== previous.signed.rolloutSetCommitment,
        'MODEL_CANDIDATE_SALT_REUSED');
    }
    if (affected) writes.push({ key: scopeKey, value: nextRecord(scope, { selectionGeneration: generation,
      metadataSyncPending: false, targetEnvelopeSha256: objectHash(bundle.members.find((member) => member.signed === target
        || member.signed.role === 'target' && scopeIdentity(member.signed.scope) === scopeIdentity(scope))) }) });
  }
  const releases = releasePublicationParts(records, proposal, verified, now);
  for (const write of writes) {
    const next = releases.candidateOpenings.get(write.key); const old = records[write.key].currentCandidateOpening;
    requireCondition(next == null || old == null || next.leafSalt !== old.leafSalt, 'MODEL_CANDIDATE_SALT_REUSED');
    write.value.currentCandidateOpening = structuredClone(next);
  }
  writes.push(...releases.writes);
  writes.push({ key: headKey, value: nextRecord(head, { published: true, releaseEnvelopes: releases.releaseEnvelopes, rootHeadRevisionAtPublish: root.revision,
    bundle: structuredClone(bundle) }) });
  return { reads: readSet.map((read) => read.key), writes, candidateOpenings: releases.candidateOpenings,
    facts: [{ kind: mode === 'refresh' ? 'metadata-refreshed' : 'selection-published', committedAt: now, headKey }] };
}
export function planPublish(records, command, now) {
  // Selection changes require a domain composition that validates and writes
  // the future baseline/candidate with this publication. Refresh has no such
  // domain writes; exposing a standalone selection command would bypass them.
  requireCondition(command.publication.mode === 'refresh', 'MODEL_DOMAIN_COMPOSITION_REQUIRED');
  const parts = publicationParts(records, command.publication, now);
  return commandPlan(records, command, now, parts.reads, parts.writes, parts.facts, { headKey: command.publication.headKey });
}
export function planPublishRoot(records, command, now) {
  const root = readRecord(records, command.rootKey); const inventory = readRecord(records, command.activeComponentsKey);
  const actualHeads = Object.entries(records).filter(([, record]) => record.recordKind === 'metadataHead'
    && record.product === root.product).map(([key]) => key).sort();
  requireCondition(sameJson(actualHeads, [...inventory.componentHeadKeys].sort())
    && new Set(inventory.componentHeadKeys.map((key) => records[key].component)).size === inventory.componentHeadKeys.length,
  'MODEL_ACTIVE_COMPONENTS_INCOMPLETE');
  requireCondition(sameJson(command.baseRoot, captureRecord(records, command.rootKey)), 'MODEL_CAS_CONFLICT');
  const activeEnvelopes = []; const reads = [command.rootKey, command.activeComponentsKey, command.denyKey];
  const deniedRootKeys = deniedKeys(records, command.denyKey, 'root');
  requireCondition(parseJson(command.successorBytes).signatures.every((signature) => !deniedRootKeys.includes(signature.keyId)),
    'SIGNING_KEY_DENIED');
  reads.push(...keyApprovalReads(records, command, root.product, command.rootKey, root.revision,
    rootChangeBody(records, command), now));
  if (root.revision === 0) {
    const anchor = readRecord(records, command.anchorKey); reads.push(command.anchorKey);
    requireCondition(root.published === false && anchor.recordKind === 'initialRootAnchor' && anchor.state === 'provisioned'
      && anchor.product === root.product && anchor.bodySha256 === objectHash(root.body)
      && inventory.componentHeadKeys.every((key) => records[key].published === false && records[key].revision === 0),
    'MODEL_GENESIS_INVALID');
  }
  for (const key of inventory.componentHeadKeys) {
    const head = readRecord(records, key); reads.push(key);
    if (!head.published) continue;
    const envelopes = [head.bundle.timestamp, head.bundle.snapshot, ...head.bundle.members, ...Object.values(head.releaseEnvelopes)];
    for (const envelope of new Map(envelopes.map((item) => [objectHash(item), item])).values()) activeEnvelopes.push({
      bytes: canonicalize(envelope), expected: { role: envelope.signed.role, product: root.product,
        component: head.component, scope: envelope.signed.scope } });
  }
  // revision zero holds an independently provisioned initial Root body, never
  // keys extracted from this command. Genesis accepts that exact body only.
  const successor = root.revision === 0
    ? verifyEnvelope(command.successorBytes, { trustedRoot: root.body,
      expected: { role: 'root', product: root.product, component: null, scope: {} }, now })
    : verifyRootSuccessor(command.successorBytes, { trustedRoot: root.body, product: root.product, now, activeEnvelopes });
  if (root.revision === 0) requireCondition(root.body.version === 1 && activeEnvelopes.length === 0, 'MODEL_GENESIS_INVALID');
  reads.push(...keyRetentionReads(records, command.keyUseIndexKey, root.product, successor.signed, now));
  reads.push(...rootLineageReads(records, command.successorLineageKey, successor.signed));
  const material = records[records[command.successorLineageKey].chainMaterialKey];
  requireCondition(material.rootEnvelopeSha256 === objectHash(successor)
    && material.previousRootBodySha256 === (root.revision === 0 ? null : objectHash(root.body)), 'MODEL_ROOT_LINEAGE_INVALID');
  if (root.revision === 0) requireCondition(records[command.successorLineageKey].anchorKey === command.anchorKey, 'MODEL_ROOT_LINEAGE_INVALID');
  else {
    reads.push(...rootLineageReads(records, root.rootLineageKey, root.body));
    requireCondition(records[command.successorLineageKey].anchorKey === records[root.rootLineageKey].anchorKey
      && records[command.successorLineageKey].anchorBodySha256 === records[root.rootLineageKey].anchorBodySha256, 'MODEL_ROOT_LINEAGE_INVALID');
  }
  // Existing committed heads already bind complete public references. Rotation
  // preserves every scoped verification key even when those heads have expired;
  // expiration must not deadlock Root recovery. This does not renew metadata or
  // grant eligibility: a later publication verifies its entire fresh chain.
  return commandPlan(records, command, now, reads, [{ key: command.rootKey,
    value: nextRecord(root, { published: true, body: successor.signed, envelopeSha256: objectHash(successor),
      rootLineageKey: command.successorLineageKey }) }],
  [{ kind: 'root-published', version: successor.signed.version, committedAt: now }], { version: successor.signed.version });
}

export function planEmergencyDeny(records, command, now) {
  const deny = denyRecord(records, command.denyKey); const inventory = readRecord(records, command.inventoryKey);
  const nextDeny = nextRecord(deny, { entries: [...deny.entries, ...command.denyEntries] });
  // Closed typed entries distinguish key/role deny from business-object deny.
  denyRecord({ nextDeny }, 'nextDeny');
  requireCondition(Array.isArray(command.denyEntries) && command.denyEntries.length > 0
    && new Set(nextDeny.entries.map(objectHash)).size === nextDeny.entries.length
    && sameJson([...command.affectedScopeKeys].sort(), [...inventory.affectedScopeKeys].sort()), 'MODEL_DENY_INVALID');
  const reads = [command.denyKey, command.inventoryKey]; const writes = [{ key: command.denyKey,
    value: nextDeny }];
  if (command.denyEntries.some((entry) => entry.subjectKind === 'signingKey')) reads.push(...keyApprovalReads(records,
    command, inventory.product, command.denyKey, deny.revision, signingKeyDenyBody(records, command), now));
  const pendingGenerations = [];
  for (const key of command.affectedScopeKeys) {
    const scope = readRecord(records, key); reads.push(key);
    pendingGenerations.push({ scopeKey: key, generation: scope.selectionGeneration + 1 });
    writes.push({ key, value: nextRecord(scope, { selectionGeneration: scope.selectionGeneration + 1, metadataSyncPending: true }) });
  }
  const oldJob = records[command.jobKey];
  if (oldJob !== undefined) reads.push(command.jobKey);
  const job = { revision: oldJob === undefined ? 1 : oldJob.revision + 1, state: 'pending',
    denyRevision: deny.revision + 1, pendingGenerations, pendingSince: oldJob?.pendingSince ?? now,
    product: inventory.product, component: inventory.component };
  writes.push({ key: command.jobKey, value: job });
  return commandPlan(records, command, now, reads, writes,
    [{ kind: 'deny-committed', jobKey: command.jobKey, committedAt: now }], { jobKey: command.jobKey });
}
export function planReconcileMetadata(records, command, now) {
  const job = readRecord(records, command.jobKey);
  requireCondition(job.state === 'pending' && job.revision === command.expectedJobRevision
    && command.publication.mode === 'reconcile'
    && sameJson(job.pendingGenerations.map((entry) => entry.scopeKey).sort(), [...command.publication.affectedScopeKeys].sort()),
  'MODEL_RECONCILIATION_CHANGED');
  for (const entry of job.pendingGenerations) requireCondition(records[entry.scopeKey].metadataSyncPending === true
    && records[entry.scopeKey].selectionGeneration === entry.generation, 'MODEL_RECONCILIATION_CHANGED');
  const parts = publicationParts(records, command.publication, now);
  requireCondition(parts.reads.includes(command.denyKey) && records[command.denyKey].revision === job.denyRevision,
    'MODEL_RECONCILIATION_CHANGED');
  const extraReads = [];
  const current = records[command.publication.headKey];
  for (const entry of job.pendingGenerations) {
    const scope = records[entry.scopeKey];
    const target = command.publication.bundle.members.find((item) => item.signed.role === 'target'
      && item.signed.scope.channel === scope.channel && item.signed.scope.targetKey === scope.targetKey).signed;
    const previous = current.bundle.members.find((item) => item.signed.role === 'target'
      && item.signed.scope.channel === scope.channel && item.signed.scope.targetKey === scope.targetKey).signed;
    requireCondition(sameJson(releaseBusinessReference(target.baselineRelease), releaseBusinessReference(previous.baselineRelease)),
      'MODEL_SELECTION_CHANGED');
    const old = scope.currentCandidateOpening ?? null; const next = parts.candidateOpenings.get(entry.scopeKey);
    requireCondition(old === null ? next === null : next !== null && sameJson(next, { ...old,
      leafSalt: next.leafSalt, releaseEnvelopeSha256: next.releaseEnvelopeSha256 }), 'MODEL_OPENING_INVALID');
    extraReads.push(...reconciliationPathReads(records, command.publication, scope, previous, target, now));
  }
  return commandPlan(records, command, now, [...new Set([...parts.reads, ...extraReads, command.jobKey])],
    [...parts.writes, { key: command.jobKey, value: nextRecord(job, { state: 'completed', completedAt: now }) }],
    parts.facts, { jobKey: command.jobKey, state: 'completed' });
}
