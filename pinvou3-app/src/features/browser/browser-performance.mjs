const MAX_SAMPLES_PER_METRIC = 200;
const MIN_GATE_SAMPLES = 30;

export const BROWSER_PERFORMANCE_THRESHOLDS_MS = Object.freeze({
  dock_surface_show_ms: 100,
  workspace_restore_status_ms: 100,
  tab_switch_ms: 100,
  agent_target_alignment_ms: 200,
});

const samples = new Map();

export function browserPerformanceNow() {
  return globalThis.performance && typeof globalThis.performance.now === 'function'
    ? globalThis.performance.now()
    : Date.now();
}

export function percentile(values, quantile) {
  if (!Array.isArray(values) || values.length === 0) return null;
  const ordered = values.slice().sort((a, b) => a - b);
  const rank = Math.max(0, Math.ceil(quantile * ordered.length) - 1);
  return ordered[Math.min(rank, ordered.length - 1)];
}

export function recordBrowserPerformance(metric, durationMs) {
  if (typeof metric !== 'string' || !metric) return;
  const value = Number(durationMs);
  if (!Number.isFinite(value) || value < 0) return;
  const metricSamples = samples.get(metric) || [];
  metricSamples.push(value);
  if (metricSamples.length > MAX_SAMPLES_PER_METRIC) {
    metricSamples.splice(0, metricSamples.length - MAX_SAMPLES_PER_METRIC);
  }
  samples.set(metric, metricSamples);
}

export function resetBrowserPerformance() {
  samples.clear();
}

export function browserPerformanceSnapshot() {
  const metrics = {};
  const names = new Set([
    ...Object.keys(BROWSER_PERFORMANCE_THRESHOLDS_MS),
    ...samples.keys(),
  ]);
  for (const name of names) {
    const values = samples.get(name) || [];
    const thresholdMs = BROWSER_PERFORMANCE_THRESHOLDS_MS[name] ?? null;
    const p95Ms = percentile(values, 0.95);
    metrics[name] = {
      count: values.length,
      p50Ms: percentile(values, 0.5),
      p95Ms,
      maxMs: values.length ? Math.max(...values) : null,
      thresholdMs,
      // 少量手工点击不能冒充 P95 门禁通过；至少 30 个样本才给出布尔结论。
      passes: thresholdMs == null || values.length < MIN_GATE_SAMPLES
        ? null
        : p95Ms < thresholdMs,
    };
  }
  return {
    generatedAt: new Date().toISOString(),
    minGateSamples: MIN_GATE_SAMPLES,
    metrics,
  };
}

if (typeof window !== 'undefined') {
  Object.defineProperty(window, '__PINVOU_BROWSER_PERF__', {
    configurable: true,
    value: Object.freeze({
      snapshot: browserPerformanceSnapshot,
      reset: resetBrowserPerformance,
    }),
  });
}
