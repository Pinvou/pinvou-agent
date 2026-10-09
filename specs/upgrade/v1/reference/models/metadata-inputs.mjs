import { requireCondition } from '../errors.mjs';
import { assertApiShape } from '../api-registry.mjs';
import { readRecord, objectHash } from './inputs.mjs';

/** T08 owns current signature envelopes and immediate deny. Business entities
 * hold a head reference; metadata renewal never changes their business revision.
 * releaseEnvelopes is a private server map, not the public Snapshot catalog.
 */
export function currentReleaseEnvelope(records, releaseKey) {
  const release = readRecord(records, releaseKey);
  const head = readRecord(records, release.metadataHeadKey);
  const envelope = head.releaseEnvelopes[release.releaseId];
  requireCondition(envelope?.signed.role === 'release' && envelope.signed.scope.releaseId === release.releaseId
    && envelope.signed.product === head.product && envelope.signed.component === head.component
    && envelope.signed.appVersion === release.appVersion && releaseBusinessHash(envelope.signed) === release.frozenBusinessSha256,
  'MODEL_MANIFEST_BINDING_INVALID');
  return envelope;
}
export function releaseBusinessHash({ revision, issuedAt, expiresAt, ...business }) { return objectHash(business); }
export function currentReleaseHash(records, releaseKey) { return objectHash(currentReleaseEnvelope(records, releaseKey)); }
export function currentRootBody(records, key) {
  const root = readRecord(records, key);
  requireCondition(root.published === true && root.revision > 0, 'MODEL_ROOT_NOT_PUBLISHED');
  return root.body;
}
export function denyRecord(records, key) {
  const deny = readRecord(records, key); assertApiShape('deny-record', deny); return deny;
}
export function deniedKeys(records, key, role) {
  return denyRecord(records, key).entries.filter((entry) => entry.subjectKind === 'signingKey' && entry.roles.includes(role))
    .map((entry) => entry.subjectId);
}
export function deniedKeyOptions(records, key) {
  const deny = denyRecord(records, key);
  const byRole = Object.create(null);
  for (const entry of deny.entries) if (entry.subjectKind === 'signingKey') {
    for (const role of entry.roles) (byRole[role] ??= []).push(entry.subjectId);
  }
  return { deniedKeyIdsByRole: byRole };
}
export function assertNotDenied(records, key, subjects) {
  const deny = denyRecord(records, key);
  requireCondition(subjects.every(([kind, id]) => !deny.entries.some((entry) => entry.subjectKind === kind && entry.subjectId === id)),
    'MODEL_SUBJECT_DENIED');
}
