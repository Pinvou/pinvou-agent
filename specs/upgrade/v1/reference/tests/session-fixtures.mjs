import { planCreateSession, hopIdentitySha256, downloadObservation, workflowContext } from '../models/workflow.mjs';
import { planEvent } from '../models/events.mjs';
import { applyAtomically } from '../models/atomic.mjs';
import { objectHash } from '../models/inputs.mjs';
import { selectionIdentitySha256 } from '../models/inputs.mjs';
import { planSessionTimeout } from '../models/lifecycle.mjs';
import { parseJson } from '../canonical-json.mjs';
import { createFixtureSet, NOW, HASH, TARGET } from './fixtures.mjs';
import { addQualification, operation, operationCommand, modelFacts } from './model-fixtures.mjs';

export function sessionFixture({ required = false, noUpdate = false, activate = false } = {}) {
  const set = createFixtureSet(); const decision = set.claims.decision;
  if (activate) { decision.update = structuredClone(set.claims['authorization-activate'].update); decision.telemetrySessionId = 'activate-session'; }
  if (required) { decision.update.backupPolicy = 'required'; decision.update.plans.backup = { planId: 'backup-1', revision: 1, sha256: HASH }; }
  if (noUpdate) { decision.updateAvailable = false; decision.update = null; decision.reason = 'no_higher_version'; }
  const records = { root: { revision: 1, published: true, body: set.metadata.root }, index: { revision: 1, installationScopeId: 'scope-1', memberKeys: [] },
    'contribution-head': { revision: 1, watermark: 0, windowGroups: {} }, operation: operation(), deny: { revision: 1, recordKind: 'deny', entries: [] } };
  const expectedContext = { role: 'decision', product: 'pinvou', component: 'app', scope: decision.scope };
  if (!noUpdate) addQualification(records, expectedContext, { owner: 'T17', claims: decision });
  if (activate) {
    const staged = set.claims['authorization-activate'].staged;
    records.workflow = { revision: 1, state: 'ongoing', workflowId: decision.update.workflowId, writableSessionId: 'old-session',
      immutableContext: workflowContext(decision), firstStartedAt: NOW - 1000, lastObservedAt: NOW - 1000, lossIntervalMs: 120_000 };
    records.staged = { revision: staged.stagedRevision, ...staged, state: 'waiting', channelRevision: 1, installationScopeId: 'scope-1',
      preinstallTransactionKey: 'original-preinstall', selectionIdentitySha256: selectionIdentitySha256(decision.update),
      observationWindowId: 'window-1', groupIdentity: HASH };
    records['original-preinstall'] = { revision: 1, state: 'staging_completed', executionPurpose: 'preinstall',
      installationScopeId: 'scope-1', channelRevision: 1, transactionId: staged.preinstallTransactionId,
      slotIdentity: staged.slotIdentity, finalInstallerSha256: HASH };
  }
  const command = { ...operationCommand, rootKey: 'root', decisionBytes: set.sign(decision), expectedContext,
    executionPurpose: activate ? 'activate' : 'install', stagedKey: 'staged',
    qualificationKey: 'qualification', sessionKey: 'session', workflowKey: 'workflow', scopeIndexKey: 'index',
    timeoutKey: 'session-timeout', diagnosticCursorKey: 'session-diagnostic', contributionHeadKey: 'contribution-head',
    preparationEpoch: 1, dataScopeSha256: HASH, observationWindowId: 'window-1', lossIntervalMs: 120_000 };
  const session = applyAtomically(records, planCreateSession(records, command, NOW), NOW).records;
  set.claims['telemetry-event'].telemetrySessionId = decision.telemetrySessionId;
  const scope = { product: 'pinvou', component: 'app', installId: 'install-1', installationScopeId: 'scope-1',
    channel: 'stable', channelRevision: 1, targetKey: TARGET };
  return { set, records: session, scope, decision };
}
export function emit(fixture, records, toState, facts, occurredAt, committedAt = occurredAt, sessionKey = 'session') {
  const session = records[sessionKey];
  const cursor = records[session.diagnosticCursorKey];
  const sequence = (cursor?.lastSequence ?? session.sequence) + 1;
  const event = { eventId: `${session.sessionId}-event-${sequence}`, sequence,
    lineageId: session.sessionId, executionPurpose: session.executionPurpose, occurredAt, kind: toState,
    fromState: cursor?.lastState ?? session.lastStateBeforeTerminal ?? session.state,
    toState, facts, factsSha256: objectHash(facts), targetIdentitySha256: session.finalInstallerSha256 };
  const claims = fixture.set.claims['telemetry-event']; claims.allowedEvents = [toState];
  const command = { rootKey: 'root', denyKey: 'deny', lineageKey: sessionKey, ledgerKey: `${event.eventId}-ledger`, outboxKey: `${event.eventId}-outbox`,
    credential: parseJson(fixture.set.sign(claims)), expectedContext: { role: claims.role, product: claims.product, component: claims.component, scope: claims.scope },
    scope: fixture.scope, event };
  return applyAtomically(records, planEvent(records, command, committedAt), committedAt).records;
}
