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

console.log('consent_marker_frontend: ok');
