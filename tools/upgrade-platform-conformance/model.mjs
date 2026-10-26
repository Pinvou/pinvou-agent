import { operations, checkObservation } from '../../specs/upgrade/v1/platforms/contract.mjs';
import { vectors } from '../../specs/upgrade/v1/platforms/vectors.mjs';

export function runModelCases() {
  const input = vectors(), names = input.map(value => value.record.operation);
  if (names.length !== new Set(names).size || names.length !== Object.keys(operations).length
    || !Object.keys(operations).every(name => names.includes(name))) throw new Error('CONFORMANCE_VECTOR_COVERAGE');
  const results = [];
  for (const { record, invalid: [field, value] } of input) {
    checkObservation(record);
    const negative = structuredClone(record); negative.result.facts[field] = value;
    let rejected = false;
    try { checkObservation(negative); } catch (error) { if (error.code === 'PLATFORM_FACT_INVALID') rejected = true; else throw error; }
    if (!rejected) throw new Error('CONFORMANCE_NEGATIVE_ACCEPTED');
    results.push({ id: `${record.operation}.accepted-and-counterexample`, operation: record.operation,
      provenance: 'contract-model', status: 'passed' });
  }
  return results;
}
