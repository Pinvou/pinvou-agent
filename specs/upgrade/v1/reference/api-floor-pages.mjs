// Private floor pages have at most 32 independently bounded records plus a
// bounded response frame. Their 33 MiB transport is not the 32 MiB source union,
// and never raises the limits of metadata, credentials, requests or other APIs.
import { canonicalize } from './canonical-json.mjs';

// Called only after the selected fixed response passed its closed JSON Schema.
// value is a fresh ordinary JSON copy produced from the shared strict parser.
export function assertFloorPageBounds(value) {
  const result = { ...value.result };
  const records = [];
  if (result.current !== undefined && result.current !== null) { records.push(result.current); result.current = null; }
  if (result.record !== undefined && result.record !== null) { records.push(result.record); result.record = null; }
  if (Array.isArray(result.records)) { records.push(...result.records); result.records = []; }
  for (const record of records) canonicalize(record);
  if (result.request !== undefined && result.request !== null) { canonicalize(result.request); result.request = null; }
  canonicalize({ ...value, result });
}
