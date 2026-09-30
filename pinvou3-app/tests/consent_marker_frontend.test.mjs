// Round-33 MAJOR 2 (review #455): the frontend's consent-failure matcher must
// key on the exact shared marker string the Rust emitters pin
// (scope::CONSENT_SYNC_FAILURE_MARKER — asserted on the Rust side by
// consent_failure_message_keeps_the_frontend_marker in ima.rs and
// skill_gate_consent_failure_message_keeps_the_frontend_marker in
// skill_gate.rs). A backend or frontend rewording must move both sides in the
// same commit; this source-text assertion is the frontend leg of that pin.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const store = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '../src/features/tools/ToolStoreView.jsx'),
  'utf8',
);
const MARKER = 'persisting their default-off consent state failed';
assert.ok(
  store.includes(MARKER),
  'ToolStoreView must key the consent-failure surfacing on the shared backend marker (update the Rust pin in the same commit if reworded)',
);

// Round-35 minor 1 (review #455): the two Rust production templates must also
// carry the marker — a pin that only asserts the constant cannot catch a
// deleted interpolation at an emit site.
const rustDir = join(dirname(fileURLToPath(import.meta.url)), '../src-tauri/src/features');
for (const [file, emitSite] of [
  ['connectors/skill_gate.rs', 'but {}: new sessions will enable it by default'],
  ['connectors/ima.rs', "ima skills installed, but {IMA_CONSENT_SYNC_FAILURE_MARKER}:"],
]) {
  const src = readFileSync(join(rustDir, file), 'utf8');
  assert.ok(
    src.includes('scope::CONSENT_SYNC_FAILURE_MARKER'),
    `${file} must interpolate the shared consent marker constant in its production template`,
  );
  assert.ok(
    src.includes(emitSite),
    `${file} production emit site drifted — re-check the marker interpolation`,
  );
}

console.log('consent_marker_frontend: ok');
