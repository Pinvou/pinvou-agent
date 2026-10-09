// T02 wire shapes reuse T01's trust/byte vocabulary by reference.
import { schemas as trustSchemas } from './build-schemas.mjs';
export const API_BASE = 'urn:pinvou:upgrade:v1:api:';
export const common = (name) => ({ $ref: `urn:pinvou:upgrade:v1:common#/$defs/${name}` });
export const api = (name) => ({ $ref: API_BASE + name });
export const credential = (role) => ({ $ref: `urn:pinvou:upgrade:v1:credentials:${role}` });
export const integer = common('integer');
export const identifier = common('identifier');
export const hash = common('hash');
export const nullable = (schema) => ({ anyOf: [schema, { type: 'null' }] });
export const array = (items, minItems = 0) => ({ type: 'array', items, minItems, maxItems: 4096 });
export const object = (properties, optional = []) => ({ type: 'object', properties,
  required: Object.keys(properties).filter((key) => !optional.includes(key)), additionalProperties: false });
export const literal = (value) => ({ const: value });

export const ERROR_DEFINITIONS = Object.freeze({
  SCOPE_UNACTIVATED: { status: 409, retryable: true },
  CONTRACT_INVALID: { status: 400, retryable: false },
  CREDENTIAL_INVALID: { status: 401, retryable: false },
  EVENT_KEY_DENIED: { status: 401, retryable: false },
  AUTHORIZATION_EXPIRED: { status: 409, retryable: false },
  AUTHORIZATION_CONFLICT: { status: 409, retryable: false },
  IDEMPOTENCY_CONFLICT: { status: 409, retryable: false },
  REVISION_CONFLICT: { status: 409, retryable: true },
  QUALIFICATION_CHANGED: { status: 409, retryable: true },
  METADATA_SYNC_PENDING: { status: 409, retryable: true },
  UNKNOWN_CONSUME_OUTCOME: { status: 404, retryable: false },
  RATE_LIMITED: { status: 429, retryable: true },
  SERVICE_UNAVAILABLE: { status: 503, retryable: true },
  PERMISSION_DENIED: { status: 403, retryable: false },
  MFA_REQUIRED: { status: 403, retryable: false },
  APPROVAL_REQUIRED: { status: 409, retryable: false },
  INVALID_TRANSITION: { status: 409, retryable: false },
  CANDIDATE_NOT_ABOVE_BASELINE: { status: 409, retryable: false },
  QUALITY_INCOMPLETE: { status: 409, retryable: true },
  OWNER_FENCED: { status: 409, retryable: true },
  UNSUPPORTED: { status: 422, retryable: false },
});

export const API_SCHEMAS = new Map();
export function addShape(name, schema) {
  API_SCHEMAS.set(name, { $schema: 'https://json-schema.org/draft/2020-12/schema', $id: API_BASE + name, ...schema });
}
addShape('trust-role', { enum: trustSchemas.get('common').$defs.rolePolicy.properties.role.enum });

addShape('error', { oneOf: Object.entries(ERROR_DEFINITIONS).filter(([code]) => code !== 'SCOPE_UNACTIVATED').map(([code, definition]) => object({
  protocolVersion: literal(1), requestId: identifier, code: literal(code), retryable: literal(definition.retryable),
  ...(definition.retryable ? { retryAfterMs: { type: 'integer', minimum: 1, maximum: 300_000 } } : {}),
}, definition.retryable ? ['retryAfterMs'] : [])) });
addShape('unactivated', object({ protocolVersion: literal(1), requestId: identifier,
  code: literal('SCOPE_UNACTIVATED'), retryable: literal(true), checkAfterMs: { type: 'integer', minimum: 1, maximum: 86_400_000 } }));
addShape('scope', object({ product: identifier, component: identifier, installId: identifier,
  installationScopeId: identifier, channel: common('channel'), channelRevision: common('positive'), targetKey: common('targetKey') }));
addShape('current-facts', object({ stableFacts: common('stableFacts'), host: common('host'),
  clientFactsDigest: hash, updaterFactsDigest: hash, helperFactsDigest: hash, launcherFactsDigest: hash }));
addShape('request-base', object({ protocolVersion: literal(1), scope: api('scope') }));
addShape('gray-targeting', { oneOf: [object({ mode: { enum: ['disabled', 'unavailable'] }, serialNumber: { type: 'null' } }),
  object({ mode: literal('enabled'), serialNumber: { type: 'string', minLength: 1, maxLength: 65_536 } })] });
addShape('revision-ref', object({ recordKind: { enum: ['rootHead', 'metadataHead', 'selectionScope', 'registry', 'artifact', 'release',
  'releaseTarget', 'deployment', 'rollout', 'supplyChain', 'bridgeEligibility', 'workflow', 'session', 'authorization',
  'transaction', 'staged', 'activeValidate', 'task', 'qualityProjection', 'freezeReason', 'stageObservation', 'operation'] },
  recordId: identifier, revision: integer, sha256: hash }));
addShape('read-set', { ...array(api('revision-ref'), 1), uniqueItems: true });
addShape('target-content', object({ targetVersion: common('version'), targetKey: common('targetKey'),
  packageManifest: common('metadataReference'), packageId: identifier, package: common('byteIdentity'),
  finalInstaller: common('nativeIdentity'), activationMode: { enum: ['directInstall', 'stagedRestart'] } }));
const stateFacts = {
  factType: literal('local-state'), verifiedObjectSha256: hash, verifiedSlotId: identifier, isolatedSlotId: identifier,
  verificationResult: { enum: ['verified', 'rejected'] }, preflightResult: { enum: ['supported', 'unsupported'] },
  targetScopeId: identifier, permissionScopeId: identifier, permissionSource: { enum: ['existing-helper', 'os-confirmed'] },
  ownerEpoch: common('positive'), freezeEpoch: common('positive'), writerCount: integer, activeWriterCount: integer, allExecutorsStopped: { type: 'boolean' },
  frozenDataScopeSha256: hash, dataScopeSha256: hash, backupFreezeEpoch: common('positive'),
  backupPreparationEpoch: common('positive'), backupDataScopeSha256: hash,
  lastOnlineAuthorizationJti: identifier, lastOnlineAt: integer, installerInvokedAt: integer,
  firstActiveWriteAt: nullable(integer), pointerSwitchedAt: nullable(integer), boundaryAt: nullable(integer),
  activeMutationCount: integer, activeObjectSha256: hash, oldIdentitySha256: hash, oldDataSha256: hash,
  stoppedOwnerEpoch: common('positive'), safetyResult: { enum: ['intact', 'damaged', 'unproven'] },
  healthResult: { enum: ['passed', 'failed', 'unknown'] }, dataResult: { enum: ['consistent', 'inconsistent', 'unknown'] },
  healthStartedAt: integer, healthAttempts: { type: 'integer', minimum: 1, maximum: 2 },
  lastHealthAttemptMs: { type: 'integer', minimum: 0, maximum: 120_000 },
  confirmedHopSha256: hash, workflowId: identifier, writableSessionId: identifier, hopSha256: hash,
  trigger: { enum: ['restart'] }, preinstallState: literal('staging_completed'), stagedRevision: common('positive'), stagedValidUntil: integer,
};
addShape('event-facts', { oneOf: [object(stateFacts, Object.keys(stateFacts).filter((key) => key !== 'factType')),
  object({ factType: literal('download-progress'), bytesReceived: integer, totalBytes: common('positive'),
    observation: { enum: ['progress', 'waiting', 'backoff', 'paused'] }, updatedAt: integer, fileSourceClass: identifier }),
  object({ factType: literal('task-observation'), originalTargetIdentitySha256: hash,
    activeObjectSha256: hash, dataResult: { enum: ['consistent', 'inconsistent', 'unknown'] },
    healthResult: { enum: ['passed', 'failed', 'unknown'] }, windowId: identifier }),
] });
addShape('status-request', object({ protocolVersion: literal(1), product: identifier, component: identifier,
  installationScopeId: identifier, purpose: { enum: ['preinstall', 'install', 'activate'] }, authorizationJti: identifier,
  consumeKey: identifier, transactionId: identifier, consumeRequestDigest: hash,
  recoverySecret: { type: 'string', pattern: '^[A-Za-z0-9_-]{43}$' } }));
addShape('status-response', { oneOf: ['consumed', 'cancelled', 'expired'].map((state) => object({
  protocolVersion: literal(1), requestId: identifier, state: literal(state),
  transactionId: state === 'consumed' ? identifier : { type: 'null' },
})) });

export const requestShape = (properties) => object({ protocolVersion: literal(1), scope: api('scope'), ...properties });
export const responseShape = (properties) => object({ protocolVersion: literal(1), requestId: identifier, ...properties });

// Read-only T12/T13/T17 port vocabulary, never a public client input.
addShape('forward-edge', object({ fromSourceProfileId: hash, releaseTargetId: identifier,
  releaseTargetRevision: common('positive'), toCanonicalAppVersion: common('version'), packageId: identifier,
  packageSha256: hash, transformId: identifier, transformRevision: common('positive'),
  allowedChannels: { ...array(common('channel'), 1), uniqueItems: true } }));
addShape('selection-node', object({ deploymentId: identifier, channel: common('channel'), version: common('version'),
  minimumSourceVersion: common('version'), hopKind: { enum: ['ordinary', 'bridge'] },
  deploymentState: { enum: ['scheduled', 'active', 'paused', 'withdrawn', 'superseded'] },
  releaseState: { enum: ['assembled', 'closed', 'cancelled'] }, releaseTargetState: { enum: ['approved', 'revoked', 'rejected'] },
  artifactState: { enum: ['valid', 'quarantined', 'rejected'] }, supplyChainState: { enum: ['approved', 'expired', 'revoked'] },
  supplyChainExpiresAt: nullable(integer), releaseVisibleAt: integer, installNotBefore: integer, installNotAfter: nullable(integer),
  isBaseline: { type: 'boolean' }, ordinaryPathApproval: nullable({ enum: ['approved', 'revoked'] }),
  ownRolloutState: nullable({ enum: ['draft', 'running', 'paused', 'completed', 'aborted'] }),
  bridgeState: nullable({ enum: ['enabled', 'disabled'] }), releaseTargetId: identifier, releaseTargetRevision: common('positive'),
  packageId: identifier, packageSha256: hash, transformId: identifier, transformRevision: common('positive'),
  upgradeType: { enum: ['normal', 'silent', 'forced'] }, activationMode: { enum: ['directInstall', 'stagedRestart'] },
  certificationState: { enum: ['certified', 'unverified'] } }));
addShape('selection-input', object({ currentVersion: common('version'), supportFloorVersion: common('version'),
  channel: common('channel'), now: integer, stableFactsSha256: hash, baselineId: identifier, candidateId: nullable(identifier),
  rollout: nullable(object({ percentage: { type: 'integer', minimum: 1, maximum: 100 }, snPresent: { type: 'boolean' },
    included: { type: 'boolean' }, excluded: { type: 'boolean' }, bucketBasisPoints: integer })),
  nodes: array(api('selection-node'), 1), profiles: array(object({ sourceProfileId: hash, stableFactsSha256: hash,
    state: { enum: ['selectable', 'forward-only', 'repair-only', 'revoked'] },
    policy: nullable(object({ policyId: identifier, revision: common('positive'), sha256: hash, edges: array(api('forward-edge'), 1) })) })) }));
addShape('model-read-ref', object({ key: identifier, recordKind: identifier, revision: integer, sha256: hash }));
addShape('deny-entry', { oneOf: [object({ subjectKind: literal('signingKey'), subjectId: hash,
  roles: { ...array(api('trust-role'), 1), uniqueItems: true } }), object({ subjectKind: { enum: ['deployment', 'release', 'releaseTarget',
  'artifact', 'package', 'transform'] }, subjectId: identifier, roles: { type: 'array', maxItems: 0 } })] });
addShape('deny-record', object({ revision: common('positive'), recordKind: literal('deny'), entries: array(api('deny-entry')) }));
addShape('prepared-bridge', object({ revision: common('positive'), projectionOwner: literal('T13'), state: literal('prepared'),
  deploymentKey: identifier, expectedDeploymentRevision: common('positive'), eligibilityKey: identifier,
  expectedEligibilityRevision: common('positive'), validFrom: integer, expiresAt: nullable(integer), approvalKey: identifier }));
addShape('chain-keys', object({ rootKey: identifier, denyKey: identifier, deploymentKey: identifier, releaseKey: identifier,
  releaseTargetKey: identifier, supplyChainKey: identifier, artifactKeys: array(identifier, 1), bridgeKey: identifier }, ['bridgeKey']));
addShape('path-projection', object({ revision: common('positive'), projectionOwner: literal('T13'), state: literal('approved'),
  deploymentKey: identifier, deploymentRevision: common('positive'), scopeKey: identifier,
  projectedWritesSha256: nullable(hash),
  rootKey: identifier, headKey: identifier, denyKey: identifier, registryKey: identifier, qualifiedAt: integer, expiresAt: integer,
  sources: array(object({ sourceProfileId: hash, currentVersion: common('version'), actualHopIndex: integer, groupIdentity: hash,
    steps: array(object({ kind: { enum: ['ordinary', 'bridge'] }, chain: api('chain-keys'),
      fromSourceProfileId: hash, toSourceProfileId: hash, activationMode: { enum: ['directInstall', 'stagedRestart'] },
      certificationKey: identifier, ordinaryApprovalKey: nullable(identifier), transformKey: nullable(identifier) }), 1) })),
  readSet: { ...array(api('model-read-ref'), 1), uniqueItems: true } }));
addShape('qualification-projection', object({ revision: integer, projectionOwner: { enum: ['T17', 'T19'] }, state: literal('qualified'),
  qualifiedAt: integer, expiresAt: integer, qualifiedClaimsSha256: hash, selectionIdentitySha256: hash, observationWindowId: identifier,
  context: { oneOf: [object({ role: identifier, product: identifier, component: identifier,
    scope: object({ installationScopeId: identifier, channel: common('channel'), channelRevision: common('positive'), targetKey: common('targetKey') }) }),
  object({ immutableContextSha256: hash })] },
  endpointKind: { enum: ['baseline', 'candidate'] }, hopKind: { enum: ['ordinary', 'bridge'] }, ordinaryPathRequired: { type: 'boolean' },
  readSet: array(api('model-read-ref'), 1), contentIdentity: object({ packageId: identifier, package: common('byteIdentity'),
    finalInstaller: common('nativeIdentity'), targetVersion: common('version') }) }, ['contentIdentity']));
addShape('quality-counts', object({ succeeded: integer, failed: integer, incomplete: integer, unknown: integer }));
addShape('quality-metric-policy', object({ minimumSamples: common('positive'), threshold: object({ numerator: common('positive'), denominator: common('positive') }) }));
addShape('approval-evidence', object({ revision: common('positive'), projectionOwner: literal('T05'), state: literal('approved'),
  bodySha256: hash, authorId: identifier, reviewerIds: { ...array(identifier, 1), uniqueItems: true },
  context: object({ command: identifier, scope: api('management-scope'), objectId: identifier, objectRevision: common('positive') }) }));
addShape('management-authorization', object({ revision: common('positive'), projectionOwner: literal('T05'),
  state: { enum: ['authorized', 'revoked'] }, actorId: identifier, mfaVerified: { type: 'boolean' },
  permissions: { ...array({ enum: ['key-management'] }, 1), uniqueItems: true }, authorizedAt: integer, expiresAt: integer,
  bodySha256: hash, context: object({ command: identifier, scope: api('management-scope'), objectId: identifier, objectRevision: common('positive') }) }));
addShape('key-use-index', object({ revision: common('positive'), recordKind: literal('keyUseIndex'), projectionOwner: literal('T07'),
  product: identifier, usageKeys: { ...array(identifier), uniqueItems: true } }));
addShape('key-use', object({ revision: common('positive'), recordKind: literal('keyUse'), projectionOwner: literal('T07'),
  product: identifier, component: nullable(identifier), role: api('trust-role'), channel: nullable(common('channel')), targetKey: nullable(common('targetKey')),
  keys: { ...array(common('signingKey'), 1), uniqueItems: true }, threshold: common('positive'),
  issuanceState: { enum: ['active', 'stopped'] }, lastIssuedAt: integer, objectValidUntil: integer,
  uploadUntil: integer, recoveryUntil: integer, retainUntil: integer, readSet: array(api('model-read-ref'), 1) }));
addShape('private-release-update', object({ releaseKey: identifier, envelope: { $ref: 'urn:pinvou:upgrade:v1:metadata:release' } }));
addShape('candidate-publication-opening', object({ scopeKey: identifier, opening: common('opening') }));
addShape('verification-archive', object({ revision: common('positive'), recordKind: literal('verificationArchive'),
  projectionOwner: literal('T07'), product: identifier, component: identifier,
  entries: array(object({ archivedAt: integer, rootHead: object({ key: identifier, revision: common('positive'), sha256: hash }),
    lineage: api('root-lineage'), root: { $ref: 'urn:pinvou:upgrade:v1:metadata:root#/properties/signed' },
    release: { $ref: 'urn:pinvou:upgrade:v1:metadata:release' }, packages: array({ $ref: 'urn:pinvou:upgrade:v1:metadata:package' }, 1) })) }));
addShape('root-lineage', object({ revision: common('positive'), recordKind: literal('rootLineage'), projectionOwner: literal('T07'),
  product: identifier, anchorKey: identifier, anchorBodySha256: hash, rootVersion: common('positive'), rootBodySha256: hash,
  chainMaterialKey: identifier, chainMaterialId: identifier, chainMaterialSha256: hash, readSet: array(api('model-read-ref'), 2) }));
addShape('root-chain-material', object({ revision: common('positive'), recordKind: literal('rootChainMaterial'), projectionOwner: literal('T07'),
  product: identifier, materialId: identifier, materialSha256: hash, rootVersion: common('positive'), rootBodySha256: hash,
  previousRootBodySha256: nullable(hash), rootEnvelopeSha256: nullable(hash) }));
const metrics = Object.fromEntries(['download', 'verification', 'installation', 'health', 'preinstallation', 'activation']
  .map((metric) => [metric, api('quality-counts')]));
addShape('quality-observation', object({ startedAt: integer,
  groups: array(object({ groupIdentity: hash, metrics: object(metrics, Object.keys(metrics)) }), 1) }));
addShape('stage-fact', object({ revision: common('positive'), rolloutKey: identifier, stageId: identifier, windowId: identifier,
  kind: { enum: ['started', 'advanced', 'completed', 'paused', 'resumed', 'aborted', 'quality-unfrozen'] }, effectiveAt: integer,
  sourceWatermark: integer, planRevision: common('positive'), planSha256: hash }));
addShape('quality-projection', object({ revision: common('positive'), rolloutKey: identifier, planSha256: hash, stageId: identifier,
  windowId: identifier, stageFactRevision: common('positive'), sourceHeadKey: identifier, sourceWatermark: integer,
  processedWatermark: integer, freezeIndexRevision: common('positive'), observation: api('quality-observation'),
  historicalObservations: array(object({ reasonKey: identifier, observation: api('quality-observation') })) }, ['historicalObservations']));
const policyMetrics = Object.fromEntries(Object.keys(metrics).map((metric) => [metric, api('quality-metric-policy')]));
addShape('frozen-stage-plan', object({ upgradeType: { enum: ['normal', 'silent', 'forced'] }, stages: array(object({ stageId: identifier,
  percentage: { type: 'integer', minimum: 1, maximum: 100 }, minimumObservationMs: common('positive'),
  requiredMetrics: { ...array({ enum: Object.keys(metrics) }, 1), uniqueItems: true },
  metrics: object(policyMetrics, Object.keys(policyMetrics)) }), 1),
  snIncludeSetRef: nullable(api('approved-change')), snExcludeSetRef: nullable(api('approved-change')),
  progressReportIntervalMs: common('positive'), lossIntervalMs: common('positive') }));
