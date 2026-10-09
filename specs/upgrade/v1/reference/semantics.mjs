import { canonicalize } from './canonical-json.mjs';
import { equalDigest, rolloutCommitment } from './digests.mjs';
import { requireCondition } from './errors.mjs';

export const DAY = 86_400_000;
export const METADATA_LIFETIMES = Object.freeze({ root: 365 * DAY, timestamp: DAY,
  snapshot: 7 * DAY, target: 30 * DAY, release: 90 * DAY });
export const CREDENTIAL_LIFETIMES = Object.freeze({ decision: 900_000, download: 900_000,
  'authorization-preinstall': 300_000, 'authorization-install': 300_000,
  'authorization-activate': 300_000, 'helper-cleanup': 120_000,
  'telemetry-event': 7 * DAY, 'preinstall-transaction-event': 37 * DAY,
  'install-transaction-event': 37 * DAY, 'activate-transaction-event': 37 * DAY,
  'reconciliation-action': 300_000, 'recovery-review-action': 300_000,
  'reconciliation-event': 7 * DAY, 'recovery-review-event': 7 * DAY });

export function sameJson(left, right) {
  return Buffer.from(canonicalize(left)).equals(Buffer.from(canonicalize(right)));
}

export function assertBindings(actual, expected) {
  requireCondition(sameJson(actual, expected), 'BINDING_MISMATCH');
}

export function compareVersions(left, right) {
  const valid = (value) => typeof value === 'string'
    && /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/u.test(value);
  requireCondition(valid(left) && valid(right), 'APP_VERSION_INVALID');
  const a = left.split('.').map(BigInt);
  const b = right.split('.').map(BigInt);
  for (let i = 0; i < 3; i++) {
    if (a[i] < b[i]) return -1;
    if (a[i] > b[i]) return 1;
  }
  return 0;
}

export function assertWindow(start, end, maximum, now) {
  requireCondition(Number.isSafeInteger(start) && Number.isSafeInteger(end)
    && Number.isSafeInteger(now) && start >= 0 && end > start
    && end - start <= maximum, 'TIME_WINDOW_INVALID');
  requireCondition(now >= start && now < end, 'TIME_WINDOW_CLOSED');
}

function referenceScope(reference) {
  const { role, scope, version } = reference;
  const fields = role === 'target' ? ['channel', 'targetKey']
    : role === 'release' ? ['releaseId'] : role === 'package' ? ['targetKey'] : [];
  requireCondition(Object.keys(scope).length === fields.length
    && fields.every((field) => Object.hasOwn(scope, field)), 'REFERENCE_SCOPE_INVALID');
  requireCondition(role === 'package' ? version === null
    : Number.isSafeInteger(version) && version >= 1, 'REFERENCE_VERSION_INVALID');
  requireCondition(role === 'package' ? reference.expiresAt === null
    : Number.isSafeInteger(reference.expiresAt), 'REFERENCE_EXPIRY_INVALID');
}

export function checkReferenceRoles(value) {
  if (value === null || typeof value !== 'object') return;
  if (Object.hasOwn(value, 'envelopeSha256')) referenceScope(value);
  for (const child of Object.values(value)) checkReferenceRoles(child);
}

function checkUpdate(update, claims, now) {
  const candidate = update.endpointKind === 'candidate';
  requireCondition(update.activationMode === (update.upgradeType === 'silent' ? 'stagedRestart' : 'directInstall'),
    'UPGRADE_MODE_INVALID');
  if (!candidate) requireCondition(update.endpointChain.deploymentId === update.baselineDeploymentId
    && update.endpointChain.deploymentRevision === update.baselineRevision, 'BASELINE_BINDING_INVALID');
  if (update.endpointChain.deploymentId === update.hopChain.deploymentId) {
    requireCondition(sameJson(update.endpointChain, update.hopChain), 'ENTITY_CHAIN_MISMATCH');
  }
  requireCondition((update.migrationMode !== 'none') === (update.plans.migration !== null)
    && (update.backupPolicy === 'required') === (update.plans.backup !== null), 'PLAN_BINDING_INVALID');
  requireCondition(compareVersions(update.targetVersion, claims.currentVersion) > 0, 'TARGET_VERSION_NOT_HIGHER');
  requireCondition(update.endpointChain.deploymentState === 'active'
    && update.hopChain.deploymentState === (update.hopKind === 'bridge' ? 'superseded' : 'active'),
  'ENTITY_STATE_INVALID');
  requireCondition(candidate === (update.rollout !== null), 'ROLLOUT_BINDING_INVALID');
  requireCondition((update.hopKind === 'bridge') === (update.bridge !== null), 'BRIDGE_BINDING_INVALID');
  if (update.hopKind === 'bridge') {
    requireCondition(update.ordinaryPathApproval === null, 'PATH_BINDING_INVALID');
  } else {
    requireCondition(update.endpointChain.deploymentId === update.hopChain.deploymentId
      || (update.hopChain.deploymentId === update.baselineDeploymentId
        && update.hopChain.deploymentRevision === update.baselineRevision)
      || update.ordinaryPathApproval !== null, 'PATH_BINDING_MISSING');
  }
  requireCondition(update.migrationMode !== 'irreversible' || update.backupPolicy === 'required',
    'BACKUP_POLICY_INVALID');
  requireCondition(update.packageId === update.hopChain.packageId, 'PACKAGE_BINDING_INVALID');
  for (const chain of [update.endpointChain, update.hopChain]) {
    requireCondition(chain.releaseManifest.role === 'release'
      && chain.releaseManifest.scope.releaseId === chain.releaseId
      && chain.releaseManifest.version === chain.releaseRevision, 'RELEASE_BINDING_INVALID');
    requireCondition(chain.packageManifest.role === 'package', 'REFERENCE_ROLE_INVALID');
    requireCondition(chain.packageManifest.scope.targetKey === claims.scope.targetKey, 'TARGET_BINDING_INVALID');
    requireCondition(chain.installNotAfter === null
      || chain.installNotAfter > Math.max(chain.releaseVisibleAt, chain.installNotBefore), 'INSTALL_WINDOW_INVALID');
    requireCondition(now >= chain.releaseVisibleAt
      && (chain.installNotAfter === null || now < chain.installNotAfter), 'INSTALL_WINDOW_CLOSED');
  }
  if (candidate) {
    const { rollout, endpointChain } = update;
    requireCondition(rollout.opening.targetKey === claims.scope.targetKey
      && rollout.opening.deploymentId === endpointChain.deploymentId
      && rollout.opening.deploymentRevision === endpointChain.deploymentRevision
      && rollout.opening.rolloutId === rollout.rolloutId
      && rollout.opening.rolloutRevision === rollout.revision
      && equalDigest(rollout.opening.releaseEnvelopeSha256, endpointChain.releaseManifest.envelopeSha256),
    'OPENING_BINDING_INVALID');
    requireCondition(rollout.locatorExpiresAt > claims.iat
      && rollout.locatorExpiresAt <= Math.min(claims.exp, claims.iat + 900_000), 'LOCATOR_WINDOW_INVALID');
  }
}

export function checkSemantics(claims, { now, production = true, allowExpiredRoot = false } = {}) {
  requireCondition(Number.isSafeInteger(now) && now >= 0, 'TRUSTED_TIME_REQUIRED');
  checkReferenceRoles(claims);
  const { role } = claims;
  requireCondition(role === 'package' || Object.hasOwn(METADATA_LIFETIMES, role)
    || Object.hasOwn(CREDENTIAL_LIFETIMES, role), 'ROLE_UNKNOWN');
  if (Object.hasOwn(METADATA_LIFETIMES, role)) {
    if (allowExpiredRoot && role === 'root') {
      requireCondition(claims.expiresAt > claims.issuedAt
        && claims.expiresAt - claims.issuedAt <= METADATA_LIFETIMES.root
        && now >= claims.issuedAt, 'TIME_WINDOW_INVALID');
    } else assertWindow(claims.issuedAt, claims.expiresAt, METADATA_LIFETIMES[role], now);
  }
  if (role === 'timestamp') requireCondition(claims.snapshot.role === 'snapshot', 'REFERENCE_ROLE_INVALID');
  if (role === 'snapshot') {
    const identities = new Set();
    for (const entry of claims.entries) {
      requireCondition(['target', 'release', 'package'].includes(entry.role), 'REFERENCE_ROLE_INVALID');
      const identity = entry.role === 'package' ? 'package:' + entry.envelopeSha256
        : entry.role + ':' + Buffer.from(canonicalize(entry.scope)).toString('utf8');
      requireCondition(!identities.has(identity), 'REFERENCE_DUPLICATE');
      identities.add(identity);
    }
  }
  if (role === 'target') {
    requireCondition(claims.baselineRelease.role === 'release', 'REFERENCE_ROLE_INVALID');
    requireCondition(claims.ordinaryPaths.every((path) => path.release.role === 'release'), 'REFERENCE_ROLE_INVALID');
    requireCondition(claims.bridgePaths.every((path) => path.release.role === 'release'), 'REFERENCE_ROLE_INVALID');
    const expected = claims.scope.targetKey.startsWith('macos-') ? ['arm64', 'x86_64']
      : claims.scope.targetKey.includes('-arm64-') ? ['arm64'] : ['x86_64'];
    requireCondition(sameJson([...claims.hostArchitectures].sort(), expected), 'HOST_ARCH_INVALID');
  }
  if (role === 'release') {
    const targets = new Set();
    for (const target of claims.targets) {
      requireCondition(!targets.has(target.targetKey), 'TARGET_DUPLICATE');
      targets.add(target.targetKey);
      requireCondition(target.packageManifest.role === 'package'
        && target.packageManifest.scope.targetKey === target.targetKey, 'TARGET_BINDING_INVALID');
      requireCondition(target.migrationMode !== 'irreversible' || target.backupPolicy === 'required', 'BACKUP_POLICY_INVALID');
    }
  }
  if (role === 'package') {
    const helperIds = new Set();
    for (const helper of claims.helpers) {
      requireCondition(!helperIds.has(helper.helperId), 'HELPER_DUPLICATE');
      helperIds.add(helper.helperId);
      requireCondition(helper.targetKey === claims.scope.targetKey && helper.nativeIdentity.format === 'helper',
        'HELPER_TARGET_INVALID');
    }
    const helper = claims.helpers.find((entry) => entry.helperId === claims.executionHelperId);
    requireCondition(helper !== undefined && sameJson(helper.nativeIdentity, claims.helper), 'HELPER_BINDING_INVALID');
    requireCondition(claims.provenance.sourceRevision.value.length
      === (claims.provenance.sourceRevision.algorithm === 'git-sha1' ? 40 : 64), 'SOURCE_REVISION_INVALID');
    requireCondition(!production || claims.incrementalPackages.length === 0, 'INCREMENTAL_PRODUCTION_DISABLED');
    requireCondition(equalDigest(claims.provenance.finalInstallerSha256, claims.finalInstaller.sha256),
      'PROVENANCE_BINDING_INVALID');
    const format = claims.scope.targetKey.startsWith('windows-') ? 'nsis'
      : claims.scope.targetKey.startsWith('macos-') ? 'dmg' : 'deb';
    requireCondition(claims.finalInstaller.format === format && claims.helper.format === 'helper'
      && claims.launcher.format === 'launcher', 'NATIVE_FORMAT_INVALID');
  }
  if (!Object.hasOwn(CREDENTIAL_LIFETIMES, role)) return;
  assertWindow(claims.iat, claims.exp, CREDENTIAL_LIFETIMES[role], now);
  requireCondition(claims.nbf >= claims.iat && claims.nbf < claims.exp
    && now >= claims.nbf, 'TIME_WINDOW_CLOSED');
  if (role === 'decision') {
    requireCondition(claims.updateAvailable === (claims.update !== null)
      && claims.updateAvailable === (claims.reason === null), 'DECISION_BINDING_INVALID');
  }
  if (claims.metadataSet) {
    const expectedOs = claims.scope.targetKey.split('-')[0];
    const expectedArchitectures = expectedOs === 'macos' ? ['arm64', 'x86_64']
      : [claims.scope.targetKey.split('-')[1]];
    requireCondition(claims.host.os === expectedOs && expectedArchitectures.includes(claims.host.arch), 'HOST_TARGET_INVALID');
    const set = claims.metadataSet;
    for (const expectedRole of ['root', 'timestamp', 'snapshot', 'target']) {
      requireCondition(set[expectedRole].role === expectedRole
        && now < set[expectedRole].expiresAt, 'METADATA_SET_INVALID');
    }
    requireCondition(sameJson(set.target.scope, { channel: claims.scope.channel, targetKey: claims.scope.targetKey })
      && set.releases.every((reference) => reference.role === 'release' && now < reference.expiresAt),
    'METADATA_SET_INVALID');
    if (claims.update) {
      for (const chain of [claims.update.endpointChain, claims.update.hopChain]) {
        requireCondition(set.releases.some((reference) => sameJson(reference, chain.releaseManifest)),
          'METADATA_SET_INCOMPLETE');
        requireCondition(chain.supplyChain.effectiveExpiresAt === null
          || now < chain.supplyChain.effectiveExpiresAt, 'SUPPLY_CHAIN_EXPIRED');
      }
    }
  }
  if (claims.update) checkUpdate(claims.update, claims, now);
  if (role.startsWith('authorization-')) {
    const preinstall = claims.purpose === 'preinstall';
    requireCondition(claims.authorizationJti === claims.jti, 'AUTHORIZATION_ID_INVALID');
    requireCondition(preinstall ? claims.freezeEpoch === null && claims.backup === null
      : claims.freezeEpoch !== null, 'FREEZE_BINDING_INVALID');
    requireCondition(preinstall || (claims.update.backupPolicy === 'required') === (claims.backup !== null), 'BACKUP_BINDING_INVALID');
    if (claims.backup) requireCondition(claims.backup.freezeEpoch === claims.freezeEpoch
      && claims.backup.preparationEpoch === claims.preparation.ownerEpoch, 'BACKUP_BINDING_INVALID');
    requireCondition((claims.purpose === 'activate') === (claims.staged !== null), 'STAGED_BINDING_INVALID');
    requireCondition(preinstall === (claims.preinstallSlot !== null), 'SLOT_BINDING_INVALID');
    requireCondition(claims.purpose === 'install' ? claims.update.upgradeType !== 'silent'
      : claims.update.upgradeType === 'silent', 'UPGRADE_PURPOSE_INVALID');
    if (preinstall || claims.purpose === 'activate') {
      requireCondition(claims.update.activationMode === 'stagedRestart', 'ACTIVATION_MODE_INVALID');
    } else requireCondition(claims.update.activationMode === 'directInstall', 'ACTIVATION_MODE_INVALID');
    for (const chain of [claims.update.endpointChain, claims.update.hopChain]) {
      if (!preinstall) requireCondition(now >= chain.installNotBefore, 'INSTALL_NOT_YET_VALID');
      requireCondition(now >= chain.releaseVisibleAt
        && (chain.installNotAfter === null || now < chain.installNotAfter), 'INSTALL_WINDOW_CLOSED');
    }
    const chain = claims.update.hopChain;
    if (claims.staged) {
      requireCondition(claims.transactionId !== claims.staged.preinstallTransactionId, 'STAGED_TRANSACTION_REUSED');
      requireCondition(claims.staged.stagedAt <= claims.iat
        && Number.isSafeInteger(claims.staged.stagedAt + 30 * DAY)
        && claims.staged.stagedValidUntil === Math.min(claims.staged.stagedAt + 30 * DAY,
          chain.installNotAfter ?? Number.MAX_SAFE_INTEGER)
        && claims.staged.stagedValidUntil > claims.staged.stagedAt
        && now < claims.staged.stagedValidUntil, 'STAGED_WINDOW_INVALID');
    }
  }
  if (role === 'download') {
    requireCondition(claims.packageId === claims.update.hopChain.packageId
      && sameJson(claims.package, claims.update.hopChain.package), 'PACKAGE_BINDING_INVALID');
  }
  if (role.endsWith('-event')) {
    const transaction = role.includes('-transaction-');
    const anchor = transaction ? claims.transactionStartedAt : claims.sessionStartedAt ?? claims.taskStartedAt;
    requireCondition(claims.iat >= anchor && claims.nbf >= anchor
      && claims.eventNotAfter > claims.nbf
      && claims.eventNotAfter <= anchor + (transaction ? 30 * DAY : DAY)
      && claims.exp <= anchor + (transaction ? 37 * DAY : 7 * DAY), 'EVENT_WINDOW_INVALID');
  }
  if (claims.task) {
    requireCondition(claims.task.taskPurpose === claims.purpose
      && claims.task.windowEnd > claims.task.windowStart, 'TASK_BINDING_INVALID');
    const task = claims.task;
    requireCondition(task.packageManifest.role === 'package'
      && task.packageManifest.scope.targetKey === claims.scope.targetKey
      && sameJson(task.packageManifest, task.hopChain.packageManifest)
      && task.packageId === task.hopChain.packageId && sameJson(task.package, task.hopChain.package), 'TASK_TARGET_INVALID');
    requireCondition(compareVersions(task.targetVersion, task.sourceVersion) > 0
      && task.activationMode === (task.upgradeType === 'silent' ? 'stagedRestart' : 'directInstall')
      && (task.originalPurpose === 'install' ? task.upgradeType !== 'silent' : task.upgradeType === 'silent'),
      'TASK_TARGET_INVALID');
    requireCondition(task.taskPurpose !== 'reconcile' || task.retirementProofSha256 !== null, 'TASK_PROOF_MISSING');
    if (role === 'recovery-review-event' || role === 'reconciliation-event') {
      requireCondition(claims.nbf >= claims.task.windowStart
        && claims.eventNotAfter <= claims.task.windowEnd, 'TASK_WINDOW_INVALID');
    }
  }
}

export function assertOccurrence(claims, occurredAt, receivedAt) {
  checkSemantics(claims, { now: receivedAt });
  requireCondition(Number.isSafeInteger(occurredAt) && occurredAt >= claims.nbf
    && occurredAt < claims.eventNotAfter && occurredAt <= receivedAt, 'EVENT_OCCURRENCE_INVALID');
}

export function assertOpening(target, releaseEnvelopeHash, opening) {
  requireCondition(opening !== null && opening.targetKey === target.scope.targetKey
    && equalDigest(opening.releaseEnvelopeSha256, releaseEnvelopeHash)
    && equalDigest(rolloutCommitment(opening), target.rolloutSetCommitment), 'OPENING_MISMATCH');
}

export function selectFullPackage(packageManifest, capabilities) {
  requireCondition(Array.isArray(capabilities) && capabilities.length === 0, 'INCREMENTAL_CAPABILITY_DISABLED');
  return packageManifest.fullPackage;
}

export function consumeQueryAvailable(authorizationExp, now) {
  requireCondition(Number.isSafeInteger(authorizationExp) && authorizationExp >= 0
    && Number.isSafeInteger(now) && now >= 0 && Number.isSafeInteger(authorizationExp + 61 * DAY), 'TRUSTED_TIME_REQUIRED');
  return now < authorizationExp + 61 * DAY;
}

export function outcomeRetentionRequired(authorizationExp, now) {
  requireCondition(Number.isSafeInteger(authorizationExp) && authorizationExp >= 0
    && Number.isSafeInteger(now) && now >= 0 && Number.isSafeInteger(authorizationExp + 62 * DAY), 'TRUSTED_TIME_REQUIRED');
  return now < authorizationExp + 62 * DAY;
}
