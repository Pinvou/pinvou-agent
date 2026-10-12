// The frontend's consent-failure matcher must
// key on the exact shared marker string the Rust emitters pin
// (scope::CONSENT_SYNC_FAILURE_MARKER — asserted on the Rust side by
// skill_gate_consent_failure_message_keeps_the_frontend_marker in
// skill_gate.rs). A backend or frontend rewording must move both sides in the
// same commit; this source-text assertion is the frontend leg of that pin.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
// The production cut is anchored at the `#[cfg(test)] mod tests` boundary and
// fails closed: scope.rs scatters test-only `#[cfg(test)] fn` items before its
// test module, and a file with no marker at all must not degrade to scanning
// test code as production.
const productionOf = (src) => {
  const idx = src.search(/#\[cfg\(test\)\]\s*mod tests\b/);
  assert.ok(idx > 0, 'Rust source must keep its test module after the production body');
  return src.slice(0, idx);
};

// The literal lives ONCE, in connector-ui-state.js
// (mirroring scope::CONSENT_SYNC_FAILURE_MARKER); the three JS matchers import
// the const. The pin asserts the single definition and that every matcher
// site imports it — one-site drift cannot survive.
const uiState = readFileSync(
  join(root, 'src/features/tools/connector-ui-state.js'),
  'utf8',
);
const MARKER = 'persisting their default-off consent state failed';
assert.ok(
  uiState.replace(/\s+/g, ' ').includes(`export const CONSENT_SYNC_FAILURE_MARKER = '${MARKER}'`),
  'connector-ui-state.js must define the shared consent marker exactly once (update the Rust pin in the same commit if reworded)',
);
const store = readFileSync(
  join(root, 'src/features/tools/ToolStoreView.jsx'),
  'utf8',
);
assert.ok(
  !store.includes(MARKER),
  'the marker literal must be defined only in connector-ui-state.js (import the const instead)',
);
assert.ok(
  store.includes('CONSENT_SYNC_FAILURE_MARKER'),
  'ToolStoreView must key the consent-failure surfacing on the shared const',
);

// A backend-only reword (Rust const + updated Rust value test) would otherwise
// pass every leg while ToolStoreView's includes() silently stopped matching.
// Cross-compare: extract the Rust constant's VALUE from scope.rs and require
// it to equal the JS literal — a reword fails one side or the other until both
// move.
const rustDir = join(root, 'src-tauri/src/features');
const scopeSrc = readFileSync(join(rustDir, 'marketplace/scope.rs'), 'utf8');
// Anchored to a top-level declaration so comment/string mentions cannot satisfy the extraction.
const rustMarker = scopeSrc.match(
  /^pub(?:\(crate\))?\s+const CONSENT_SYNC_FAILURE_MARKER:\s*&str\s*=\s*"([^"]+)"/m,
)?.[1];
assert.ok(
  rustMarker,
  'scope.rs must define CONSENT_SYNC_FAILURE_MARKER as a plain string literal',
);
assert.strictEqual(
  rustMarker,
  MARKER,
  'the Rust marker constant and the frontend matcher literal have drifted — reword both sides in the same commit',
);

// The consent-failure copy is SINGLE-SOURCED: the sentence lives once, inside
// the shared scope.rs helper consent_sync_failure_message(subject, error)
// which interpolates the marker between the subject and the fixed tail; every
// emit site (the marketplace command layer and the skill gate) calls the
// helper instead of hand-rolling the wording. The helper IS the marker
// interpolation, so a wording without the marker is impossible by
// construction, one-site drift cannot survive, and any hand-rolled copy fails
// the exact-count legs below. The per-channel call counts are LOWER BOUNDS so
// deleting an emit site still fails a leg while a legitimately added channel
// routed through the helper does not.
const TEMPLATE_TAIL = 'new sessions will enable it by default';

// (a) The template tail must exist exactly once in scope.rs PRODUCTION —
// inside the shared helper.
const scopeProduction = productionOf(scopeSrc);
assert.ok(
  scopeProduction.split(TEMPLATE_TAIL).length - 1 === 1,
  'the consent-failure template must exist exactly once in scope.rs production — inside the shared helper',
);

// (a2) Every leg above survives a helper-body reword that drops the
// interpolation (const defined, tail present once, call counts intact) while
// ToolStoreView's includes() silently stops matching and every consent
// failure degrades to the generic skills_enable_failed code — exactly the
// degradation this file exists to prevent. Pin the interpolation from the
// helper's declaration to the end of production: a dropped interpolation
// fails here by construction.
const helperStart = scopeProduction.indexOf('fn consent_sync_failure_message');
assert.ok(helperStart > 0, 'scope.rs must define the shared consent_sync_failure_message helper in production');
assert.ok(
  scopeProduction.slice(helperStart).includes('{CONSENT_SYNC_FAILURE_MARKER}'),
  'consent_sync_failure_message must interpolate {CONSENT_SYNC_FAILURE_MARKER} — a wording without it degrades every consent failure to the generic error code',
);

// (b)/(c) The marketplace command layer (the post-landing consent-failure
// channels: tool/skill/import) and the skill gate route every emit site
// through the shared helper (lower bound 1) and NONE of them may hand-roll
// the template — the tail exists exactly once system-wide, inside the helper
// ((a)). A hand-rolled copy (marker interpolated or not) fails here; deleting
// the emit site drops the lower-bound leg.
const assertRoutesThroughSharedHelper = (label, src) => {
  const production = productionOf(src);
  const calls = production.split('consent_sync_failure_message(').length - 1;
  assert.ok(
    calls >= 1,
    `${label}.rs emit sites must route through consent_sync_failure_message (found ${calls}, expected >= 1)`,
  );
  const copies = production.split(TEMPLATE_TAIL).length - 1;
  assert.ok(
    copies === 0,
    `${label}.rs must not hand-roll the consent-failure template (found ${copies}, expected 0) — every channel must call the shared helper`,
  );
};

// (b) The marketplace command layer.
const marketplaceSrc = readFileSync(
  join(root, 'src-tauri/src/app/commands/marketplace.rs'),
  'utf8',
);
assertRoutesThroughSharedHelper('marketplace', marketplaceSrc);

// (c) The skill-gate channel's Rust leg.
const skillGateSrc = readFileSync(join(rustDir, 'connectors/skill_gate.rs'), 'utf8');
// Pin the Rust-side VALUE test by name — a rename or
// rewrite that left the constant referenced only from a comment would
// otherwise hollow this leg silently.
assert.ok(
  skillGateSrc.includes('fn skill_gate_consent_failure_message_keeps_the_frontend_marker'),
  'skill_gate.rs must keep the Rust-side value contract test this leg leans on (fn skill_gate_consent_failure_message_keeps_the_frontend_marker)',
);
assertRoutesThroughSharedHelper('skill_gate', skillGateSrc);

console.log('consent_marker_frontend: ok');
