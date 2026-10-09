import { addShape, api, common, credential, literal, requestShape, responseShape, object, array,
  identifier, hash, integer, nullable } from './api-shapes.mjs';

export const OPERATIONS = [];
function operation(name, owner, path, request, response, purpose, errors, management = false) {
  addShape(`${name}-request`, request); addShape(`${name}-response`, response);
  OPERATIONS.push({ name, owner, path: '/v1/' + path, purpose, errors, management });
}
const decision = { decision: credential('decision') };
const prepare = { preparation: common('preparation'), freezeEpoch: nullable(common('positive')),
  backup: nullable(common('backup')), staged: nullable(common('staged')),
  preinstallSlot: nullable(object({ slotIdentity: identifier, slotRevision: common('positive'), state: literal('inactive') })) };
const commonErrors = ['CONTRACT_INVALID', 'CREDENTIAL_INVALID', 'REVISION_CONFLICT', 'QUALIFICATION_CHANGED',
  'METADATA_SYNC_PENDING', 'RATE_LIMITED', 'SERVICE_UNAVAILABLE'];
operation('check', 'T17', 'upgrade/check', requestShape({ requestNonce: identifier, facts: api('current-facts'),
  grayTargeting: api('gray-targeting'),
  executionPurpose: { enum: ['preinstall', 'install', 'activate'] },
  supportedIncrementalAlgorithms: array(identifier), supportedIncrementalFormats: array(identifier) }),
responseShape({ decision: credential('decision'), telemetryCredential: credential('telemetry-event') }), 'check',
['SCOPE_UNACTIVATED', ...commonErrors]);
operation('refresh', 'T17', 'upgrade/refresh', requestShape({ requestNonce: identifier, facts: api('current-facts'),
  grayTargeting: api('gray-targeting'),
  workflowId: identifier, expectedWorkflowRevision: integer, previousSessionId: identifier,
  immutableContextSha256: hash, ...decision }), responseShape({ decision: credential('decision'),
  telemetryCredential: credential('telemetry-event'), workflowRevision: integer, writableSessionId: identifier }),
'check', commonErrors);
operation('download-info', 'T18', 'upgrade/download-info', requestShape({ ...decision, downloadKey: identifier, packageId: identifier }),
responseShape({ credential: credential('download'), packageId: identifier, size: common('positive'), sha256: hash,
  url: { type: 'string', minLength: 1, maxLength: 8192, pattern: '^https://' }, expiresAt: integer,
  fileSourceClass: identifier }), 'download', commonErrors);
for (const purpose of ['preinstall', 'install', 'activate']) {
  operation(`validate-${purpose}`, 'T19', `upgrade/${purpose}/validate`, requestShape({ ...decision,
    validateKey: identifier, facts: api('current-facts'), ...prepare }),
  responseShape({ authorization: credential(`authorization-${purpose}`), authorizationRevision: integer }), purpose, commonErrors);
  const consume = { $ref: 'urn:pinvou:upgrade:v1:common#/$defs/consumeRequest' };
  operation(`consume-${purpose}`, 'T19', `upgrade/${purpose}/consume`, requestShape({ authorization: credential(`authorization-${purpose}`),
    authorizationRevision: integer, semanticRequest: consume, consumeRequestDigest: hash }),
  responseShape({ authorizationState: literal('consumed'), authorizationRevision: integer, transactionId: identifier,
    transactionCredential: credential(`${purpose}-transaction-event`) }), purpose,
  [...commonErrors, 'AUTHORIZATION_EXPIRED', 'AUTHORIZATION_CONFLICT', 'IDEMPOTENCY_CONFLICT']);
  operation(`cancel-${purpose}`, 'T19', `upgrade/${purpose}/cancel`, requestShape({ authorization: credential(`authorization-${purpose}`), cancelKey: identifier,
    authorizationJti: identifier, authorizationRevision: integer, reason: identifier }),
  responseShape({ authorizationState: { enum: ['cancelled', 'expired', 'consumed'] }, authorizationRevision: integer }), purpose,
  [...commonErrors, 'AUTHORIZATION_CONFLICT', 'IDEMPOTENCY_CONFLICT']);
  operation(`execution-review-${purpose}`, 'T19', `upgrade/${purpose}/execution-review`, requestShape({
    authorization: credential(`authorization-${purpose}`), transactionId: identifier, transactionRevision: integer,
    facts: api('current-facts'), verifiedObjectSha256: hash, ...prepare }),
  responseShape({ transactionId: identifier, transactionRevision: integer, reviewedAt: integer,
    currentReadSet: api('read-set'), verifiedObjectSha256: hash }), purpose, commonErrors);
}
operation('consume-status', 'T19', 'upgrade/consume-status', api('status-request'), api('status-response'), 'consumeStatus',
['CONTRACT_INVALID', 'UNKNOWN_CONSUME_OUTCOME', 'RATE_LIMITED', 'SERVICE_UNAVAILABLE']);
operation('helper-cleanup', 'T12', 'upgrade/helper-cleanup', requestShape({ cleanupKey: identifier,
  facts: api('current-facts'), preparation: common('preparation'),
  deletionSet: array(object({ identity: common('nativeIdentity'), protectedPathId: identifier }), 1) }),
responseShape({ credential: credential('helper-cleanup') }), 'cleanup', commonErrors);
operation('channel-change', 'T44', 'upgrade/channel', requestShape({ changeKey: identifier,
  expectedChannelRevision: common('positive'), newChannel: common('channel'), preparation: common('preparation') }),
responseShape({ channel: common('channel'), channelRevision: common('positive') }), 'channelChange',
[...commonErrors, 'PERMISSION_DENIED']);
for (const role of ['telemetry-event', 'preinstall-transaction-event', 'install-transaction-event',
  'activate-transaction-event', 'reconciliation-event', 'recovery-review-event']) {
  operation(`events-${role}`, 'T20', `upgrade/events/${role}`, requestShape({ credential: credential(role),
    events: array(object({ eventId: identifier, sequence: common('positive'), occurredAt: integer,
      lineageId: identifier, executionPurpose: role === 'telemetry-event' ? { enum: ['preinstall', 'install', 'activate'] }
        : literal(role === 'reconciliation-event' ? 'reconcile' : role === 'recovery-review-event' ? 'recoveryReview' : role.split('-')[0]),
      kind: identifier, fromState: identifier, toState: identifier,
      factsSha256: hash, facts: api('event-facts'), targetIdentitySha256: role === 'telemetry-event' ? nullable(hash) : hash }), 1) }),
  responseShape({ acknowledgements: array(object({ eventId: identifier, disposition: { enum: ['committed', 'replayed', 'late-diagnostic'] } }), 1) }),
  role, [...commonErrors, 'EVENT_KEY_DENIED', 'IDEMPOTENCY_CONFLICT', 'INVALID_TRANSITION']);
}
for (const [prefix, owner, purpose] of [['reconciliation', 'T22', 'reconcile'], ['recovery-review', 'T23', 'recoveryReview']]) {
  operation(`${prefix}-create`, owner, `admin/upgrade/${prefix}`, requestShape({ commandKey: identifier,
    originalTransactionId: identifier, originalConsumeRequestDigest: hash, target: api('target-content'),
    originalStage: common('originalStage'), windowStart: integer, windowEnd: integer, reason: identifier,
    expectedReadSet: api('read-set') }), responseShape({ task: common('taskBinding'),
    actionCredential: credential(`${prefix}-action`), eventCredential: credential(`${prefix}-event`) }),
  purpose, [...commonErrors, 'PERMISSION_DENIED', 'MFA_REQUIRED', 'APPROVAL_REQUIRED'], true);
  operation(`${prefix}-action`, owner, `upgrade/${prefix}/action`, requestShape({ actionKey: identifier,
    credential: credential(`${prefix}-action`), taskRevision: integer,
    evidenceSha256: hash, evidence: api('event-facts') }), responseShape({ taskId: identifier, taskRevision: integer,
    disposition: { enum: ['recorded', 'protection-retained', 'reconciled'] } }), purpose,
  [...commonErrors, 'PERMISSION_DENIED', 'IDEMPOTENCY_CONFLICT']);
}
