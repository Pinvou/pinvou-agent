import { requireCondition } from '../errors.mjs';
import { sameJson } from '../semantics.mjs';

export function assertCount(value) {
  requireCondition(Number.isSafeInteger(value) && value >= 0, 'MODEL_QUALITY_INVALID');
  return BigInt(value);
}

/** Exact rational arithmetic; no floating rounding can turn equality into pass.
 * thresholds are approved ratios, not an implementation-wide percentage default.
 */
export function metricAssessment({ succeeded, failed, incomplete, unknown }, { minimumSamples, threshold }) {
  const success = assertCount(succeeded); const failure = assertCount(failed);
  const pending = assertCount(incomplete) + assertCount(unknown);
  const minimum = assertCount(minimumSamples);
  requireCondition(minimum > 0n && Number.isSafeInteger(threshold.numerator)
    && Number.isSafeInteger(threshold.denominator) && threshold.numerator > 0
    && threshold.numerator <= threshold.denominator, 'MODEL_THRESHOLD_INVALID');
  const n = BigInt(threshold.numerator); const d = BigInt(threshold.denominator);
  const confirmed = success + failure; const total = confirmed + pending;
  const below = (count, denominator) => denominator > 0n && count * d < denominator * n;
  return { sufficient: confirmed >= minimum, realBelow: below(failure, confirmed),
    upperBelow: below(failure + pending, total),
    confirmedFailureThresholdReached: confirmed > 0n && !below(failure, confirmed),
    unknownFailureThresholdReached: unknown > 0 && !below(failure + pending, total) };
}

export function assessStage(plan, observation, now, requiredGroupIdentities) {
  requireCondition(Number.isSafeInteger(now) && Number.isSafeInteger(observation.startedAt)
    && now >= observation.startedAt && Number.isSafeInteger(plan.minimumObservationMs)
    && plan.minimumObservationMs > 0, 'MODEL_OBSERVATION_INVALID');
  const timeComplete = now - observation.startedAt >= plan.minimumObservationMs;
  requireCondition(Array.isArray(plan.requiredMetrics) && plan.requiredMetrics.length > 0
    && new Set(plan.requiredMetrics).size === plan.requiredMetrics.length, 'MODEL_QUALITY_INCOMPLETE');
  requireCondition(observation.groups.length > 0
    && new Set(observation.groups.map((group) => group.groupIdentity)).size === observation.groups.length,
  'MODEL_SAMPLES_INSUFFICIENT');
  requireCondition(Array.isArray(requiredGroupIdentities) && requiredGroupIdentities.length > 0
    && new Set(requiredGroupIdentities).size === requiredGroupIdentities.length
    && sameJson([...requiredGroupIdentities].sort(), observation.groups.map((group) => group.groupIdentity).sort()),
  'MODEL_QUALITY_INCOMPLETE');
  let passed = timeComplete; let freeze = false;
  for (const group of observation.groups) {
    for (const metric of plan.requiredMetrics) {
      requireCondition(Object.hasOwn(group.metrics, metric) && Object.hasOwn(plan.metrics, metric), 'MODEL_QUALITY_INCOMPLETE');
      const assessment = metricAssessment(group.metrics[metric], plan.metrics[metric]);
      passed &&= assessment.sufficient && assessment.realBelow && assessment.upperBelow;
      freeze ||= assessment.confirmedFailureThresholdReached || assessment.unknownFailureThresholdReached;
    }
  }
  return { passed, freeze, timeComplete };
}

/** Independent projection: accepted late evidence can resolve an unknown result;
 * confirmed failure survives retries, repair and new health checks.
 */
export function projectEvidence(sample, event) {
  requireCondition(event.workflowId === sample.workflowId && event.groupIdentity === sample.groupIdentity
    && event.targetIdentitySha256 === sample.targetIdentitySha256 && event.installationScopeId === sample.installationScopeId
    && event.metric === sample.metric && event.windowId === sample.windowId
    && Number.isSafeInteger(event.contributionWatermark) && event.contributionWatermark > 0,
  'MODEL_QUALITY_BINDING_INVALID');
  const previous = sample.events.find((item) => item.eventId === event.eventId);
  if (previous !== undefined) {
    requireCondition(previous.sha256 === event.sha256, 'IDEMPOTENCY_CONFLICT');
    return structuredClone(sample);
  }
  const result = structuredClone(sample); result.events.push({ eventId: event.eventId, sha256: event.sha256 });
  requireCondition(['succeeded', 'failed', 'incomplete', 'outcome_unknown'].includes(event.outcome), 'MODEL_QUALITY_INVALID');
  // A genuine original-result failure remains evidence even when a worker
  // processes it after a higher watermark. Older progress cannot overwrite it.
  if (event.outcome === 'failed' && !['higher-version-repair', 'health-observation'].includes(event.kind)) {
    result.outcome = 'failed'; result.lastContributionWatermark = Math.max(sample.lastContributionWatermark, event.contributionWatermark);
    return result;
  }
  if (event.contributionWatermark <= sample.lastContributionWatermark) return result;
  result.lastContributionWatermark = event.contributionWatermark;
  if (sample.outcome === 'failed') return result;
  if (event.kind === 'higher-version-repair' || event.kind === 'health-observation') return result;
  if (sample.outcome === 'succeeded' && ['incomplete', 'outcome_unknown'].includes(event.outcome)) return result;
  result.outcome = event.outcome;
  return result;
}
