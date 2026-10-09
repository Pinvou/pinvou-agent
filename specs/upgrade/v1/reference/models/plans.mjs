import { requireCondition } from '../errors.mjs';
import { assertApiShape } from '../api-registry.mjs';

/** Deterministic wire-to-frozen projection. The approving body is the exact
 * returned plan, including both SN references and observation intervals.
 */
export function freezeStagePlan(wirePlan, upgradeType) {
  assertApiShape('stage-plan', wirePlan);
  requireCondition(['normal', 'forced', 'silent'].includes(upgradeType), 'MODEL_PLAN_INVALID');
  const requiredMetrics = ['download', 'verification', 'installation', 'health',
    ...(upgradeType === 'silent' ? ['preinstallation', 'activation'] : [])];
  const result = { ...structuredClone(wirePlan), upgradeType, stages: wirePlan.stages.map((stage) => ({
    stageId: stage.stageId, percentage: stage.percentage, minimumObservationMs: stage.minimumObservationMs,
    requiredMetrics, metrics: Object.fromEntries(requiredMetrics.map((metric) => {
      requireCondition(stage.thresholds[metric] !== null, 'MODEL_PLAN_INVALID');
      return [metric, { minimumSamples: stage.minimumSamples, threshold: structuredClone(stage.thresholds[metric]) }];
    })),
  })) };
  assertStagePlan(result); return result;
}

export function assertStagePlan(plan) {
  assertApiShape('frozen-stage-plan', plan);
  requireCondition(new Set(plan.stages.map((stage) => stage.stageId)).size === plan.stages.length,
    'MODEL_PLAN_INVALID');
  let percentage = 0;
  const expected = ['download', 'verification', 'installation', 'health',
    ...(plan.upgradeType === 'silent' ? ['preinstallation', 'activation'] : [])].sort();
  for (const stage of plan.stages) {
    requireCondition(stage.percentage > percentage && stage.requiredMetrics.length > 0
      && JSON.stringify([...stage.requiredMetrics].sort()) === JSON.stringify(expected)
      && Object.keys(stage.metrics).length === stage.requiredMetrics.length, 'MODEL_PLAN_INVALID');
    for (const metric of stage.requiredMetrics) {
      const definition = stage.metrics[metric];
      requireCondition(definition !== undefined && definition.threshold.numerator <= definition.threshold.denominator,
        'MODEL_PLAN_INVALID');
    }
    percentage = stage.percentage;
  }
  requireCondition(percentage === 100 && plan.lossIntervalMs >= plan.progressReportIntervalMs, 'MODEL_PLAN_INVALID');
}
