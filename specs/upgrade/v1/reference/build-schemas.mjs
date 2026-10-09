// This source emits the frozen JSON Schema artifacts. It does not emit fixtures.
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

const base = 'urn:pinvou:upgrade:v1:';
const ref = (name) => ({ $ref: `${base}common#/$defs/${name}` });
const integer = { type: 'integer', minimum: 0, maximum: Number.MAX_SAFE_INTEGER };
const positive = { ...integer, minimum: 1 };
const text = { type: 'string', minLength: 1, maxLength: 65536 };
const identifier = { type: 'string', pattern: '^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$' };
const hash = { type: 'string', pattern: '^[0-9a-f]{64}$' };
const version = { type: 'string', pattern: '^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$' };
const channel = { enum: ['stable', 'beta', 'internal'] };
const targetKey = { enum: ['windows-x86_64-nsis-perMachine', 'linux-x86_64-deb-perMachine',
  'linux-arm64-deb-perMachine', 'macos-universal-dmg-perMachine'] };
const nullable = (schema) => ({ anyOf: [schema, { type: 'null' }] });
const array = (items, minimum = 0) => ({ type: 'array', items, minItems: minimum, maxItems: 4096 });
const object = (properties, optional = []) => ({ type: 'object', properties,
  required: Object.keys(properties).filter((key) => !optional.includes(key)), additionalProperties: false });
const roles = ['root', 'timestamp', 'snapshot', 'target', 'release', 'package', 'decision',
  'download', 'authorization-preinstall', 'authorization-install', 'authorization-activate',
  'helper-cleanup', 'telemetry-event', 'preinstall-transaction-event',
  'install-transaction-event', 'activate-transaction-event',
  'reconciliation-action', 'recovery-review-action', 'reconciliation-event', 'recovery-review-event'];

const defs = {
  identifier, hash, version, integer, positive, text, channel, targetKey,
  byteIdentity: object({ size: positive, sha256: hash }),
  metadataReference: object({ role: { enum: ['root', 'timestamp', 'snapshot', 'target', 'release', 'package'] },
    scope: { type: 'object', maxProperties: 4, additionalProperties: identifier },
    version: nullable(positive), expiresAt: nullable(integer), size: { ...positive, maximum: 1048576 }, envelopeSha256: hash }),
  nativeIdentity: object({ size: positive, sha256: hash, identity: identifier,
    publisher: identifier, format: { enum: ['nsis', 'deb', 'dmg', 'helper', 'launcher'] } }),
  supplyChain: object({ approvalId: identifier, revision: positive, effectiveExpiresAt: nullable(integer) }),
  planReference: object({ planId: identifier, revision: positive, sha256: hash }),
  plans: object({ installation: ref('planReference'), migration: nullable(ref('planReference')),
    backup: nullable(ref('planReference')), helperMigration: nullable(ref('planReference')),
    launcherMigration: nullable(ref('planReference')), takeover: nullable(ref('planReference')) }),
  relativePath: { type: 'string', minLength: 1, maxLength: 1024,
    pattern: '^(?!/)(?!.*(?:^|/)\\.\\.?(?:/|$))[A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)*$' },
  helperArtifact: object({ helperId: identifier, version, targetKey,
    nativeIdentity: ref('nativeIdentity'), installationTarget: ref('relativePath') }),
  host: object({ os: { enum: ['windows', 'linux', 'macos'] }, arch: { enum: ['x86_64', 'arm64'] },
    osVersion: identifier }),
  pathPolicy: object({ policyId: identifier, revision: positive, sha256: hash, edgeSha256: hash }),
  opening: object({ targetKey, deploymentId: identifier, deploymentRevision: positive,
    rolloutId: identifier, rolloutRevision: positive, releaseEnvelopeSha256: hash, leafSalt: hash }),
  entityChain: object({ deploymentId: identifier, deploymentRevision: positive,
    deploymentState: { enum: ['active', 'superseded'] },
    releaseId: identifier, releaseRevision: positive, releaseTargetId: identifier,
    releaseState: { const: 'closed' }, releaseManifest: ref('metadataReference'),
    releaseTargetRevision: positive, releaseTargetState: { const: 'approved' }, packageManifest: ref('metadataReference'),
    artifactId: identifier, artifactState: { const: 'valid' }, packageId: identifier, package: ref('byteIdentity'),
    releaseVisibleAt: integer, installNotBefore: integer, installNotAfter: nullable(integer),
    supplyChain: ref('supplyChain') }),
  metadataSet: object({ root: ref('metadataReference'), timestamp: ref('metadataReference'),
    snapshot: ref('metadataReference'), target: ref('metadataReference'),
    releases: array(ref('metadataReference'), 1) }),
  staged: object({ preinstallTransactionId: identifier, stagedRevision: positive,
    slotIdentity: identifier, stagedAt: integer, stagedValidUntil: integer }),
  preparation: object({ ownerEpoch: positive, scopeRevision: positive }),
  backup: object({ recordId: identifier, freezeEpoch: positive, sha256: hash,
    dataScopeDigest: hash, preparationEpoch: positive }),
  originalStage: object({ deploymentId: identifier, rolloutId: nullable(identifier),
    stageId: identifier, groupIdentity: hash, firstQualifiedAt: integer }),
  updateBinding: object({ workflowId: identifier, originalStage: ref('originalStage'),
    endpointKind: { enum: ['baseline', 'candidate'] }, hopKind: { enum: ['ordinary', 'bridge'] },
    baselineDeploymentId: identifier, baselineRevision: positive,
    endpointChain: ref('entityChain'), hopChain: ref('entityChain'),
    rollout: nullable(object({ rolloutId: identifier, revision: positive, state: { const: 'running' },
      policyRevision: positive, opening: ref('opening'), locator: identifier, locatorExpiresAt: integer })),
    bridge: nullable(object({ eligibilityId: identifier, revision: positive })),
    ordinaryPathApproval: nullable(object({ approvalId: identifier, revision: positive })),
    targetVersion: version, packageId: identifier, finalInstaller: ref('nativeIdentity'),
    helper: ref('nativeIdentity'), launcher: ref('nativeIdentity'), plans: ref('plans'),
    activationMode: { enum: ['directInstall', 'stagedRestart'] },
    upgradeType: { enum: ['normal', 'silent', 'forced'] },
    migrationMode: { enum: ['none', 'backwardCompatible', 'irreversible'] },
    backupPolicy: { enum: ['notRequired', 'required'] } }),
  taskBinding: object({ taskId: identifier, revision: positive, taskPurpose: { enum: ['reconcile', 'recoveryReview'] },
    originalTransactionId: identifier, originalAuthorizationJti: identifier,
    packageId: identifier, finalInstaller: ref('nativeIdentity'),
    activationMode: { enum: ['directInstall', 'stagedRestart'] },
    originalPurpose: { enum: ['preinstall', 'install', 'activate'] },
    originalConsumeRequestDigest: hash, retirementProofSha256: nullable(hash),
    workflowId: identifier, originalStage: ref('originalStage'),
    endpointChain: ref('entityChain'), hopChain: ref('entityChain'),
    targetVersion: version, sourceVersion: version, upgradeType: { enum: ['normal', 'silent', 'forced'] },
    packageManifest: ref('metadataReference'), package: ref('byteIdentity'),
    windowId: identifier, windowStart: integer, windowEnd: integer }),
  signature: object({ keyId: hash, signature: { type: 'string', pattern: '^[A-Za-z0-9_-]{86}$' } }),
  signingKey: object({ keyId: hash, algorithm: { const: 'ed25519' },
    publicKey: { type: 'string', pattern: '^[A-Za-z0-9_-]{43}$' } }),
  rolePolicy: object({ role: { enum: roles }, component: nullable(identifier),
    channel: nullable(channel), targetKey: nullable(targetKey), threshold: positive,
    keyIds: { ...array(hash, 1), uniqueItems: true } }),
  stableFacts: object({ product: identifier, component: identifier, targetKey,
    canonicalAppVersion: version, application: ref('nativeIdentity'), helper: ref('nativeIdentity'),
    launcher: ref('nativeIdentity'), installation: object({ format: { enum: ['nsis', 'deb', 'dmg'] },
      installScope: { const: 'perMachine' }, ownershipProtocol: identifier }),
    dataMigration: object({ dataFormat: identifier, migrationProtocol: identifier }) }),
  statusBinding: object({ recoveryHandleHash: hash, product: identifier, component: identifier,
    installationScopeId: identifier, purpose: { enum: ['preinstall', 'install', 'activate'] },
    authorizationJti: identifier, consumeKey: identifier, transactionId: identifier, consumeRequestDigest: hash }),
  consumeRequest: object({ protocolVersion: { const: 1 }, product: identifier, component: identifier,
    installationScopeId: identifier, purpose: { enum: ['preinstall', 'install', 'activate'] },
    authorizationJti: identifier, consumeKey: identifier, transactionId: identifier,
    authorizationEnvelopeSha256: hash, recoveryHandleHash: hash,
    clientFactsDigest: hash, preparation: ref('preparation'), freezeEpoch: nullable(positive),
    backup: nullable(ref('backup')), staged: nullable(ref('staged')), plans: ref('plans'),
    preinstallSlot: nullable(object({ slotIdentity: identifier, slotRevision: positive, state: { const: 'inactive' } })) }),
};

const schemas = new Map();
function add(name, schema) {
  schemas.set(name, { $schema: 'https://json-schema.org/draft/2020-12/schema',
    $id: base + name.replaceAll('/', ':'), ...schema });
}
add('common', { $defs: defs });

const context = (role, scope, component = identifier) => ({ protocolVersion: { const: 1 },
  role: { const: role }, product: identifier, component, scope });
const emptyScope = object({});
const targetScope = object({ channel, targetKey });
function envelope(signed) {
  return object({ signed, signatures: array(ref('signature'), 1) });
}
function metadata(role, scope, fields, optional = []) {
  const expiration = role === 'package' ? {} : { issuedAt: integer, expiresAt: integer };
  const revision = role === 'package' ? {} : { [role === 'release' ? 'revision' : 'version']: positive };
  add(`metadata/${role}`, envelope(object({ ...context(role, scope, role === 'root' ? { type: 'null' } : identifier),
    ...revision, ...expiration, ...fields }, optional)));
}
metadata('root', emptyScope, { keys: array(ref('signingKey'), 1), roles: array(ref('rolePolicy'), 1) });
metadata('timestamp', emptyScope, { snapshot: ref('metadataReference') });
metadata('snapshot', emptyScope, { entries: array(ref('metadataReference'), 1) });
metadata('target', targetScope, { supportFloorVersion: version, installScope: { const: 'perMachine' },
  hostArchitectures: { ...array({ enum: ['x86_64', 'arm64'] }, 1), uniqueItems: true },
  minimumOsVersion: identifier, maximumOsVersion: nullable(identifier), selectionGeneration: positive,
  baselineRelease: ref('metadataReference'), rolloutSetCommitment: hash,
  ordinaryPaths: array(object({ deploymentId: identifier, deploymentRevision: positive,
    approvalId: identifier, approvalRevision: positive, release: ref('metadataReference') })),
  bridgePaths: array(object({ deploymentId: identifier, deploymentRevision: positive,
    deploymentState: { const: 'superseded' }, eligibilityId: identifier, eligibilityRevision: positive,
    release: ref('metadataReference') })) });
metadata('release', object({ releaseId: identifier }), { appVersion: version,
  notes: object({ 'zh-CN': text, en: text, ja: text }),
  targets: array(object({ targetKey, releaseTargetId: identifier, releaseTargetRevision: positive,
    minimumSourceVersion: version, migrationMode: { enum: ['none', 'backwardCompatible', 'irreversible'] },
    backupPolicy: { enum: ['notRequired', 'required'] },
    activationModes: { ...array({ enum: ['directInstall', 'stagedRestart'] }, 1), uniqueItems: true },
    packageManifest: ref('metadataReference'), launcher: ref('nativeIdentity') }), 1) });
metadata('package', object({ targetKey }), { manifestId: identifier, appVersion: version,
  activationModes: { ...array({ enum: ['directInstall', 'stagedRestart'] }, 1), uniqueItems: true },
  fullPackage: object({ packageId: identifier, size: positive, sha256: hash }),
  finalInstaller: ref('nativeIdentity'), helper: ref('nativeIdentity'), launcher: ref('nativeIdentity'),
  helpers: array(ref('helperArtifact'), 1), executionHelperId: identifier,
  sbom: object({ fileName: ref('relativePath'), schemaVersion: identifier,
    format: { const: 'CycloneDX-JSON' }, size: positive, sha256: hash }),
  provenance: object({ fileName: ref('relativePath'), schemaVersion: identifier, format: { const: 'in-toto-JSONL' },
    sourceRevision: object({ algorithm: { enum: ['git-sha1', 'git-sha256'] },
      value: { type: 'string', pattern: '^(?:[0-9a-f]{40}|[0-9a-f]{64})$' } }),
    pipelineIdentity: identifier, size: positive, sha256: hash,
    finalInstallerSha256: hash }),
  incrementalPackages: array(object({ packageId: identifier, size: positive, sha256: hash,
    baseVersion: version, baseArtifactSha256: hash, resultArtifactSize: positive, resultArtifactSha256: hash,
    algorithm: identifier, algorithmVersion: identifier, format: identifier })) }, ['manifestId']);

const credentialScope = object({ installationScopeId: identifier, channel, channelRevision: positive, targetKey });
const credentialBase = (role, audience, purpose) => ({ ...context(role, credentialScope),
  credentialType: { const: role }, aud: { const: audience }, purpose: { const: purpose },
  credentialPurpose: { const: purpose },
  jti: identifier, signingKeyId: hash, iat: integer, nbf: integer, exp: integer });
const qualification = {
  installId: identifier, decisionId: identifier, decisionRevision: positive,
  telemetrySessionId: identifier, currentVersion: version, host: ref('host'),
  selectionGeneration: positive, metadataSet: ref('metadataSet'), registryVersion: positive,
  sourceProfileId: hash, clientFactsDigest: hash, updaterFactsDigest: hash,
  helperFactsDigest: hash, launcherFactsDigest: hash, forwardPath: nullable(ref('pathPolicy')),
};
function credential(role, audience, purpose, fields) {
  add(`credentials/${role}`, envelope(object({ ...credentialBase(role, audience, purpose), ...fields })));
}
credential('decision', 'upgrade-control', 'check', { ...qualification, requestNonce: identifier,
  updateAvailable: { type: 'boolean' }, update: nullable(ref('updateBinding')), reason: nullable(identifier) });
credential('download', 'upgrade-download-info', 'download', { ...qualification,
  update: ref('updateBinding'), packageId: identifier, package: ref('byteIdentity') });
for (const purpose of ['preinstall', 'install', 'activate']) {
  credential(`authorization-${purpose}`, 'upgrade-consume', purpose, { ...qualification,
    update: ref('updateBinding'), authorizationJti: identifier, transactionId: identifier,
    preparation: ref('preparation'), freezeEpoch: nullable(positive), backup: nullable(ref('backup')),
    staged: nullable(ref('staged')),
    preinstallSlot: nullable(object({ slotIdentity: identifier, slotRevision: positive, state: { const: 'inactive' } })) });
  credential(`${purpose}-transaction-event`, 'update-events', purpose, { installId: identifier,
    decisionId: identifier, decisionRevision: positive, authorizationJti: identifier,
    transactionId: identifier, transactionStartedAt: integer, denyRevisionAtIssue: integer,
    eventModel: { const: 'transaction-v1' }, allowedEvents: { ...array(identifier, 1), uniqueItems: true }, eventNotAfter: integer });
}
credential('helper-cleanup', 'upgrade-helper-cleanup', 'cleanup', { installId: identifier,
  registryVersion: positive, selectionGeneration: positive, beforeFactsDigest: hash,
  afterFactsDigest: hash, preparation: ref('preparation'),
  deletionSet: array(object({ identity: ref('nativeIdentity'), protectedPathId: identifier }), 1) });
credential('telemetry-event', 'update-events', 'telemetry', { installId: identifier,
  decisionId: identifier, decisionRevision: positive, telemetrySessionId: identifier,
  selectionGeneration: positive, sessionStartedAt: integer, denyRevisionAtIssue: integer,
  eventModel: { const: 'telemetry-v1' }, allowedEvents: { ...array(identifier, 1), uniqueItems: true }, eventNotAfter: integer });
for (const [prefix, purpose] of [['reconciliation', 'reconcile'], ['recovery-review', 'recoveryReview']]) {
  credential(`${prefix}-action`, 'upgrade-task-action', purpose, { installId: identifier, task: ref('taskBinding') });
  credential(`${prefix}-event`, 'update-events', purpose, { installId: identifier,
    task: ref('taskBinding'), taskStartedAt: integer, denyRevisionAtIssue: integer,
    eventModel: { const: `${prefix}-v1` }, allowedEvents: { ...array(identifier, 1), uniqueItems: true }, eventNotAfter: integer });
}
add('bindings/source-facts', ref('stableFacts'));
add('bindings/status', ref('statusBinding'));
add('bindings/opening', ref('opening'));
add('bindings/consume-request', ref('consumeRequest'));
add('bindings/bytes', ref('byteIdentity'));
add('bindings/native-identity', ref('nativeIdentity'));

export { schemas };

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  for (const [name, schema] of schemas) {
    const destination = new URL(`../schemas/${name}.json`, import.meta.url);
    await mkdir(new URL('.', destination), { recursive: true });
    await writeFile(destination, JSON.stringify(schema, null, 2) + '\n');
  }
}
