import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { readRecord, objectHash, assertIdentity } from './inputs.mjs';
import { captureRecord } from './atomic.mjs';
import { sha256 } from '../digests.mjs';
import { approvalContext, approvalReads } from './entities.mjs';

export function rootChangeBody(records, command) {
  return { command: 'PublishRootChange', actorId: command.actorId, baseRoot: captureRecord(records, command.rootKey),
    initialAnchor: records[command.rootKey].revision === 0 ? captureRecord(records, command.anchorKey) : null,
    successorEnvelopeSha256: sha256(command.successorBytes) };
}
export function signingKeyDenyBody(records, command) {
  return { command: 'CommitEmergencyDeny', actorId: command.actorId, baseDeny: captureRecord(records, command.denyKey),
    denyEntries: command.denyEntries, affectedScopeKeys: command.affectedScopeKeys, jobKey: command.jobKey };
}
export function keyApprovalReads(records, command, product, objectKey, objectRevision, body, now) {
  assertIdentity(command.actorId);
  const context = approvalContext(body.command, { product, component: null }, objectKey, objectRevision + 1);
  const permission = readRecord(records, command.managementAuthorizationKey);
  assertApiShape('management-authorization', permission);
  const hash = objectHash(body);
  requireCondition(permission.state === 'authorized' && permission.actorId === command.actorId
    && permission.mfaVerified === true && permission.permissions.includes('key-management')
    && permission.authorizedAt <= now && now < permission.expiresAt
    && permission.bodySha256 === hash && sameJson(permission.context, context), 'MODEL_MANAGEMENT_AUTHORIZATION_INVALID');
  const reads = approvalReads(records, command.approvalKey, hash, 2, context);
  requireCondition(!records[command.approvalKey].reviewerIds.includes(permission.actorId), 'MODEL_APPROVAL_INVALID');
  return [command.managementAuthorizationKey, ...reads];
}
