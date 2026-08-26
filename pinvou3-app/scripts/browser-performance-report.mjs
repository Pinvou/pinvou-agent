#!/usr/bin/env node
import { readFileSync } from 'node:fs';

const THRESHOLDS_MS = {
  dock_surface_show_ms: 100,
  workspace_restore_status_ms: 100,
  tab_switch_ms: 100,
  agent_target_alignment_ms: 200,
};

function percentile(values, quantile) {
  if (!values.length) return null;
  const ordered = [...values].sort((a, b) => a - b);
  return ordered[Math.max(0, Math.ceil(ordered.length * quantile) - 1)];
}

const paths = process.argv.slice(2).filter((arg) => !arg.startsWith('--'));
const minSamplesArg = process.argv.find((arg) => arg.startsWith('--min-samples='));
const parsedMinSamples = Number(minSamplesArg?.split('=')[1] || 30);
const minSamples = Number.isInteger(parsedMinSamples) && parsedMinSamples > 0
  ? parsedMinSamples
  : 30;
const input = paths.length
  ? paths.map((path) => readFileSync(path, 'utf8')).join('\n')
  : readFileSync(0, 'utf8');
const grouped = new Map();

for (const line of input.split(/\r?\n/)) {
  const marker = line.indexOf('[browser-perf] ');
  const encoded = marker >= 0 ? line.slice(marker + '[browser-perf] '.length) : line;
  try {
    const sample = JSON.parse(encoded);
    if (typeof sample.metric !== 'string' || !Number.isFinite(sample.durationMs)) continue;
    const values = grouped.get(sample.metric) || [];
    values.push(sample.durationMs);
    grouped.set(sample.metric, values);
  } catch {
    // Ordinary application logs are not performance samples; ignore them.
  }
}

let failed = grouped.size === 0;
const report = {};
for (const [metric, values] of grouped) {
  const thresholdMs = THRESHOLDS_MS[metric] ?? null;
  const p95Ms = percentile(values, 0.95);
  const enoughSamples = values.length >= minSamples;
  const passes = enoughSamples && (thresholdMs == null || p95Ms < thresholdMs);
  if (!passes) failed = true;
  report[metric] = { count: values.length, p95Ms, thresholdMs, passes };
}

process.stdout.write(`${JSON.stringify({ minSamples, metrics: report }, null, 2)}\n`);
process.exitCode = failed ? 1 : 0;
