import { requireCondition } from '../errors.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { sameJson } from '../semantics.mjs';
import { verifyPublicChain } from '../metadata-chain.mjs';
import { captureRecord, nextRecord } from './atomic.mjs';
import { commandPlan } from './operation.mjs';
import { readRecord, objectHash } from './inputs.mjs';
import { chainReads } from './entities.mjs';
import { publicationParts } from './publication.mjs';
import { pathReads } from './paths.mjs';
import { currentReleaseHash, deniedKeyOptions } from './metadata-inputs.mjs';

export function planScheduleFirst(records, command, now) {
  requireCondition(readRecord(records, command.deploymentKey).state === 'in_review', 'MODEL_FIRST_ACTIVATION_INVALID');
  return scheduleParts(records, command, now, false);
}
export function planRebuildFirst(records, command, now) {
  requireCondition(readRecord(records, command.deploymentKey).state === 'scheduled', 'MODEL_FIRST_ACTIVATION_INVALID');
  return scheduleParts(records, command, now, true);
}
function scheduleParts(records, command, now, rebuilding) {
  const scope = readRecord(records, command.scopeKey); const deployment = readRecord(records, command.deploymentKey);
  const root = readRecord(records, command.rootKey); const head = readRecord(records, command.headKey);
  requireCondition(scope.state === 'unactivated' && scope.baselineDeploymentKey === null && scope.selectionGeneration === 0
    && deployment.revision === command.expectedDeploymentRevision,
  'MODEL_FIRST_ACTIVATION_INVALID');
  const scheduled = rebuilding ? deployment : nextRecord(deployment, { state: 'scheduled' });
  const future = { ...records, [command.deploymentKey]: scheduled };
  const chain = chainReads(future, deployment.chain, now, { phase: 'schedule', scheduled: true, expectedScope: scope });
  const reads = [...new Set([command.scopeKey, command.deploymentKey, command.rootKey, command.headKey, command.inventoryKey, ...chain])];
  const inventory = readRecord(records, command.inventoryKey);
  reads.push(...inventory.scopeKeys, ...inventory.requiredEntityKeys);
  reads.push(...pathReads(future, command.pathKey, command.deploymentKey, command.scopeKey, now,
    { phase: 'schedule', scheduled: true, targetBundle: command.bundle }));
  publicationParts(future, { rootKey: command.rootKey, headKey: command.headKey, inventoryKey: command.inventoryKey,
    denyKey: command.denyKey, mode: 'selection', affectedScopeKeys: [command.scopeKey], baseRoot: captureRecord(records, command.rootKey),
    baseHead: captureRecord(records, command.headKey), readSet: [...new Set(reads)].map((key) => captureRecord(future, key)),
    bundle: command.bundle }, now);
  const verified = verifyPublicChain({ trustedRoot: root.body, product: head.product, component: head.component, now,
    timestampBytes: canonicalize(command.bundle.timestamp), snapshotBytes: canonicalize(command.bundle.snapshot),
    members: command.bundle.members.map(canonicalize), ...deniedKeyOptions(records, command.denyKey) });
  const expectedScopes = inventory.scopeKeys.filter((key) => records[key].state === 'active' || key === command.scopeKey)
    .map((key) => `${records[key].channel}:${records[key].targetKey}`).sort();
  requireCondition(sameJson(expectedScopes, verified.targets.map((target) => `${target.scope.channel}:${target.scope.targetKey}`).sort()),
    'MODEL_SNAPSHOT_INCOMPLETE');
  const target = verified.targets.find((item) => item.scope.channel === scope.channel && item.scope.targetKey === scope.targetKey);
  requireCondition(target !== undefined && target.selectionGeneration === 1
    && target.baselineRelease.envelopeSha256 === currentReleaseHash(records, deployment.releaseKey), 'MODEL_GENERATION_MISMATCH');
  const staged = { revision: 1, state: 'pending', scopeKey: command.scopeKey, deploymentKey: command.deploymentKey,
    expectedScheduledRevision: scheduled.revision, baseRoot: captureRecord(records, command.rootKey), baseHead: captureRecord(records, command.headKey),
    readSet: [...new Set(reads)].map((key) => captureRecord(future, key)), bundle: structuredClone(command.bundle),
    createdAt: now, expiresAt: now + 900_000, terminalAt: null };
  const writes = [{ key: command.stagedSetKey, value: staged }];
  if (rebuilding) {
    const old = readRecord(records, command.replacesStagedSetKey); reads.push(command.replacesStagedSetKey);
    requireCondition(command.stagedSetKey !== command.replacesStagedSetKey && old.state === 'pending'
      && old.revision === command.expectedStagedRevision && old.scopeKey === command.scopeKey
      && old.deploymentKey === command.deploymentKey && old.expectedScheduledRevision === deployment.revision,
    'MODEL_FIRST_ACTIVATION_INVALID');
    writes.push({ key: command.replacesStagedSetKey, value: nextRecord(old,
      { state: 'replaced', terminalAt: now, cleanupAt: now + 86_400_000 }) });
  } else writes.push({ key: command.deploymentKey, value: scheduled });
  return commandPlan(records, command, now, [...new Set(reads)], writes, [],
    { stagedSetKey: command.stagedSetKey, scheduledRevision: scheduled.revision });
}
export function planActivateFirst(records, command, now) {
  const scope = readRecord(records, command.scopeKey); const deployment = readRecord(records, command.deploymentKey);
  const staged = readRecord(records, command.stagedSetKey);
  requireCondition(scope.state === 'unactivated' && scope.baselineDeploymentKey === null && scope.selectionGeneration === 0
    && deployment.state === 'scheduled' && staged.state === 'pending' && deployment.revision === staged.expectedScheduledRevision
    && staged.scopeKey === command.scopeKey && staged.deploymentKey === command.deploymentKey
    && now >= staged.createdAt && now < staged.expiresAt
    && staged.expiresAt <= staged.createdAt + 900_000
    && sameJson(staged.baseRoot, captureRecord(records, command.publication.rootKey))
    && sameJson(staged.baseHead, captureRecord(records, command.publication.headKey))
    && sameJson(staged.bundle, command.publication.bundle), 'MODEL_FIRST_ACTIVATION_INVALID');
  for (const read of staged.readSet) requireCondition(sameJson(read, captureRecord(records, read.key)), 'MODEL_CAS_CONFLICT');
  const chain = chainReads(records, deployment.chain, now, { scheduled: true, expectedScope: scope });
  chain.push(...pathReads(records, command.pathKey, command.deploymentKey, command.scopeKey, now,
    { scheduled: true, targetBundle: command.publication.bundle }));
  const parts = publicationParts(records, command.publication, now);
  requireCondition(command.publication.mode === 'selection' && sameJson(command.publication.affectedScopeKeys, [command.scopeKey]),
    'MODEL_FIRST_ACTIVATION_INVALID');
  const scopeWrite = parts.writes.find((write) => write.key === command.scopeKey);
  Object.assign(scopeWrite.value, { state: 'active', baselineDeploymentKey: command.deploymentKey, runningRolloutKey: null });
  return commandPlan(records, command, now, [...new Set([command.scopeKey, command.deploymentKey, command.stagedSetKey,
    ...staged.readSet.map((read) => read.key), ...chain, ...parts.reads])], [...parts.writes,
    { key: command.deploymentKey, value: nextRecord(deployment, { state: 'active' }) },
    { key: command.stagedSetKey, value: nextRecord(staged, { state: 'committed', terminalAt: now, cleanupAt: now + 86_400_000 }) },
  ], parts.facts, { baselineDeploymentKey: command.deploymentKey });
}
export function stagedSetCleanupDue(staged, now) {
  const terminal = staged.terminalAt ?? staged.expiresAt;
  return now >= terminal + 86_400_000;
}
