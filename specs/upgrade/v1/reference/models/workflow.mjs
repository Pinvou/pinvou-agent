import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { nextRecord } from './atomic.mjs';
import { qualificationReads, assertFields, objectHash, selectionIdentitySha256 } from './inputs.mjs';
import { scopeIndexWrite } from './scope-index.mjs';
import { verifyEnvelope } from '../signatures.mjs';
import { readRecord, assertInteger, assertIdentity } from './inputs.mjs';
import { commandPlan } from './operation.mjs';
import { contributionWrite } from './contributions.mjs';
import { currentRootBody } from './metadata-inputs.mjs';

export function workflowContext(claims) {
  const update = claims.update;
  requireCondition(update !== null, 'MODEL_NO_UPDATE');
  return { product: claims.product, component: claims.component, installationScopeId: claims.scope.installationScopeId,
    channel: claims.scope.channel, channelRevision: claims.scope.channelRevision, targetKey: claims.scope.targetKey,
    sourceProfileId: claims.sourceProfileId, currentVersion: claims.currentVersion,
    endpointId: update.endpointChain.deploymentId, hopId: update.hopChain.deploymentId,
    upgradeType: update.upgradeType, activationMode: update.activationMode,
    selectionIdentitySha256: selectionIdentitySha256(update),
    helper: structuredClone(update.helper), launcher: structuredClone(update.launcher),
    originalStage: structuredClone(update.originalStage),
    contentIdentity: { packageId: update.packageId, package: structuredClone(update.hopChain.package),
      finalInstaller: structuredClone(update.finalInstaller), targetVersion: update.targetVersion } };
}
export function hopIdentitySha256(claims) {
  return hopContextIdentitySha256(workflowContext(claims));
}
function hopContextIdentitySha256(context) {
  return objectHash({ hopId: context.hopId, upgradeType: context.upgradeType, activationMode: context.activationMode,
    contentIdentity: context.contentIdentity });
}

/** New execution rights always read the actual logical workflow, not just a
 * still-valid qualification snapshot or its static signed identity. Historical
 * recovery and already-consumed transaction convergence do not use this guard.
 */
export function workflowSessionReads(records, session) {
  const workflow = readRecord(records, session.workflowKey); const context = session.immutableContext;
  requireCondition(workflow.state === 'ongoing' && workflow.workflowId === session.workflowId
    && workflow.writableSessionId === session.sessionId && context != null && workflow.immutableContext != null
    && sameJson(workflow.immutableContext, context) && objectHash(context) === session.immutableContextSha256
    && context.installationScopeId === session.installationScopeId && context.channelRevision === session.channelRevision
    && context.upgradeType === session.upgradeType && hopContextIdentitySha256(context) === session.hopSha256
    && (session.upgradeType !== 'normal' || workflow.confirmedHopSha256 === session.hopSha256), 'MODEL_WORKFLOW_ENDED');
  return [session.workflowKey];
}

function sessionFromDecision(claims, command, now) {
  const immutableContext = workflowContext(claims);
  assertIdentity(command.timeoutKey); assertIdentity(command.diagnosticCursorKey);
  const session = { revision: 1, recordKind: 'session', installationScopeId: claims.scope.installationScopeId,
    sessionId: claims.telemetrySessionId, installId: claims.installId, workflowId: claims.update.workflowId, workflowKey: command.workflowKey,
    decisionId: claims.decisionId, decisionRevision: claims.decisionRevision,
    updateAvailable: true, state: 'decision_received', executionPurpose: command.executionPurpose, upgradeType: claims.update.upgradeType,
    channelRevision: claims.scope.channelRevision, startedAt: now, futureActions: 'writable', sequence: 0,
    immutableContext, immutableContextSha256: objectHash(immutableContext),
    hopSha256: hopIdentitySha256(claims), finalInstallerSha256: claims.update.finalInstaller.sha256,
    backupPolicy: claims.update.backupPolicy, preparationEpoch: command.preparationEpoch,
    freezeEpoch: null, dataScopeSha256: command.dataScopeSha256, authorizationKey: null, activeValidateKey: null,
    timeoutKey: command.timeoutKey, backupCleanupKey: null, contributionHeadKey: command.contributionHeadKey,
    downloadResumeBinding: null,
    observationWindowId: command.observationWindowId, groupIdentity: claims.update.originalStage.groupIdentity,
    diagnosticCursorKey: command.diagnosticCursorKey };
  assertFields(session, { preparationEpoch: 'positive', dataScopeSha256: 'hash' });
  return session;
}
function diagnosticReservation(key, sessionId, now) {
  return { key, value: { revision: 1, recordKind: 'diagnosticCursor', lineageId: sessionId,
    retainUntil: now + 7 * 86_400_000 + 120_000 } };
}

export function planCreateSession(records, command, now) {
  assertIdentity(command.timeoutKey); assertIdentity(command.diagnosticCursorKey);
  const claims = verifyEnvelope(command.decisionBytes, { trustedRoot: currentRootBody(records, command.rootKey),
    expected: command.expectedContext, now }).signed;
  requireCondition(claims.role === 'decision' && claims.iat === now
    && ['preinstall', 'install', 'activate'].includes(command.executionPurpose), 'MODEL_PURPOSE_INVALID');
  if (!claims.updateAvailable) return commandPlan(records, command, now, [command.rootKey, command.scopeIndexKey], [
    scopeIndexWrite(records, command.scopeIndexKey, claims.scope.installationScopeId, [command.sessionKey]),
    { key: command.sessionKey, value: { revision: 1, recordKind: 'session', updateAvailable: false, state: 'decision_received',
      sessionId: claims.telemetrySessionId, installId: claims.installId, installationScopeId: claims.scope.installationScopeId,
      decisionId: claims.decisionId, decisionRevision: claims.decisionRevision, executionPurpose: command.executionPurpose,
      channelRevision: claims.scope.channelRevision, startedAt: now, futureActions: 'writable', sequence: 0,
      finalInstallerSha256: null, workflowId: null, workflowKey: null, authorizationKey: null, activeValidateKey: null,
      timeoutKey: command.timeoutKey, backupCleanupKey: null, diagnosticCursorKey: command.diagnosticCursorKey } },
    { key: command.timeoutKey, value: { revision: 1, state: 'pending', sessionId: claims.telemetrySessionId, dueAt: now + 86_400_000 } },
    diagnosticReservation(command.diagnosticCursorKey, claims.telemetrySessionId, now),
  ], [], { sessionId: claims.telemetrySessionId, workflowId: null });
  requireCondition(claims.updateAvailable === true
    && ['preinstall', 'install', 'activate'].includes(command.executionPurpose)
    && (command.executionPurpose === 'install' ? claims.update.upgradeType !== 'silent' : claims.update.upgradeType === 'silent'),
  'MODEL_PURPOSE_INVALID');
  assertInteger(command.lossIntervalMs, 1);
  const immutableContext = workflowContext(claims);
  requireCondition(records[command.qualificationKey].observationWindowId === command.observationWindowId, 'MODEL_QUALITY_BINDING_INVALID');
  const session = sessionFromDecision(claims, command, now);
  const reads = [command.rootKey, command.scopeIndexKey, command.contributionHeadKey,
    ...qualificationReads(records, command.qualificationKey, now, command.expectedContext, 'T17', claims)];
  const workflow = records[command.workflowKey];
  if (command.executionPurpose === 'activate') {
    const staged = readRecord(records, command.stagedKey); const original = readRecord(records, staged.preinstallTransactionKey);
    reads.push(command.stagedKey, staged.preinstallTransactionKey, command.workflowKey);
    requireCondition(staged.state === 'waiting' && now < staged.stagedValidUntil && original.state === 'staging_completed'
      && original.executionPurpose === 'preinstall' && original.transactionId === staged.preinstallTransactionId
      && original.installationScopeId === session.installationScopeId && original.channelRevision === session.channelRevision
      && original.slotIdentity === staged.slotIdentity && original.finalInstallerSha256 === session.finalInstallerSha256
      && staged.channelRevision === session.channelRevision && staged.installationScopeId === session.installationScopeId
      && workflow?.state === 'ongoing' && workflow.workflowId === session.workflowId
      && sameJson(workflow.immutableContext, immutableContext)
      && staged.selectionIdentitySha256 === records[command.qualificationKey].selectionIdentitySha256
      && staged.observationWindowId === session.observationWindowId && staged.groupIdentity === session.groupIdentity,
    'MODEL_STAGED_INVALID');
    session.stagedRevision = staged.revision;
  } else requireCondition(workflow === undefined, 'MODEL_WORKFLOW_DUPLICATE');
  const writes = [contributionWrite(records, command.contributionHeadKey, { windowId: command.observationWindowId,
    groupIdentity: claims.update.originalStage.groupIdentity }), { key: command.sessionKey, value: session },
    { key: command.workflowKey, value: command.executionPurpose === 'activate'
      ? nextRecord(workflow, { writableSessionId: session.sessionId }) : { revision: 1, state: 'ongoing', workflowId: session.workflowId,
      writableSessionId: session.sessionId, immutableContext, firstStartedAt: now, lastObservedAt: now,
      lossIntervalMs: command.lossIntervalMs, confirmedHopSha256: null, observationWindowId: command.observationWindowId } },
    { key: command.timeoutKey, value: { revision: 1, state: 'pending', sessionId: session.sessionId, dueAt: now + 86_400_000 } },
    diagnosticReservation(command.diagnosticCursorKey, session.sessionId, now),
    scopeIndexWrite(records, command.scopeIndexKey, session.installationScopeId, [command.sessionKey])];
  return commandPlan(records, command, now, [...new Set(reads)], writes, [], { sessionId: session.sessionId, workflowId: session.workflowId });
}

/** Logical downloads have no total deadline. A new qualified session owns only
 * future actions; original stage/group/content and the old session remain intact.
 */
export function planDownloadHandoff(records, command, now) {
  requireCondition(!Object.hasOwn(command, 'newSession'), 'MODEL_RESUME_INVALID');
  const { workflowKey, oldSessionKey, newSessionKey, qualificationKey, scopeIndexKey, rootKey,
    decisionBytes, expectedContext, expectedRevision, preparationKey, timeoutKey, diagnosticCursorKey } = command;
  const workflow = readRecord(records, workflowKey); const previous = readRecord(records, oldSessionKey);
  const preparation = readRecord(records, preparationKey); const qualification = readRecord(records, qualificationKey);
  const decision = verifyEnvelope(decisionBytes, { trustedRoot: currentRootBody(records, rootKey), expected: expectedContext, now }).signed;
  requireCondition(decision.role === 'decision' && decision.updateAvailable === true && decision.iat === now
    && decision.update.workflowId === workflow.workflowId && decision.telemetrySessionId !== previous.sessionId
    && decision.installId === previous.installId && sameJson(workflowContext(decision), workflow.immutableContext)
    && (decision.update.upgradeType !== 'normal' || workflow.confirmedHopSha256 === previous.hopSha256), 'MODEL_RESUME_INVALID');
  requireCondition(workflow.state === 'ongoing' && workflow.revision === expectedRevision
    && workflow.writableSessionId === previous.sessionId, 'MODEL_RESUME_CONFLICT');
  const priorState = previous.state === 'expired' ? previous.lastStateBeforeTerminal : previous.state;
  requireCondition(previous.workflowId === workflow.workflowId && ['download_started', 'deferred'].includes(priorState)
    && previous.immutableContextSha256 === objectHash(workflow.immutableContext)
    && ['preinstall', 'install'].includes(previous.executionPurpose)
    && qualification.context.immutableContextSha256 === previous.immutableContextSha256
    && previous.installationScopeId === decision.scope.installationScopeId && previous.channelRevision === decision.scope.channelRevision
    && qualification.observationWindowId === previous.observationWindowId
    && previous.groupIdentity === decision.update.originalStage.groupIdentity
    && sameJson(qualification.contentIdentity, workflow.immutableContext.contentIdentity)
    && preparation.state === 'idle' && preparation.installationScopeId === previous.installationScopeId
    && preparation.freezeEpoch === null && preparation.backup === null && preparation.transactionId === null,
  'MODEL_RESUME_INVALID');
  const reads = qualificationReads(records, qualificationKey, now, qualification.context, 'T17', decision);
  const session = sessionFromDecision(decision, { workflowKey, executionPurpose: previous.executionPurpose,
    timeoutKey, diagnosticCursorKey, preparationEpoch: preparation.ownerEpoch, dataScopeSha256: preparation.dataScopeSha256,
    contributionHeadKey: previous.contributionHeadKey, observationWindowId: previous.observationWindowId }, now);
  // This protected registration, never event-supplied facts, grants the resume
  // edge. Both sides of the handoff record the same exact original intent.
  session.downloadResumeBinding = { workflowId: workflow.workflowId, fromSessionId: previous.sessionId,
    toSessionId: session.sessionId, hopSha256: session.hopSha256,
    immutableContextSha256: session.immutableContextSha256, confirmedHopSha256: workflow.confirmedHopSha256 ?? null,
    handedOffAt: now };
  requireCondition(!Object.hasOwn(records, newSessionKey) && !Object.hasOwn(records, timeoutKey)
    && !Object.hasOwn(records, diagnosticCursorKey) && timeoutKey !== diagnosticCursorKey
    && timeoutKey !== previous.timeoutKey && diagnosticCursorKey !== previous.diagnosticCursorKey, 'MODEL_SESSION_DUPLICATE');
  return commandPlan(records, command, now, [...new Set([workflowKey, oldSessionKey, scopeIndexKey, rootKey, preparationKey, ...reads])], [
    scopeIndexWrite(records, scopeIndexKey, session.installationScopeId, [newSessionKey]),
    { key: workflowKey, value: nextRecord(workflow, { writableSessionId: session.sessionId,
      downloadResumeBinding: structuredClone(session.downloadResumeBinding) }) },
    { key: oldSessionKey, value: nextRecord(previous, { futureActions: 'fenced', fencedAt: now }) },
    { key: newSessionKey, value: session },
    diagnosticReservation(diagnosticCursorKey, session.sessionId, now),
    { key: timeoutKey, value: { revision: 1, state: 'pending', sessionId: session.sessionId, dueAt: now + 86_400_000 } },
  ], [{ kind: 'download-session-handoff', workflowId: workflow.workflowId, committedAt: now,
    fromSessionId: previous.sessionId, toSessionId: session.sessionId }], { writableSessionId: session.sessionId });
}

export function downloadObservation(workflow, now) {
  requireCondition(Number.isSafeInteger(now) && now >= workflow.lastObservedAt
    && Number.isSafeInteger(workflow.lossIntervalMs) && workflow.lossIntervalMs > 0, 'MODEL_OBSERVATION_INVALID');
  if (workflow.state !== 'ongoing') return workflow.state;
  return now - workflow.lastObservedAt < workflow.lossIntervalMs ? 'incomplete' : 'outcome_unknown';
}
