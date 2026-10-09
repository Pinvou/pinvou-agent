import { requireCondition } from '../errors.mjs';
import { canonicalize } from '../canonical-json.mjs';
import { verifyEnvelope } from '../signatures.mjs';
import { assertOccurrence } from '../semantics.mjs';
import { sameJson } from '../semantics.mjs';
import { assertApiContract } from '../api-registry.mjs';
import { nextRecord, atomicPlan } from './atomic.mjs';
import { readRecord, objectHash, assertIdentity } from './inputs.mjs';
import { assertSessionTransition } from './session.mjs';
import { assertTransactionTransition, transactionTransition } from './transaction.mjs';
import { SESSION_TERMINALS, TRANSACTION_TERMINALS, assertEntityEdge } from './state-graphs.mjs';
import { contributionWrite } from './contributions.mjs';
import { deniedKeys, currentRootBody } from './metadata-inputs.mjs';

/** One-event atomic reference unit. The later T20 transport may batch these
 * units but must not acknowledge any uncommitted event. Deny wins over replay.
 */
export function planEvent(records, command, now) {
  const unverified = command.credential.signed;
  requireCondition(!deniedKeys(records, command.denyKey, unverified.role).includes(unverified.signingKeyId), 'EVENT_KEY_DENIED');
  const claims = verifyEnvelope(canonicalize(command.credential), { trustedRoot: currentRootBody(records, command.rootKey),
    expected: command.expectedContext, now }).signed;
  const { event } = command;
  assertApiContract(`events-${claims.role}`, 'request', { protocolVersion: 1, scope: command.scope,
    credential: command.credential, events: [event] });
  assertOccurrence(claims, event.occurredAt, now);
  const ledger = records[command.ledgerKey]; const lineage = readRecord(records, command.lineageKey);
  const digest = objectHash(event); const reads = [command.denyKey, command.rootKey, command.lineageKey];
  const telemetry = claims.role === 'telemetry-event'; const task = claims.task !== undefined;
  const expectedId = telemetry ? lineage.sessionId : task ? lineage.taskId : lineage.transactionId;
  requireCondition(event.lineageId === expectedId && event.executionPurpose === lineage.executionPurpose
    && claims.scope.installationScopeId === lineage.installationScopeId
    && claims.installId === lineage.installId
    && claims.scope.channelRevision === lineage.channelRevision
    && (telemetry ? claims.sessionStartedAt === lineage.startedAt
      && claims.decisionId === lineage.decisionId && claims.decisionRevision === lineage.decisionRevision
      : task ? sameJson(claims.task, lineage.binding) && claims.taskStartedAt === lineage.startedAt
        : claims.transactionStartedAt === lineage.startedAt && claims.authorizationJti === lineage.authorizationJti)
    && event.targetIdentitySha256 === lineage.finalInstallerSha256, 'MODEL_EVENT_BINDING_INVALID');
  if (ledger !== undefined) {
    reads.push(command.ledgerKey);
    requireCondition(ledger.eventId === event.eventId && ledger.eventDigest === digest
      && ledger.credentialJti === claims.jti, 'IDEMPOTENCY_CONFLICT');
    return atomicPlan(records, reads, [], [], now);
  }
  if (task) requireCondition(claims.task.taskId === lineage.taskId && claims.task.revision === lineage.taskRevision
    && event.kind === 'task_observation' && event.facts.factType === 'task-observation'
    && event.facts.windowId === lineage.windowId
    && event.facts.originalTargetIdentitySha256 === lineage.finalInstallerSha256, 'MODEL_TASK_BINDING_INVALID');
  else if (event.facts.factType === 'local-state') requireCondition(event.kind === event.toState, 'MODEL_EVENT_BINDING_INVALID');
  const terminal = telemetry ? SESSION_TERMINALS.includes(lineage.state) : task
    ? ['completed', 'failed', 'cancelled'].includes(lineage.state) : TRANSACTION_TERMINALS.includes(lineage.state);
  if (telemetry && lineage.fencedAt !== undefined) requireCondition(event.occurredAt < lineage.fencedAt, 'MODEL_EVENT_FENCED');
  let late = terminal || telemetry && lineage.futureActions === 'fenced'
    || now >= (telemetry ? lineage.startedAt + 86_400_000 : task ? lineage.windowEnd : lineage.startedAt + 30 * 86_400_000);
  let workflow = null;
  if (telemetry && lineage.updateAvailable !== false) {
    workflow = readRecord(records, lineage.workflowKey); reads.push(lineage.workflowKey);
    requireCondition(workflow.workflowId === lineage.workflowId, 'MODEL_EVENT_BINDING_INVALID');
    if (workflow.state !== 'ongoing') {
      // The confirmation cutoff is the server commit instant, distinct from
      // the original failure occurrence. Lawful already-occurred observations
      // remain diagnostic even if this Session was not independently fenced.
      requireCondition(Number.isSafeInteger(workflow.endedAt) && workflow.endedAt <= now && event.occurredAt < workflow.endedAt,
        'MODEL_WORKFLOW_ENDED');
      late = true;
    }
    if (!late) requireCondition(workflow.state === 'ongoing' && workflow.writableSessionId === lineage.sessionId,
      'MODEL_WORKFLOW_ENDED');
    if (event.toState === 'download_resume_context') requireCondition(lineage.downloadResumeBinding != null
      && sameJson(workflow.immutableContext, lineage.immutableContext)
      && (late || workflow.downloadResumeBinding != null && sameJson(workflow.downloadResumeBinding, lineage.downloadResumeBinding))
      && (lineage.upgradeType !== 'normal' || workflow.confirmedHopSha256 === lineage.hopSha256), 'MODEL_RESUME_INVALID');
  }
  const writes = []; let updated = null;
  if (late) {
    const cursorKey = assertIdentity(lineage.diagnosticCursorKey); const cursor = records[cursorKey];
    requireCondition(cursor?.lineageId === undefined || cursor.lineageId === event.lineageId, 'MODEL_EVENT_BINDING_INVALID');
    if (cursor !== undefined) reads.push(cursorKey);
    const reportedState = cursor?.lastState ?? (terminal ? lineage.lastStateBeforeTerminal : lineage.state);
    const reportedSequence = cursor?.lastSequence ?? lineage.sequence;
    requireCondition(reportedState !== undefined && event.sequence === reportedSequence + 1
      && event.fromState === reportedState, 'MODEL_EVENT_SEQUENCE_INVALID');
    const historical = { ...structuredClone(lineage), state: reportedState,
      freezeEpoch: cursor?.freezeEpoch ?? lineage.freezeEpoch,
      healthStartedAt: cursor?.healthStartedAt ?? lineage.healthStartedAt };
    let boundaryAt = cursor?.boundaryAt ?? lineage.boundaryAt ?? null;
    if (telemetry) {
      if (event.facts.factType === 'download-progress') requireCondition(reportedState === 'download_started'
        && event.toState === reportedState && event.kind === 'download_progress'
        && event.facts.updatedAt === event.occurredAt && event.facts.bytesReceived <= event.facts.totalBytes
        && event.facts.bytesReceived >= (cursor?.bytesReceived ?? lineage.bytesReceived ?? 0),
      'MODEL_PROGRESS_INVALID');
      else assertSessionTransition(historical, event.toState, { now: event.occurredAt, actor: 'client', facts: event.facts });
    } else if (task) {
      requireCondition(event.toState === 'observing' && event.facts.factType === 'task-observation', 'MODEL_TASK_SERVER_ONLY');
      if (reportedState !== 'observing') assertEntityEdge('task', reportedState, event.toState);
    } else {
      historical.boundaryAt = boundaryAt;
      const initial = reportedSequence === 0 && reportedState === 'authorization_consumed'
        && event.kind === 'authorization_consumed' && event.toState === reportedState;
      if (!initial) {
        assertTransactionTransition(historical, event.toState, { now: event.occurredAt, facts: event.facts });
        if (['installer_started', 'activation_started'].includes(event.toState)) boundaryAt = event.occurredAt;
      }
    }
    writes.push({ key: cursorKey, value: { revision: cursor === undefined ? 1 : cursor.revision + 1,
      recordKind: 'diagnosticCursor', lineageId: event.lineageId,
      lastState: event.toState, lastSequence: event.sequence, boundaryAt,
      freezeEpoch: event.toState === 'writers_frozen' ? event.facts.freezeEpoch : (historical.freezeEpoch ?? null),
      healthStartedAt: event.toState === 'health_check_started' ? event.facts.healthStartedAt : (historical.healthStartedAt ?? null),
      bytesReceived: event.facts.factType === 'download-progress' ? event.facts.bytesReceived : (cursor?.bytesReceived ?? lineage.bytesReceived ?? 0),
      retainUntil: claims.exp + 120_000 } });
  }
  if (!late) {
    requireCondition(event.sequence === lineage.sequence + 1 && event.fromState === lineage.state, 'MODEL_EVENT_SEQUENCE_INVALID');
    if (event.facts.factType === 'download-progress') {
      requireCondition(telemetry && lineage.state === 'download_started' && event.toState === lineage.state
        && event.kind === 'download_progress' && event.facts.updatedAt === event.occurredAt
        && event.facts.bytesReceived <= event.facts.totalBytes
        && event.facts.bytesReceived >= (lineage.bytesReceived ?? 0), 'MODEL_PROGRESS_INVALID');
      updated = nextRecord(lineage, { sequence: event.sequence, bytesReceived: event.facts.bytesReceived,
        lastObservedAt: event.occurredAt, downloadObservation: event.facts.observation });
    } else if (telemetry) {
      assertSessionTransition(lineage, event.toState, { now: event.occurredAt, actor: 'client', facts: event.facts });
      updated = nextRecord(lineage, { state: event.toState, sequence: event.sequence,
        ...(event.toState === 'writers_frozen' ? { freezeEpoch: event.facts.freezeEpoch } : {}) });
      if (event.toState === 'user_confirmed') {
        requireCondition(workflow.workflowId === lineage.workflowId && workflow.state === 'ongoing'
          && workflow.writableSessionId === lineage.sessionId, 'MODEL_CONFIRMATION_REQUIRED');
        writes.push({ key: lineage.workflowKey, value: nextRecord(workflow, { confirmedHopSha256: event.facts.confirmedHopSha256 }) });
      }
    } else if (task) {
      requireCondition(claims.task.taskId === lineage.taskId && claims.task.revision === lineage.taskRevision
        && event.facts.factType === 'task-observation' && event.facts.windowId === lineage.windowId, 'MODEL_TASK_BINDING_INVALID');
      requireCondition(event.toState === 'observing', 'MODEL_TASK_SERVER_ONLY');
      if (lineage.state !== 'observing') assertEntityEdge('task', lineage.state, event.toState);
      updated = nextRecord(lineage, { state: event.toState, sequence: event.sequence });
    } else {
      const initial = lineage.sequence === 0 && event.sequence === 1 && event.kind === 'authorization_consumed'
        && event.fromState === 'authorization_consumed' && event.toState === event.fromState;
      if (initial) updated = nextRecord(lineage, { sequence: 1 });
      else updated = { ...transactionTransition(lineage, event.toState, { now: event.occurredAt, facts: event.facts }), sequence: event.sequence };
    }
    writes.push({ key: command.lineageKey, value: updated });
  }
  const disposition = late ? 'late-diagnostic' : 'committed';
  // An accepted historical failure preserves the old Session, but is still a
  // confirmed failure of this logical download. Commit the latch with its
  // ledger/outbox so a concurrent handoff or new-session action loses the CAS.
  if (telemetry && event.toState === 'download_failed' && workflow.state === 'ongoing') writes.push({ key: lineage.workflowKey,
    value: nextRecord(workflow, { state: 'failed', failedAt: event.occurredAt, endedAt: now,
      failureEventId: event.eventId, failureSessionId: lineage.sessionId }) });
  if (telemetry && event.facts.factType === 'download-progress') {
    requireCondition(workflow.workflowId === lineage.workflowId, 'MODEL_EVENT_BINDING_INVALID');
    if (!late && workflow.state === 'ongoing' && workflow.writableSessionId === lineage.sessionId) writes.push({ key: lineage.workflowKey,
      value: nextRecord(workflow, { lastObservedAt: Math.max(workflow.lastObservedAt, event.occurredAt) }) });
  }
  const contribution = lineage.updateAvailable === false ? null : contributionWrite(records, lineage.contributionHeadKey,
    { windowId: lineage.observationWindowId, groupIdentity: lineage.groupIdentity });
  if (contribution !== null) { reads.push(lineage.contributionHeadKey); writes.push(contribution); }
  writes.push({ key: command.ledgerKey, value: { revision: 1, eventId: event.eventId, eventDigest: digest,
    credentialJti: claims.jti, retainUntil: claims.exp + 120_000, disposition, committedAt: now } });
  writes.push({ key: command.outboxKey, value: { revision: 1, lineageId: event.lineageId,
    event: structuredClone(event), contributionWatermark: contribution?.value.watermark ?? null, disposition, committedAt: now } });
  return atomicPlan(records, [...new Set(reads)], writes, [{ kind: disposition, eventId: event.eventId, committedAt: now }], now);
}
