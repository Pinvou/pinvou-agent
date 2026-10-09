// Round-33 MAJOR 2 (review #455): the frontend's consent-failure matcher must
// key on the exact shared marker string the Rust emitters pin
// (scope::CONSENT_SYNC_FAILURE_MARKER — asserted on the Rust side by
// consent_failure_marker_matches_the_frontend_contract in ima.rs and
// skill_gate_consent_failure_message_keeps_the_frontend_marker in
// skill_gate.rs). A backend or frontend rewording must move both sides in the
// same commit; this source-text assertion is the frontend leg of that pin.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

// Round-37 F3 (review #455): the literal lives ONCE, in connector-ui-state.js
// (mirroring scope::CONSENT_SYNC_FAILURE_MARKER); the three JS matchers import
// the const. The pin asserts the single definition and that every matcher
// site imports it — one-site drift cannot survive.
const uiState = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '../src/features/tools/connector-ui-state.js'),
  'utf8',
);
const MARKER = 'persisting their default-off consent state failed';
assert.ok(
  uiState.includes(`export const CONSENT_SYNC_FAILURE_MARKER =\n  '${MARKER}'`)
    || uiState.includes(`export const CONSENT_SYNC_FAILURE_MARKER = '${MARKER}'`),
  'connector-ui-state.js must define the shared consent marker exactly once (update the Rust pin in the same commit if reworded)',
);
const store = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '../src/features/tools/ToolStoreView.jsx'),
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

// Round-19 review: the two sides above were independent frozen strings, so a
// backend-only reword (Rust const + updated Rust value test) passed every leg
// while ToolStoreView's includes() silently stopped matching. Cross-compare:
// extract the Rust constant's VALUE from scope.rs and require it to equal the
// JS literal — a reword now fails one side or the other until both move.
const rustDir = join(dirname(fileURLToPath(import.meta.url)), '../src-tauri/src/features');
const scopeSrc = readFileSync(join(rustDir, 'marketplace/scope.rs'), 'utf8');
// Round-20 review: anchor the extraction to a top-level declaration — an
// unanchored first match would silently validate a hypothetical
// `SOMEPREFIX_CONSENT_SYNC_FAILURE_MARKER` defined earlier in the file
// instead of the real constant.
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

// Round-19 review: the marketplace command layer carries the post-landing
// consent-failure templates too (tool/skill/import channels, the same
// "installed, but {marker}: new sessions…" shape as skill_gate.rs). Pin the
// shared interpolation shape in the production region with a lower bound so
// deleting the emit sites outright fails here instead of silently degrading
// the alert copy; legitimate reshapes update this leg knowingly.
const marketplaceSrc = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '../src-tauri/src/app/commands/marketplace.rs'),
  'utf8',
);
const marketplaceProduction = marketplaceSrc.split('#[cfg(test)]')[0];
const shapeCount = marketplaceProduction.split('but {}: new sessions will enable it by default')
  .length - 1;
assert.ok(
  shapeCount >= 5,
  `marketplace.rs must keep its post-landing consent-failure emit sites (found ${shapeCount}, expected >= 5)`,
);

// Round-20 review: the shape pin counts the template wording, not the marker
// ARGUMENT — keeping the sentence but swapping or dropping the
// CONSENT_SYNC_FAILURE_MARKER interpolation at an emit site kept every leg
// green while ToolStoreView's includes() stopped matching and the alert
// degraded to generic copy. Pin the constant-reference count in the same
// production region (currently exactly 5 sites).
const markerRefCount = marketplaceProduction.split('CONSENT_SYNC_FAILURE_MARKER').length - 1;
assert.ok(
  markerRefCount >= 5,
  `marketplace.rs emit sites must interpolate CONSENT_SYNC_FAILURE_MARKER, not just keep the template wording (found ${markerRefCount}, expected >= 5)`,
);

// Round-35 minor 1 (review #455): the Rust production templates must also
// carry the marker — a pin that only asserts the constant cannot catch a
// deleted interpolation at an emit site. Round-16: the ima-local wrap site is
// production-dead since deny-first (a refused gate aborts before anything
// lands, so ima connect surfaces the raw refusal — see the comment on
// consent_failure_marker_matches_the_frontend_contract in ima.rs), so the
// ima leg pins the shared-constant reference only; skill_gate.rs keeps a
// real production template and still pins its emit site.
for (const [file, emitSite, valueTest] of [
  [
    'connectors/skill_gate.rs',
    'but {}: new sessions will enable it by default',
    'fn skill_gate_consent_failure_message_keeps_the_frontend_marker',
  ],
  [
    'connectors/ima.rs',
    null,
    'fn consent_failure_marker_matches_the_frontend_contract',
  ],
]) {
  const src = readFileSync(join(rustDir, file), 'utf8');
  assert.ok(
    src.includes('scope::CONSENT_SYNC_FAILURE_MARKER'),
    `${file} must reference the shared consent marker constant`,
  );
  // Round-18 review (P3): for the reference-only legs this assertion can be
  // satisfied by the Rust test module alone, so pin the Rust-side VALUE test
  // by name too — a rename or rewrite that left the constant referenced only
  // from a comment would otherwise hollow this leg silently.
  assert.ok(
    src.includes(valueTest),
    `${file} must keep the Rust-side value contract test this leg leans on (${valueTest})`,
  );
  if (emitSite) {
    // Round-17 review: grep the PRODUCTION region only (up to the test
    // module) — the test module carries its own hand-built copy of the
    // template for the Rust-side message pin, so a whole-file grep would
    // keep passing after the production emit site drifted.
    const production = src.split('#[cfg(test)]')[0];
    assert.ok(
      production.includes(emitSite),
      `${file} production emit site drifted — re-check the marker interpolation`,
    );
  }
}

console.log('consent_marker_frontend: ok');
