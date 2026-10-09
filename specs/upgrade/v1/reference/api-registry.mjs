import { readFile } from 'node:fs/promises';
import Ajv2020 from 'ajv/dist/2020.js';
import { schemas as trustSchemas } from './build-schemas.mjs';
import { API_SCHEMAS, API_BASE, OPERATIONS } from './build-api.mjs';
import { canonicalize, parseJson } from './canonical-json.mjs';
import { consumeRequestDigest, sha256 } from './digests.mjs';
import { requireCondition } from './errors.mjs';
import { checkSemantics, sameJson } from './semantics.mjs';

const ajv = new Ajv2020({ strict: true, allErrors: false, validateFormats: false });
for (const name of trustSchemas.keys()) ajv.addSchema(parseJson(await readFile(new URL(`../schemas/${name}.json`, import.meta.url))));
for (const name of API_SCHEMAS.keys()) ajv.addSchema(parseJson(await readFile(new URL(`../schemas/api/${name}.json`, import.meta.url))));

export function assertApiShape(name, value) {
  const validate = ajv.getSchema(API_BASE + name);
  requireCondition(validate !== undefined && validate(value), 'API_CONTRACT_INVALID');
  return value;
}

function embeddedCredentials(value, result = []) {
  if (value === null || typeof value !== 'object') return result;
  if (Object.hasOwn(value, 'signed') && Object.hasOwn(value, 'signatures')) { result.push(value); return result; }
  for (const child of Object.values(value)) embeddedCredentials(child, result);
  return result;
}

/** Structural and semantic contract validation grants no authentication or
 * eligibility. The service must separately verify signatures against its current
 * Root and obtain current qualifications from the server-owned read set.
 */
export function assertApiContract(operationName, direction, value) {
  const operation = OPERATIONS.find((item) => item.name === operationName);
  requireCondition(operation !== undefined && ['request', 'response'].includes(direction), 'API_OPERATION_UNKNOWN');
  // Reparse also applies T01's UTF-8, depth, string and aggregate input bounds.
  assertApiShape(`${operationName}-${direction}`, parseJson(canonicalize(value)));
  for (const envelope of embeddedCredentials(value)) {
    requireCondition(canonicalize(envelope).length <= 65_536, 'CREDENTIAL_TOO_LARGE');
    checkSemantics(envelope.signed, { now: Math.max(envelope.signed.iat, envelope.signed.nbf) });
    if (value.scope !== undefined && !operation.management) {
      const claims = envelope.signed;
      requireCondition(claims.product === value.scope.product && claims.component === value.scope.component
        && sameJson(claims.scope, { installationScopeId: value.scope.installationScopeId,
          channel: value.scope.channel, channelRevision: value.scope.channelRevision, targetKey: value.scope.targetKey })
        && claims.installId === value.scope.installId, 'API_SCOPE_MISMATCH');
    }
  }
  if (direction === 'request') {
    if (operationName === 'check') requireCondition(value.supportedIncrementalAlgorithms.length === 0
      && value.supportedIncrementalFormats.length === 0, 'INCREMENTAL_NOT_ENABLED');
    if (['consume-preinstall', 'consume-install', 'consume-activate'].includes(operationName)) requireCondition(value.semanticRequest.purpose === operation.purpose
      && consumeRequestDigest(value.semanticRequest) === value.consumeRequestDigest
      && value.semanticRequest.product === value.scope.product && value.semanticRequest.component === value.scope.component
      && value.semanticRequest.installationScopeId === value.scope.installationScopeId, 'API_CONSUME_MISMATCH');
    if (operationName === 'consume-status') {
      const bytes = Buffer.from(value.recoverySecret, 'base64url');
      requireCondition(bytes.length === 32 && bytes.toString('base64url') === value.recoverySecret, 'API_CONTRACT_INVALID');
    }
    if (operationName.startsWith('validate-')) {
      const preinstall = operation.purpose === 'preinstall';
      requireCondition(preinstall ? value.freezeEpoch === null && value.backup === null && value.staged === null
        && value.preinstallSlot !== null : value.freezeEpoch !== null && value.preinstallSlot === null
        && (operation.purpose === 'activate' ? value.staged !== null : value.staged === null), 'API_PURPOSE_MISMATCH');
    }
    if (operationName.startsWith('cancel-')) requireCondition(value.authorizationJti === value.authorization.signed.authorizationJti
      && value.authorization.signed.purpose === operation.purpose, 'API_PURPOSE_MISMATCH');
    if (operationName.startsWith('events-')) {
      const claims = value.credential.signed;
      for (const event of value.events) {
        const purpose = claims.purpose === 'telemetry' ? event.executionPurpose : claims.purpose;
        const lineageId = claims.telemetrySessionId ?? claims.transactionId ?? claims.task?.taskId;
        requireCondition(event.executionPurpose === purpose && event.lineageId === lineageId
          && event.factsSha256 === sha256(canonicalize(event.facts))
          && event.occurredAt >= (claims.sessionStartedAt ?? claims.transactionStartedAt ?? claims.taskStartedAt)
          && event.occurredAt < claims.eventNotAfter && claims.allowedEvents.includes(event.kind), 'API_EVENT_MISMATCH');
      }
    }
    if (['reconciliation-action', 'recovery-review-action'].includes(operationName)) requireCondition(
      value.taskRevision === value.credential.signed.task.revision
      && value.evidenceSha256 === sha256(canonicalize(value.evidence)), 'API_EVIDENCE_MISMATCH');
    if (operationName.startsWith('execution-review-')) requireCondition(
      value.transactionId === value.authorization.signed.transactionId
      && ['preparation', 'freezeEpoch', 'backup', 'staged', 'preinstallSlot'].every((field) => sameJson(value[field], value.authorization.signed[field])),
    'API_PURPOSE_MISMATCH');
    const configurations = operationName === 'create-deployment-draft' ? [value.draft]
      : operationName === 'synchronize-stable' ? value.selections.map((item) => item.configuration) : [];
    for (const configuration of configurations) requireCondition((configuration.installNotAfter === null
      || configuration.releaseVisibleAt < configuration.installNotAfter && configuration.installNotBefore < configuration.installNotAfter)
      && (configuration.upgradeType !== 'forced' || configuration.releaseVisibleAt <= configuration.installNotBefore), 'API_CONFIGURATION_INVALID');
  }
  if (direction === 'response') {
    if (value.authorization !== undefined) requireCondition(value.authorization.signed.purpose === operation.purpose, 'API_PURPOSE_MISMATCH');
    if (['check', 'refresh'].includes(operationName)) {
      const decision = value.decision.signed; const telemetry = value.telemetryCredential.signed;
      requireCondition(['product', 'component', 'installId', 'decisionId', 'decisionRevision', 'telemetrySessionId', 'selectionGeneration']
        .every((field) => sameJson(decision[field], telemetry[field])) && sameJson(decision.scope, telemetry.scope), 'API_RESPONSE_MISMATCH');
      if (operationName === 'refresh') requireCondition(value.writableSessionId === decision.telemetrySessionId, 'API_RESPONSE_MISMATCH');
    }
    if (operationName === 'download-info') {
      const claims = value.credential.signed;
      requireCondition(value.packageId === claims.packageId && value.size === claims.package.size && value.sha256 === claims.package.sha256
        && value.expiresAt <= claims.exp && value.expiresAt > claims.nbf, 'API_RESPONSE_MISMATCH');
    }
    if (operationName.startsWith('consume-') && operationName !== 'consume-status') requireCondition(
      value.transactionId === value.transactionCredential.signed.transactionId, 'API_RESPONSE_MISMATCH');
    if (['reconciliation-create', 'recovery-review-create'].includes(operationName)) requireCondition(
      sameJson(value.task, value.actionCredential.signed.task) && sameJson(value.task, value.eventCredential.signed.task)
      && sameJson(value.actionCredential.signed.scope, value.eventCredential.signed.scope)
      && ['product', 'component', 'installId'].every((field) => value.actionCredential.signed[field] === value.eventCredential.signed[field]),
    'API_RESPONSE_MISMATCH');
  }
  return value;
}
