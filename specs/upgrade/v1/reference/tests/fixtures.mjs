// Test keys are generated in memory; no private key is stored in this repository.
import { generateKeyPairSync } from 'node:crypto';
import { sha256, rolloutCommitment } from '../digests.mjs';
import { publicKeyDescriptor, signEnvelope, envelopeReference } from '../signatures.mjs';
import { DAY } from '../semantics.mjs';

export const NOW = 1_700_000_000_000;
export const TARGET = 'linux-x86_64-deb-perMachine';
export const HASH = 'a'.repeat(64);
export const native = (format, identity = format) => ({ size: 123, sha256: HASH,
  identity, publisher: 'test-publisher', format });
export const byteIdentity = () => ({ size: 123, sha256: HASH });

export function createFixtureSet({ component = 'app', clock = NOW, signingKeys = null } = {}) {
  const keys = signingKeys ?? Array.from({ length: 4 }, () => generateKeyPairSync('ed25519').privateKey);
  const descriptors = keys.map(publicKeyDescriptor);
  const roles = ['root', 'timestamp', 'snapshot', 'target', 'release', 'package', 'decision', 'download',
    'authorization-preinstall', 'authorization-install', 'authorization-activate', 'helper-cleanup',
    'telemetry-event', 'preinstall-transaction-event', 'install-transaction-event', 'activate-transaction-event',
    'reconciliation-action', 'recovery-review-action', 'reconciliation-event', 'recovery-review-event'];
  const policies = roles.map((role) => {
    const global = ['root', 'timestamp', 'snapshot', 'release'].includes(role);
    return { role, component: role === 'root' ? null : component,
      channel: global || role === 'package' ? null : 'stable', targetKey: global ? null : TARGET,
      threshold: role === 'root' ? 2 : 1,
      keyIds: role === 'root' ? descriptors.slice(0, 3).map((key) => key.keyId) : [descriptors[3].keyId] };
  });
  const context = (role, scope = {}) => ({ protocolVersion: 1, role, product: 'pinvou',
    component: role === 'root' ? null : component, scope });
  const timed = { version: 1, issuedAt: clock, expiresAt: clock + DAY };
  const root = { ...context('root'), ...timed, expiresAt: clock + 365 * DAY,
    keys: descriptors, roles: policies };
  const packageManifest = { ...context('package', { targetKey: TARGET }), appVersion: '2.0.0',
    activationModes: ['directInstall', 'stagedRestart'],
    fullPackage: { packageId: 'package-2', ...byteIdentity() }, finalInstaller: native('deb'),
    helper: native('helper'), launcher: native('launcher'),
    helpers: [{ helperId: 'helper-2', version: '2.0.0', targetKey: TARGET,
      nativeIdentity: native('helper'), installationTarget: 'helpers/helper-2' }], executionHelperId: 'helper-2',
    sbom: { fileName: 'SBOM.cdx.json', schemaVersion: '1.6', format: 'CycloneDX-JSON', ...byteIdentity() },
    provenance: { fileName: 'Provenance.intoto.jsonl', schemaVersion: '1', format: 'in-toto-JSONL',
      sourceRevision: { algorithm: 'git-sha1', value: 'a'.repeat(40) }, pipelineIdentity: 'test-pipeline', ...byteIdentity(),
      finalInstallerSha256: HASH }, incrementalPackages: [] };
  const metadata = { root, package: packageManifest };
  const bytes = { root: signEnvelope(root, keys.slice(0, 2)), package: signEnvelope(packageManifest, [keys[3]]) };
  const release = { ...context('release', { releaseId: 'release-2' }), revision: 1,
    issuedAt: clock, expiresAt: clock + 90 * DAY, appVersion: '2.0.0',
    notes: { 'zh-CN': 'Test release', en: 'Test release', ja: 'Test release' },
    targets: [{ targetKey: TARGET, releaseTargetId: 'release-target-2', releaseTargetRevision: 1,
      minimumSourceVersion: '1.0.0', migrationMode: 'none', backupPolicy: 'notRequired',
      activationModes: ['directInstall', 'stagedRestart'], packageManifest: envelopeReference(bytes.package),
      launcher: native('launcher') }] };
  metadata.release = release;
  bytes.release = signEnvelope(release, [keys[3]]);
  const target = { ...context('target', { channel: 'stable', targetKey: TARGET }), ...timed,
    supportFloorVersion: '1.0.0', installScope: 'perMachine', hostArchitectures: ['x86_64'],
    minimumOsVersion: '22.04', maximumOsVersion: null, selectionGeneration: 1,
    baselineRelease: envelopeReference(bytes.release), rolloutSetCommitment: rolloutCommitment(null), ordinaryPaths: [], bridgePaths: [] };
  metadata.target = target;
  bytes.target = signEnvelope(target, [keys[3]]);
  const snapshot = { ...context('snapshot'), ...timed,
    entries: ['target', 'release', 'package'].map((role) => envelopeReference(bytes[role])) };
  metadata.snapshot = snapshot;
  bytes.snapshot = signEnvelope(snapshot, [keys[3]]);
  const timestamp = { ...context('timestamp'), ...timed, snapshot: envelopeReference(bytes.snapshot) };
  metadata.timestamp = timestamp;
  bytes.timestamp = signEnvelope(timestamp, [keys[3]]);
  const metadataSet = { root: envelopeReference(bytes.root), timestamp: envelopeReference(bytes.timestamp),
    snapshot: envelopeReference(bytes.snapshot), target: envelopeReference(bytes.target),
    releases: [envelopeReference(bytes.release)] };
  const chain = { deploymentId: 'deployment-2', deploymentRevision: 1, deploymentState: 'active',
    releaseId: 'release-2', releaseRevision: 1, releaseState: 'closed', releaseManifest: envelopeReference(bytes.release),
    releaseTargetId: 'release-target-2', releaseTargetRevision: 1, releaseTargetState: 'approved',
    packageManifest: envelopeReference(bytes.package), artifactId: 'artifact-2', artifactState: 'valid',
    packageId: 'package-2', package: byteIdentity(), releaseVisibleAt: clock, installNotBefore: clock,
    installNotAfter: clock + DAY, supplyChain: { approvalId: 'approval-2', revision: 1, effectiveExpiresAt: clock + DAY } };
  const update = { workflowId: 'workflow-1', originalStage: { deploymentId: 'deployment-2', rolloutId: null,
    stageId: 'baseline', groupIdentity: HASH, firstQualifiedAt: clock }, endpointKind: 'baseline', hopKind: 'ordinary',
    baselineDeploymentId: 'deployment-2', baselineRevision: 1,
    endpointChain: structuredClone(chain), hopChain: structuredClone(chain), rollout: null, bridge: null,
    ordinaryPathApproval: null, targetVersion: '2.0.0', packageId: 'package-2', finalInstaller: native('deb'),
    helper: native('helper'), launcher: native('launcher'), activationMode: 'directInstall', upgradeType: 'normal',
    migrationMode: 'none', backupPolicy: 'notRequired', plans: {
      installation: { planId: 'native-full-2', revision: 1, sha256: HASH },
      migration: null, backup: null, helperMigration: null, launcherMigration: null,
      takeover: { planId: 'takeover-2', revision: 1, sha256: HASH } } };
  const credentialScope = { installationScopeId: 'scope-1', channel: 'stable', channelRevision: 1, targetKey: TARGET };
  const credential = (role, audience, purpose, fields = {}) => ({ ...context(role, credentialScope),
    credentialType: role, aud: audience, purpose, credentialPurpose: purpose, jti: 'jti-1', signingKeyId: descriptors[3].keyId,
    iat: clock, nbf: clock, exp: clock + 300_000, ...fields });
  const qualification = { installId: 'install-1', decisionId: 'decision-1', decisionRevision: 1,
    telemetrySessionId: 'session-1', currentVersion: '1.0.0', host: { os: 'linux', arch: 'x86_64', osVersion: '22.04' },
    selectionGeneration: 1, metadataSet, registryVersion: 1, sourceProfileId: HASH, clientFactsDigest: HASH,
    updaterFactsDigest: HASH, helperFactsDigest: HASH, launcherFactsDigest: HASH, forwardPath: null };
  const claims = {
    decision: credential('decision', 'upgrade-control', 'check', { ...qualification,
      exp: clock + 900_000, requestNonce: 'nonce-1', updateAvailable: true, update, reason: null }),
    download: credential('download', 'upgrade-download-info', 'download', { ...qualification,
      exp: clock + 900_000, update, packageId: 'package-2', package: byteIdentity() }),
    'helper-cleanup': credential('helper-cleanup', 'upgrade-helper-cleanup', 'cleanup', {
      exp: clock + 120_000, installId: 'install-1', registryVersion: 1, selectionGeneration: 1,
      beforeFactsDigest: HASH, afterFactsDigest: HASH, preparation: { ownerEpoch: 1, scopeRevision: 1 },
      deletionSet: [{ identity: native('helper'), protectedPathId: 'obsolete-helper-1' }] }),
    'telemetry-event': credential('telemetry-event', 'update-events', 'telemetry', {
      exp: clock + 7 * DAY, installId: 'install-1', decisionId: 'decision-1', decisionRevision: 1,
      telemetrySessionId: 'session-1', selectionGeneration: 1, sessionStartedAt: clock, denyRevisionAtIssue: 0,
      eventModel: 'telemetry-v1', allowedEvents: ['download_started', 'download_progress'], eventNotAfter: clock + DAY }),
  };
  for (const purpose of ['preinstall', 'install', 'activate']) {
    const authorizationUpdate = structuredClone(update);
    if (purpose !== 'install') {
      authorizationUpdate.activationMode = 'stagedRestart'; authorizationUpdate.upgradeType = 'silent';
    }
    const staged = purpose === 'activate' ? { preinstallTransactionId: 'preinstall-1', stagedRevision: 1,
      slotIdentity: 'slot-1', stagedAt: clock - 1000, stagedValidUntil: clock + DAY } : null;
    claims[`authorization-${purpose}`] = credential(`authorization-${purpose}`, 'upgrade-consume', purpose,
      { ...qualification, update: authorizationUpdate, authorizationJti: 'jti-1', transactionId: 'transaction-1',
        preparation: { ownerEpoch: 1, scopeRevision: 1 }, freezeEpoch: purpose === 'preinstall' ? null : 1,
        backup: null, staged, preinstallSlot: purpose === 'preinstall'
          ? { slotIdentity: 'slot-1', slotRevision: 1, state: 'inactive' } : null });
    claims[`${purpose}-transaction-event`] = credential(`${purpose}-transaction-event`, 'update-events', purpose,
      { exp: clock + 37 * DAY, installId: 'install-1', decisionId: 'decision-1', decisionRevision: 1,
        authorizationJti: 'jti-1', transactionId: 'transaction-1', transactionStartedAt: clock,
        denyRevisionAtIssue: 0, eventModel: 'transaction-v1', allowedEvents: ['authorization_consumed'], eventNotAfter: clock + 30 * DAY });
  }
  for (const [prefix, purpose] of [['reconciliation', 'reconcile'], ['recovery-review', 'recoveryReview']]) {
    const task = { taskId: 'task-1', revision: 1, taskPurpose: purpose, originalTransactionId: 'transaction-1',
      originalAuthorizationJti: 'jti-1', packageId: 'package-2', finalInstaller: native('deb'),
      originalPurpose: 'install', originalConsumeRequestDigest: HASH,
      retirementProofSha256: purpose === 'reconcile' ? HASH : null,
      workflowId: 'workflow-1', originalStage: structuredClone(update.originalStage),
      endpointChain: structuredClone(chain), hopChain: structuredClone(chain),
      targetVersion: '2.0.0', sourceVersion: '1.0.0', upgradeType: 'normal',
      packageManifest: envelopeReference(bytes.package), package: byteIdentity(),
      activationMode: 'directInstall', windowId: 'window-1', windowStart: clock, windowEnd: clock + DAY };
    claims[`${prefix}-action`] = credential(`${prefix}-action`, 'upgrade-task-action', purpose,
      { installId: 'install-1', task });
    claims[`${prefix}-event`] = credential(`${prefix}-event`, 'update-events', purpose,
      { exp: clock + 7 * DAY, installId: 'install-1', task, taskStartedAt: clock,
        denyRevisionAtIssue: 0, eventModel: `${prefix}-v1`, allowedEvents: ['task_observation'], eventNotAfter: clock + DAY });
  }
  const sourceFacts = { product: 'pinvou', component, targetKey: TARGET, canonicalAppVersion: '1.0.0',
    application: native('deb', 'app'), helper: native('helper'), launcher: native('launcher'),
    installation: { format: 'deb', installScope: 'perMachine', ownershipProtocol: 'owner-v1' },
    dataMigration: { dataFormat: 'data-v1', migrationProtocol: 'migration-v1' } };
  const consume = { protocolVersion: 1, product: 'pinvou', component, installationScopeId: 'scope-1',
    purpose: 'install', authorizationJti: 'jti-1', consumeKey: 'consume-1', transactionId: 'transaction-1',
    authorizationEnvelopeSha256: HASH, recoveryHandleHash: sha256(Buffer.alloc(32, 1)),
    clientFactsDigest: HASH, preparation: { ownerEpoch: 1, scopeRevision: 1 }, freezeEpoch: 1,
    backup: null, staged: null, plans: structuredClone(update.plans), preinstallSlot: null };
  return { keys, descriptors, metadata, bytes, claims, sourceFacts, consume,
    sign: (signed) => signEnvelope(signed, signed.role === 'root' ? keys.slice(0, 2) : [keys[3]]) };
}
