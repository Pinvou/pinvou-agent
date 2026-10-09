import { NOW, HASH, createFixtureSet } from './fixtures.mjs';
import { canonicalize, parseJson } from '../canonical-json.mjs';
import { sha256, consumeRequestDigest } from '../digests.mjs';
import { captureRecord } from '../models/atomic.mjs';
import { QUALIFICATION_KINDS, objectHash, selectionIdentitySha256 } from '../models/inputs.mjs';
import { workflowContext, hopIdentitySha256 } from '../models/workflow.mjs';

export function modelSession(state = 'authorized', purpose = 'install') {
  return { revision: 1, recordKind: 'session', sessionId: 'session-1', installId: 'install-1',
    decisionId: 'decision-1', decisionRevision: 1,
    installationScopeId: 'scope-1', workflowId: 'workflow-1', state, executionPurpose: purpose,
    upgradeType: purpose === 'install' ? 'normal' : 'silent', startedAt: NOW, hopSha256: HASH,
    finalInstallerSha256: HASH, dataScopeSha256: HASH, preparationEpoch: 1, freezeEpoch: purpose === 'preinstall' ? null : 1,
    backupPolicy: 'notRequired', futureActions: 'writable', stagedRevision: 1,
    activeValidateKey: null, authorizationKey: 'authorization', timeoutKey: 'session-timeout', backupCleanupKey: null,
    channelRevision: 1, sequence: 0, contributionHeadKey: 'contribution-head', observationWindowId: 'window-1', groupIdentity: HASH,
    diagnosticCursorKey: 'session-diagnostic' };
}
export function modelFacts() {
  return { factType: 'local-state', confirmedHopSha256: HASH, verifiedObjectSha256: HASH, verificationResult: 'verified',
    preflightResult: 'supported', targetScopeId: 'scope-1', permissionScopeId: 'scope-1', permissionSource: 'existing-helper',
    workflowId: 'workflow-1', writableSessionId: 'session-1', hopSha256: HASH, trigger: 'restart',
    preinstallState: 'staging_completed', stagedRevision: 1, stagedValidUntil: NOW + 100_000,
    ownerEpoch: 1, freezeEpoch: 1, writerCount: 0, frozenDataScopeSha256: HASH, backupFreezeEpoch: 1,
    backupPreparationEpoch: 1, backupDataScopeSha256: HASH, stoppedOwnerEpoch: 1, activeWriterCount: 0,
    allExecutorsStopped: true, isolatedSlotId: 'slot-1', verifiedSlotId: 'slot-1', activeMutationCount: 0,
    oldIdentitySha256: HASH, oldDataSha256: HASH, boundaryAt: null, safetyResult: 'intact',
    lastOnlineAuthorizationJti: 'jti-1', lastOnlineAt: NOW + 1, installerInvokedAt: NOW + 1,
    firstActiveWriteAt: NOW + 1, pointerSwitchedAt: NOW + 1, activeObjectSha256: HASH,
    dataScopeSha256: HASH, healthResult: 'passed', dataResult: 'consistent', healthStartedAt: NOW,
    healthAttempts: 1, lastHealthAttemptMs: 1 };
}
export function modelTransaction(state = 'authorization_consumed', purpose = 'install') {
  return { revision: 1, recordKind: 'transaction', transactionId: 'transaction-1', authorizationJti: 'jti-1',
    installId: 'install-1',
    installationScopeId: 'scope-1', executionPurpose: purpose, state, startedAt: NOW, ownerEpoch: 1,
    helperPlanRef: null, finalInstallerSha256: HASH, dataScopeSha256: HASH, oldIdentitySha256: HASH,
    oldDataSha256: HASH, boundaryAt: ['installer_started', 'activation_started', 'reconciling', 'installation_verified', 'health_check_started'].includes(state) ? NOW : null,
    healthStartedAt: state === 'health_check_started' ? NOW : null,
    slotIdentity: purpose === 'install' ? null : 'slot-1', channelRevision: 1, sequence: 0, contributionHeadKey: 'contribution-head',
    observationWindowId: 'window-1', groupIdentity: HASH, diagnosticCursorKey: 'transaction-diagnostic' };
}
export function operation(now = NOW) {
  return { revision: 1, state: 'processing', requestDigest: HASH, ownerEpoch: 1,
    ownerStartedAt: now, lastRenewedAt: now, leaseExpiresAt: now + 30_000 };
}
export const operationCommand = { operationKey: 'operation', ownerEpoch: 1, requestDigest: HASH, resultId: 'result-1' };

export function addQualification(records, context, { key = 'qualification', owner = 'T19', now = NOW,
  endpointKind = 'baseline', hopKind = 'ordinary', extraKeys = [], claims = null } = {}) {
  const kinds = [...QUALIFICATION_KINDS, ...(endpointKind === 'candidate' ? ['rollout'] : []),
    ...(hopKind === 'bridge' ? ['bridgeEligibility'] : [])];
  const readSet = kinds.map((recordKind) => {
    const recordKey = `qualified-${recordKind}`;
    records[recordKey] = { revision: 1, recordKind, state: 'current', bindingSha256: HASH };
    return { ...captureRecord(records, recordKey), recordKind };
  });
  for (const recordKey of extraKeys) readSet.push({ ...captureRecord(records, recordKey), recordKind: records[recordKey].recordKind });
  records[key] = { revision: 1, projectionOwner: owner, state: 'qualified', context: structuredClone(context),
    qualifiedClaimsSha256: claims === null ? HASH : objectHash(claims),
    selectionIdentitySha256: claims === null ? HASH : selectionIdentitySha256(claims.update),
    observationWindowId: 'window-1',
    qualifiedAt: now, expiresAt: now + 300_000, endpointKind, hopKind, ordinaryPathRequired: false, readSet };
  return key;
}
export function consumeFixture(purpose = 'install') {
  const set = createFixtureSet(); const claims = set.claims[`authorization-${purpose}`];
  const authorizationBytes = set.sign(claims); const context = { role: claims.role, product: claims.product,
    component: claims.component, scope: structuredClone(claims.scope) };
  const request = { ...structuredClone(set.consume), purpose, authorizationEnvelopeSha256: sha256(authorizationBytes),
    freezeEpoch: claims.freezeEpoch, backup: claims.backup, staged: claims.staged,
    preinstallSlot: claims.preinstallSlot, plans: claims.update.plans };
  request.consumeRequestDigest = consumeRequestDigest(request);
  const session = modelSession('authorized', purpose);
  session.immutableContext = workflowContext(claims); session.hopSha256 = hopIdentitySha256(claims);
  session.immutableContextSha256 = objectHash(session.immutableContext); session.workflowKey = 'workflow';
  const records = { session, root: { revision: 1, published: true, body: set.metadata.root }, operation: operation(),
    workflow: { revision: 1, state: 'ongoing', workflowId: session.workflowId, writableSessionId: session.sessionId,
      immutableContext: structuredClone(session.immutableContext), confirmedHopSha256: purpose === 'install' ? session.hopSha256 : null },
    'contribution-head': { revision: 1, watermark: 0, windowGroups: {} },
    authorization: { revision: 1, recordKind: 'authorization', installationScopeId: 'scope-1', state: 'available',
      authorizationJti: claims.authorizationJti, sessionId: session.sessionId, sessionKey: 'session', expiresAt: claims.exp,
      envelopeSha256: sha256(authorizationBytes), context, claims, consumeReservationKey: 'reservation' },
    reservation: { revision: 1, request, authorizationJti: claims.authorizationJti,
      transactionKey: 'transaction', outcomeKey: 'outcome', timeoutKey: 'transaction-timeout', diagnosticCursorKey: 'transaction-diagnostic' },
    preparation: { revision: 1, state: 'preparing', ownerEpoch: 1, scopeRevision: 1, installationScopeId: 'scope-1',
      freezeEpoch: claims.freezeEpoch, backup: claims.backup, dataScopeSha256: HASH, oldIdentitySha256: HASH, oldDataSha256: HASH,
      waitingStagedKey: null },
    'session-timeout': { revision: 1, state: 'pending', sessionId: session.sessionId, dueAt: NOW + 86_400_000 },
    index: { revision: 1, installationScopeId: 'scope-1', memberKeys: ['session', 'authorization'] } };
  if (purpose === 'activate') {
    records.staged = { revision: 1, recordKind: 'staged', installationScopeId: 'scope-1', state: 'waiting', channelRevision: 1,
      ...claims.staged, preinstallState: 'staging_completed', preinstallTransactionKey: 'original-preinstall',
      selectionIdentitySha256: selectionIdentitySha256(claims.update), observationWindowId: 'window-1', groupIdentity: HASH };
    records['original-preinstall'] = { revision: 1, recordKind: 'transaction', installationScopeId: 'scope-1',
      channelRevision: 1,
      transactionId: claims.staged.preinstallTransactionId, executionPurpose: 'preinstall', state: 'staging_completed',
      slotIdentity: claims.staged.slotIdentity, finalInstallerSha256: claims.update.finalInstaller.sha256 };
    records.index.memberKeys.push('staged', 'original-preinstall');
    records.preparation.waitingStagedKey = 'staged';
  }
  addQualification(records, context, { claims });
  const command = { ...operationCommand, authorizationKey: 'authorization', sessionKey: 'session', transactionKey: 'transaction',
    preparationKey: 'preparation', qualificationKey: 'qualification', rootKey: 'root', stagedKey: purpose === 'activate' ? 'staged' : null,
    scopeIndexKey: 'index', expectedAuthorizationRevision: 1, request, authorizationBytes };
  return { set, records, command, claims, context };
}
