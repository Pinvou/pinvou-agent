import { requireCondition } from '../errors.mjs';
import { compareVersions } from '../semantics.mjs';
import { assertInteger, objectHash } from './inputs.mjs';
import { assertApiShape } from '../api-registry.mjs';

/** T17 consumes a stable pseudonymous bucket from its private bucketing port.
 * This contract freezes precedence/boundaries, not a plaintext SN wire field.
 * The bucket is stable for the entire rollout and lies in [0,10000).
 */
export function candidateHit({ percentage, snPresent, included, excluded, bucketBasisPoints }) {
  assertInteger(percentage, 1); requireCondition(percentage <= 100, 'MODEL_PERCENTAGE_INVALID');
  requireCondition([snPresent, included, excluded].every((value) => typeof value === 'boolean'), 'MODEL_INPUT_INVALID');
  if (!snPresent) return percentage === 100;
  if (excluded) return false;
  if (included) return true;
  assertInteger(bucketBasisPoints); requireCondition(bucketBasisPoints < 10_000, 'MODEL_BUCKET_INVALID');
  return bucketBasisPoints < percentage * 100;
}

function nodeEligible(node, channel, now, endpoint = false) {
  return node.channel === channel && node.releaseState === 'closed' && node.releaseTargetState === 'approved'
    && node.certificationState === 'certified'
    && node.artifactState === 'valid' && node.supplyChainState === 'approved'
    && (node.supplyChainExpiresAt === null || now < node.supplyChainExpiresAt)
    && now >= node.releaseVisibleAt && (node.installNotAfter === null || now < node.installNotAfter)
    && (endpoint || node.hopKind === 'ordinary' ? node.deploymentState === 'active'
      && (endpoint || node.isBaseline === true || node.ordinaryPathApproval === 'approved')
      && !['paused', 'aborted'].includes(node.ownRolloutState)
      : node.hopKind === 'bridge' && node.deploymentState === 'superseded' && node.bridgeState === 'enabled');
}

/** Pure initial selection model. The production owner must supply nodes from
 * its complete consistent chain projection, never from the HTTP request.
 * No candidate identifiers are returned unless the private match succeeds.
 */
export function selectNextHop(input) {
  assertApiShape('selection-input', input);
  const { currentVersion, supportFloorVersion, channel, now, profiles, stableFactsSha256, baselineId, candidateId,
    rollout, nodes } = input;
  assertInteger(now);
  requireCondition(candidateId === null || rollout !== null, 'MODEL_CANDIDATE_INVALID');
  if (compareVersions(currentVersion, supportFloorVersion) < 0) return { reason: 'source_below_support_floor' };
  const matches = profiles.filter((profile) => profile.stableFactsSha256 === stableFactsSha256);
  if (matches.length !== 1) return { reason: matches.length === 0 ? 'source_profile_unknown' : 'source_profile_ambiguous' };
  const profile = matches[0];
  requireCondition(profile.state === 'forward-only' ? profile.policy !== null : profile.policy === null, 'MODEL_POLICY_INVALID');
  if (!['selectable', 'forward-only'].includes(profile.state)) return { reason: profile.state === 'revoked' ? 'source_revoked' : 'source_repair_only' };
  requireCondition(new Set(nodes.map((node) => node.deploymentId)).size === nodes.length, 'MODEL_PATH_AMBIGUOUS');
  const baseline = nodes.find((node) => node.deploymentId === baselineId);
  requireCondition(baseline !== undefined, 'MODEL_BASELINE_INVALID');
  let endpoint = baseline; let endpointKind = 'baseline';
  if (candidateId !== null && candidateHit(rollout)) {
    const candidate = nodes.find((node) => node.deploymentId === candidateId);
    requireCondition(candidate !== undefined && nodeEligible(candidate, channel, now, true)
      && candidate.ownRolloutState === 'running' && compareVersions(candidate.version, baseline.version) > 0,
    'MODEL_CANDIDATE_INVALID');
    endpoint = candidate; endpointKind = 'candidate';
  }
  requireCondition(nodeEligible(endpoint, channel, now, true), 'MODEL_BASELINE_INVALID');
  if (compareVersions(currentVersion, endpoint.version) >= 0) return { reason: 'no_higher_version' };
  const eligible = nodes.filter((node) => nodeEligible(node, channel, now, node.deploymentId === endpoint.deploymentId)
    && node.activationMode === endpoint.activationMode
    && (endpoint.upgradeType !== 'forced' || node.releaseVisibleAt <= endpoint.installNotBefore
      && node.installNotBefore <= endpoint.installNotBefore && (endpoint.installNotAfter === null ? node.installNotAfter === null
        : node.installNotAfter === null || node.installNotAfter >= endpoint.installNotAfter))
    && compareVersions(node.version, currentVersion) > 0 && compareVersions(node.version, endpoint.version) <= 0);
  // Strictly increasing versions form a DAG. Work backward from the endpoint;
  // bounded valid chains must not depend on the JavaScript call-stack limit.
  const reachable = new Map();
  const descending = [...eligible].sort((left, right) => compareVersions(right.version, left.version));
  let minimumReachableSource = endpoint.minimumSourceVersion;
  for (let index = 0; index < descending.length;) {
    const version = descending[index].version;
    let end = index + 1;
    while (end < descending.length && descending[end].version === version) end++;
    const canReach = compareVersions(version, minimumReachableSource) >= 0;
    reachable.set(version, canReach);
    // Equal-version nodes cannot reach one another. Incorporate this complete
    // group only after deciding it against strictly higher reachable versions.
    if (canReach) for (let member = index; member < end; member++) {
      if (compareVersions(descending[member].minimumSourceVersion, minimumReachableSource) < 0)
        minimumReachableSource = descending[member].minimumSourceVersion;
    }
    index = end;
  }
  const hops = eligible.filter((node) => compareVersions(currentVersion, node.minimumSourceVersion) >= 0 && reachable.get(node.version))
    .sort((left, right) => compareVersions(right.version, left.version));
  if (hops.length === 0) return { reason: 'source_path_unavailable' };
  requireCondition(hops.length < 2 || hops[0].version !== hops[1].version, 'MODEL_PATH_AMBIGUOUS');
  const hop = hops[0];
  if (profile.state === 'forward-only') {
    requireCondition(profile.policy !== null && profile.policy.sha256 === objectHash(profile.policy.edges), 'MODEL_POLICY_INVALID');
    const edges = profile.policy.edges.filter((edge) => edge.fromSourceProfileId === profile.sourceProfileId
      && edge.releaseTargetId === hop.releaseTargetId && edge.releaseTargetRevision === hop.releaseTargetRevision
      && edge.toCanonicalAppVersion === hop.version && edge.packageId === hop.packageId
      && edge.packageSha256 === hop.packageSha256 && edge.transformId === hop.transformId
      && edge.transformRevision === hop.transformRevision && edge.allowedChannels.includes(channel));
    if (edges.length === 0) return { reason: 'source_forward_path_unavailable' };
    requireCondition(edges.length === 1, 'MODEL_POLICY_AMBIGUOUS');
  }
  return { endpointKind, hopKind: hop.hopKind, endpointId: endpoint.deploymentId, hopId: hop.deploymentId,
    sourceProfileId: profile.sourceProfileId };
}

export function assertForcedPathWindow(endpoint, hops) {
  requireCondition(endpoint.upgradeType === 'forced' && hops.length > 0, 'MODEL_FORCED_PATH_INVALID');
  for (const hop of hops) requireCondition(hop.activationMode === 'directInstall' && hop.certificationState === 'certified'
    && hop.releaseVisibleAt <= endpoint.installNotBefore && hop.installNotBefore <= endpoint.installNotBefore
    && (endpoint.installNotAfter === null ? hop.installNotAfter === null
      : hop.installNotAfter === null || hop.installNotAfter >= endpoint.installNotAfter), 'MODEL_FORCED_PATH_INVALID');
}
