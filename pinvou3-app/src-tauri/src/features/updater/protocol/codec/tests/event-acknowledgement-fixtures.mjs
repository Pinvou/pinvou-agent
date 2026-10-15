// Genuine original credential templates; supplied contract fixtures, not trust.
import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
const root = pathToFileURL(process.argv[1]);
const {parseJson, canonicalize} = await import(new URL('./canonical-json.mjs', root));
const {assertApiContract} = await import(new URL('./api-registry.mjs', root));
const {sha256} = await import(new URL('./digests.mjs', root));
const {createFixtureSet, HASH, TARGET} = await import(new URL('./tests/fixtures.mjs', root));
const fixtures = createFixtureSet();
const roles = ['telemetry-event', 'preinstall-transaction-event', 'install-transaction-event',
  'activate-transaction-event', 'reconciliation-event', 'recovery-review-event'];
const cases = roles.map(role => {
  const operation = `events-${role}`;
  const credential = parseJson(fixtures.sign(fixtures.claims[role]));
  const claims = credential.signed;
  const facts = {factType: 'local-state'};
  const first = {eventId: 'event-A', sequence: 1,
    occurredAt: claims.sessionStartedAt ?? claims.transactionStartedAt ?? claims.taskStartedAt,
    lineageId: claims.telemetrySessionId ?? claims.transactionId ?? claims.task.taskId,
    executionPurpose: claims.purpose === 'telemetry' ? 'install' : claims.purpose,
    kind: claims.allowedEvents[0], fromState: 'test-from', toState: 'test-to',
    facts, factsSha256: sha256(canonicalize(facts)),
    targetIdentitySha256: claims.purpose === 'telemetry' ? null : HASH};
  const second = {...structuredClone(first), eventId: 'event-B', sequence: 2};
  const request = {protocolVersion: 1,
    scope: {product: 'pinvou', component: 'app', installId: 'install-1', installationScopeId: 'scope-1',
      channel: 'stable', channelRevision: 1, targetKey: TARGET},
    credential, events: [first, second, structuredClone(first)]};
  const response = {protocolVersion: 1, requestId: 'server-generated', acknowledgements: [
    {eventId: 'event-A', disposition: 'committed'}, {eventId: 'event-B', disposition: 'late-diagnostic'},
    {eventId: 'event-A', disposition: 'replayed'}]};
  assertApiContract(operation, 'request', request);
  assertApiContract(operation, 'response', response);
  assert.equal(request.credential.signed.role, role);
  return {operation, request, response};
});
console.log(JSON.stringify(cases));
