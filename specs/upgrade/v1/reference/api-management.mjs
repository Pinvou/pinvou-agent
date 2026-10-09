import { OPERATIONS } from './api-operations.mjs';
import { addShape, api, common, credential, literal, responseShape, object, array,
  identifier, hash, integer, nullable } from './api-shapes.mjs';

addShape('management-scope', object({ product: identifier, component: nullable(identifier),
  channel: nullable(common('channel')), targetKey: nullable(common('targetKey')) }));
addShape('approved-change', object({ changeId: identifier, revision: common('positive'), sha256: hash }));
addShape('stage-plan', object({ stages: array(object({ stageId: identifier,
  percentage: { type: 'integer', minimum: 1, maximum: 100 }, minimumObservationMs: common('positive'),
  minimumSamples: common('positive'), thresholds: object({ download: api('ratio'), verification: api('ratio'),
    installation: api('ratio'), health: api('ratio'), preinstallation: nullable(api('ratio')), activation: nullable(api('ratio')) }) }), 1),
  snIncludeSetRef: nullable(api('approved-change')), snExcludeSetRef: nullable(api('approved-change')),
  progressReportIntervalMs: common('positive'), lossIntervalMs: common('positive') }));
addShape('ratio', object({ numerator: common('positive'), denominator: common('positive') }));
addShape('deployment-draft', object({ releaseId: identifier, releaseRevision: common('positive'),
  releaseTargetId: identifier, releaseTargetRevision: common('positive'),
  upgradeType: { enum: ['normal', 'silent', 'forced'] }, releaseVisibleAt: integer,
  installNotBefore: integer, installNotAfter: nullable(integer),
  plan: api('stage-plan'), responsibilityIdentity: identifier }));
addShape('release-draft', object({ appVersion: common('version'),
  notes: object({ 'zh-CN': common('text'), en: common('text'), ja: common('text') }),
  targets: array(object({ targetKey: common('targetKey'), packageManifest: common('metadataReference'),
    minimumSourceVersion: common('version'), migrationMode: { enum: ['none', 'backwardCompatible', 'irreversible'] },
    backupPolicy: { enum: ['notRequired', 'required'] },
    activationModes: { ...array({ enum: ['directInstall', 'stagedRestart'] }, 1), uniqueItems: true },
    certificationRef: api('approved-change') }), 1) }));
addShape('artifact-upload', object({ artifactKind: { enum: ['composite-upload', 'full-package', 'native-installer', 'helper', 'launcher', 'sbom', 'provenance'] },
  artifact: common('byteIdentity'), targetKey: common('targetKey'), appVersion: common('version'),
  sourceRevision: { oneOf: [
    object({ algorithm: literal('git-sha1'), value: { type: 'string', pattern: '^[0-9a-f]{40}$' } }),
    object({ algorithm: literal('git-sha256'), value: { type: 'string', pattern: '^[0-9a-f]{64}$' } }),
  ] } }));

const managementRequest = (fields) => object({ protocolVersion: literal(1), scope: api('management-scope'),
  commandKey: identifier, expectedReadSet: api('read-set'), reason: identifier, ...fields });
for (const [kind, payload] of [['artifact', 'artifact-upload'], ['release', 'release-draft'], ['deployment', 'deployment-draft']]) {
  const name = `create-${kind}-draft`;
  addShape(`${name}-request`, managementRequest({ draft: api(payload) }));
  addShape(`${name}-response`, responseShape({ recordId: identifier, revision: integer, draftSha256: hash }));
  OPERATIONS.push({ name, owner: kind === 'artifact' ? 'T09' : kind === 'release' ? 'T11' : 'T14',
    path: `/v1/admin/upgrade/drafts/${kind}`, purpose: 'management', management: true,
    errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'MFA_REQUIRED', 'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'SERVICE_UNAVAILABLE'] });
}
// External intent names do not expose publication/lifecycle kernel dispatch.
// T05 authorizes the exact immutable change; the owning business module then
// composes all mandatory internal commands and current final guards.
export const MANAGEMENT_INTENTS = Object.freeze(['RotateRoot', 'DenySigningKey', 'QuarantineArtifact',
  'ApproveSupplyChain', 'RevokeSupplyChain', 'AssembleRelease', 'SubmitReleaseTarget', 'ApproveReleaseTarget',
  'RejectReleaseTarget', 'RevokeReleaseTarget', 'CloseRelease', 'CancelRelease', 'SubmitDeployment',
  'ScheduleDeployment', 'ActivateDeployment', 'PauseDeployment', 'ResumeDeployment', 'WithdrawDeployment',
  'SupersedeDeployment', 'PublishRegistryRevision', 'EnableBridgeEligibility', 'DisableBridgeEligibility',
  'ChangeSupportFloor', 'FreezeRolloutPlan', 'StartRollout', 'AbortRollout', 'ReleaseQualityFreeze']);
addShape('management-command-request', { oneOf: MANAGEMENT_INTENTS
  .map((name) => managementRequest({ command: literal(name), approvedChange: api('approved-change') })) });
addShape('management-command-response', responseShape({ resultId: identifier, committedAt: integer,
  resultingReadSet: api('read-set') }));
OPERATIONS.push({ name: 'management-command', owner: 'T06', path: '/v1/admin/upgrade/commands',
  purpose: 'management', management: true, errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'MFA_REQUIRED', 'APPROVAL_REQUIRED',
    'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'INVALID_TRANSITION', 'CANDIDATE_NOT_ABOVE_BASELINE', 'QUALITY_INCOMPLETE', 'OWNER_FENCED', 'SERVICE_UNAVAILABLE'] });

// Supporting commands own new revisions instead of mutating approved content.
addShape('review-change-request', managementRequest({ change: api('approved-change'),
  expectedChangeRevision: common('positive'), disposition: { enum: ['approve', 'reject'] } }));
addShape('review-change-response', responseShape({ change: api('approved-change'),
  approvalEvidenceId: identifier, disposition: { enum: ['approved', 'rejected', 'awaiting-second-review'] } }));
OPERATIONS.push({ name: 'review-change', owner: 'T05', path: '/v1/admin/upgrade/review-change',
  purpose: 'management', management: true, errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'MFA_REQUIRED', 'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'SERVICE_UNAVAILABLE'] });
addShape('synchronize-stable-request', managementRequest({ sourceReleaseId: identifier,
  selections: array(object({ sourceStableDeploymentId: identifier, sourceStableDeploymentRevision: common('positive'),
    targetKey: common('targetKey'), channel: { enum: ['beta', 'internal'] }, configuration: api('deployment-draft') }), 1) }));
addShape('synchronize-stable-response', responseShape({ batchId: identifier,
  drafts: array(object({ channel: { enum: ['beta', 'internal'] }, targetKey: common('targetKey'), deploymentId: identifier, rolloutId: identifier }), 1) }));
OPERATIONS.push({ name: 'synchronize-stable', owner: 'T16', path: '/v1/admin/upgrade/synchronize-stable',
  purpose: 'management', management: true, errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'MFA_REQUIRED', 'REVISION_CONFLICT',
    'IDEMPOTENCY_CONFLICT', 'CANDIDATE_NOT_ABOVE_BASELINE', 'SERVICE_UNAVAILABLE'] });

const changePayloads = {
  'supply-chain': object({ releaseTargetId: identifier, releaseTargetRevision: common('positive'), channel: common('channel'),
    scanSnapshot: api('approved-change'), effectiveExpiresAt: nullable(integer), exceptionRefs: array(api('approved-change')) }),
  'support-floor': object({ supportFloorVersion: common('version'), evidence: api('approved-change'), announcement: api('approved-change') }),
  'bridge-eligibility': object({ deploymentId: identifier, deploymentRevision: common('positive'),
    compatibilityEvidence: api('approved-change'), validFrom: integer, expiresAt: nullable(integer) }),
  'registry': object({ previousRegistryVersion: integer, stableFacts: common('stableFacts'),
    sourceState: { enum: ['selectable', 'forward-only', 'repair-only', 'revoked'] },
    forwardPolicy: nullable(api('approved-change')), transformRefs: array(api('approved-change')), certificationRef: api('approved-change') }),
  'root': object({ nextRoot: { $ref: 'urn:pinvou:upgrade:v1:metadata:root#/properties/signed' } }),
  'key-deny': object({ signingKeyIds: { ...array(hash, 1), uniqueItems: true }, roles: { ...array(api('trust-role'), 1), uniqueItems: true } }),
};
addShape('create-change-request', { oneOf: Object.entries(changePayloads).map(([kind, payload]) => managementRequest({ kind: literal(kind), payload })) });
addShape('create-change-response', responseShape({ change: api('approved-change'), state: literal('draft') }));
OPERATIONS.push({ name: 'create-change', owner: 'T05', path: '/v1/admin/upgrade/changes', purpose: 'management', management: true,
  errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'SERVICE_UNAVAILABLE'] });

addShape('upload-locator-request', managementRequest({ artifactDraftId: identifier, artifactRevision: common('positive'),
  artifact: common('byteIdentity') }));
addShape('upload-locator-response', responseShape({ uploadId: identifier, url: { type: 'string', maxLength: 8192, pattern: '^https://' },
  expiresAt: integer, artifact: common('byteIdentity') }));
OPERATIONS.push({ name: 'upload-locator', owner: 'T09', path: '/v1/admin/upgrade/upload-locator', purpose: 'management', management: true,
  errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'SERVICE_UNAVAILABLE'] });
addShape('complete-upload-request', managementRequest({ uploadId: identifier, artifactDraftId: identifier,
  artifactRevision: common('positive'), artifact: common('byteIdentity') }));
addShape('complete-upload-response', responseShape({ artifactId: identifier, revision: common('positive'), state: literal('validating') }));
OPERATIONS.push({ name: 'complete-upload', owner: 'T09', path: '/v1/admin/upgrade/complete-upload', purpose: 'management', management: true,
  errors: ['CONTRACT_INVALID', 'PERMISSION_DENIED', 'REVISION_CONFLICT', 'IDEMPOTENCY_CONFLICT', 'SERVICE_UNAVAILABLE'] });
