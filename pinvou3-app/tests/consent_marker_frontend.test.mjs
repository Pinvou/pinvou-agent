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

// Round-35 minor 1 (review #455): the Rust production templates must also
// carry the marker — a pin that only asserts the constant cannot catch a
// deleted interpolation at an emit site. Round-16: the ima-local wrap site is
// production-dead since deny-first (a refused gate aborts before anything
// lands, so ima connect surfaces the raw refusal — see the comment on
// consent_failure_marker_matches_the_frontend_contract in ima.rs), so the
// ima leg pins the shared-constant reference only; skill_gate.rs keeps a
// real production template and still pins its emit site.
const rustDir = join(dirname(fileURLToPath(import.meta.url)), '../src-tauri/src/features');
for (const [file, emitSite] of [
  ['connectors/skill_gate.rs', 'but {}: new sessions will enable it by default'],
  ['connectors/ima.rs', null],
]) {
  const src = readFileSync(join(rustDir, file), 'utf8');
  assert.ok(
    src.includes('scope::CONSENT_SYNC_FAILURE_MARKER'),
    `${file} must reference the shared consent marker constant`,
  );
  if (emitSite) {
    assert.ok(
      src.includes(emitSite),
      `${file} production emit site drifted — re-check the marker interpolation`,
    );
  }
}

console.log('consent_marker_frontend: ok');
