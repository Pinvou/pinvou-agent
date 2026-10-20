// The frontends' consent-failure matchers must key on the exact shared marker
// strings the Rust emitters pin:
// - scope::CONSENT_SYNC_FAILURE_MARKER — the INSTALL channels (tool/skill/
//   import post-landing consent + the defensive ima matcher), asserted on the
//   Rust side by the value legs in skill_gate.rs;
// - scope::CONNECT_CONSENT_SYNC_FAILURE_MARKER — the CONNECT channel
//   (feat/connector-connect-grants-enable: the CLI connector connect flow
//   records an ENABLE grant, so its persist failure is fail-closed and gets
//   its own `connect_consent_persist_failed` copy).
// A backend or frontend rewording must move both sides in the same commit;
// these source-text assertions are the frontend legs of that pin.
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

// Each literal lives ONCE in connector-ui-state.js (mirroring its scope.rs
// constant); the JS matchers import the consts. The pins assert the single
// definitions and that every matcher site imports a const — one-site drift
// cannot survive.
const INSTALL_MARKER = 'persisting their default-off consent state failed';
const CONNECT_MARKER = "persisting the connector's connect-enabled consent failed";
const uiState = readFileSync(
  join(root, 'src/features/tools/connector-ui-state.js'),
  'utf8',
);
const collapsed = (src) => src.replace(/\s+/g, ' ');
assert.ok(
  collapsed(uiState).includes(`export const CONSENT_SYNC_FAILURE_MARKER = '${INSTALL_MARKER}'`),
  'connector-ui-state.js must define the install consent marker exactly once (update the Rust pin in the same commit if reworded)',
);
assert.ok(
  collapsed(uiState).includes(`export const CONNECT_CONSENT_SYNC_FAILURE_MARKER = "${CONNECT_MARKER}"`),
  'connector-ui-state.js must define the connect consent marker exactly once (update the Rust pin in the same commit if reworded)',
);
const store = readFileSync(
  join(root, 'src/features/tools/ToolStoreView.jsx'),
  'utf8',
);
for (const MARKER of [INSTALL_MARKER, CONNECT_MARKER]) {
  assert.ok(
    !store.includes(MARKER),
    'the marker literal must be defined only in connector-ui-state.js (import the const instead)',
  );
}
// Name presence alone is satisfied by the import and the comments alone, so a
// matcher site reverted to a stale const — or the matcher deleted outright —
// passes a bare includes() leg while the failure degrades to the generic copy.
// Pin the per-channel CALL COUNTS instead, anchored on the `includes(<CONST>)`
// call shape: the connect channel has exactly its two catch sites (feishu/wecom
// apply + the awaited dingtalk/tmeet branch) and the install channel exactly
// its one ima-pattern matcher. Anchoring after `includes(` keeps the
// CONNECT_ call sites from satisfying the install count and keeps the import
// and comment mentions from satisfying either.
assert.strictEqual(
  store.split('includes(CONNECT_CONSENT_SYNC_FAILURE_MARKER)').length - 1,
  2,
  'ToolStoreView must key exactly its two connect catch sites on CONNECT_CONSENT_SYNC_FAILURE_MARKER — a site reverted to a stale const, a deleted site, or a hand-rolled literal breaks this pin',
);
assert.strictEqual(
  store.split('includes(CONSENT_SYNC_FAILURE_MARKER)').length - 1,
  1,
  'ToolStoreView must keep exactly one install consent-failure matcher keyed on CONSENT_SYNC_FAILURE_MARKER (the ima-pattern matcher) — reverting it to a stale const, deleting it, or a hand-rolled literal breaks this pin',
);

// A backend-only reword (Rust const + updated Rust value test) would otherwise
// pass every leg while ToolStoreView's includes() silently stopped matching.
// Cross-compare: extract each Rust constant's VALUE from scope.rs and require
// it to equal the JS literal — a reword fails one side or the other until both
// move.
const rustDir = join(root, 'src-tauri/src/features');
const scopeSrc = readFileSync(join(rustDir, 'marketplace/scope.rs'), 'utf8');
// Anchored to a top-level declaration so comment/string mentions cannot
// satisfy the extraction; `\s*` bridges rustfmt's wrapped string literal.
const rustMarker = (name) =>
  scopeSrc.match(new RegExp(`^pub(?:\\(crate\\))?\\s+const ${name}:\\s*&str\\s*=\\s*"([^"]+)"`, 'm'))?.[1];
assert.ok(
  rustMarker('CONSENT_SYNC_FAILURE_MARKER'),
  'scope.rs must define CONSENT_SYNC_FAILURE_MARKER as a plain string literal',
);
assert.strictEqual(
  rustMarker('CONSENT_SYNC_FAILURE_MARKER'),
  INSTALL_MARKER,
  'the Rust install marker constant and the frontend matcher literal have drifted — reword both sides in the same commit',
);
assert.ok(
  rustMarker('CONNECT_CONSENT_SYNC_FAILURE_MARKER'),
  'scope.rs must define CONNECT_CONSENT_SYNC_FAILURE_MARKER as a plain string literal',
);
assert.strictEqual(
  rustMarker('CONNECT_CONSENT_SYNC_FAILURE_MARKER'),
  CONNECT_MARKER,
  'the Rust connect marker constant and the frontend matcher literal have drifted — reword both sides in the same commit',
);

// The consent-failure copy is SINGLE-SOURCED per channel: each sentence lives
// once, inside its shared scope.rs helper (consent_sync_failure_message for
// the install channels, connect_consent_sync_failure_message for the connect
// channel) which interpolates its marker between the subject and the fixed
// tail; every emit site routes through its channel's helper instead of
// hand-rolling the wording. The helper IS the marker interpolation, so a
// wording without the marker is impossible by construction, one-site drift
// cannot survive, and any hand-rolled copy fails the exact-count legs below.
// The per-channel call counts are LOWER BOUNDS so deleting an emit site still
// fails a leg while a legitimately added channel routed through the helper
// does not.
const INSTALL_TAIL = 'new sessions will enable it by default';
const CONNECT_TAIL = 'new sessions will keep it disabled';

// (a) Each template tail must exist exactly once in scope.rs PRODUCTION —
// inside its shared helper.
const scopeProduction = productionOf(scopeSrc);
assert.ok(
  scopeProduction.split(INSTALL_TAIL).length - 1 === 1,
  'the install consent-failure template must exist exactly once in scope.rs production — inside the shared helper',
);
assert.ok(
  scopeProduction.split(CONNECT_TAIL).length - 1 === 1,
  'the connect consent-failure template must exist exactly once in scope.rs production — inside the shared helper',
);

// (a2) Every leg above survives a helper-body reword that drops the
// interpolation (const defined, tail present once, call counts intact) while
// ToolStoreView's includes() silently stops matching and every consent
// failure degrades to the generic error code — exactly the degradation this
// file exists to prevent. Pin the interpolations from the helpers'
// declarations: a dropped interpolation fails here by construction.
const pinInterpolation = (src, helperName, markerConst) => {
  const helperStart = src.indexOf(`fn ${helperName}`);
  assert.ok(helperStart > 0, `scope.rs must define the shared ${helperName} helper in production`);
  assert.ok(
    src.slice(helperStart).includes(`{${markerConst}}`),
    `${helperName} must interpolate {${markerConst}} — a wording without it degrades every consent failure to the generic error code`,
  );
};
pinInterpolation(scopeProduction, 'consent_sync_failure_message', 'CONSENT_SYNC_FAILURE_MARKER');
pinInterpolation(scopeProduction, 'connect_consent_sync_failure_message', 'CONNECT_CONSENT_SYNC_FAILURE_MARKER');

// (b)/(c) Each channel's emit sites route through its helper (lower bound 1)
// and neither file may hand-roll its channel's template. Scope of the
// uniqueness claim: the tail legs pin each tail to exactly once in scope.rs
// production only, and these legs cover only these two files — a
// consent-failure emitter added in a THIRD file is not guarded here today
// (grep the tails across src-tauri when adding one). A hand-rolled copy in
// either covered file (marker interpolated or not) fails here; deleting the
// emit site drops the lower-bound leg.
const assertRoutesThroughSharedHelper = (label, src, helperName, tail) => {
  const production = productionOf(src);
  const calls = production.split(`${helperName}(`).length - 1;
  assert.ok(
    calls >= 1,
    `${label}.rs emit sites must route through ${helperName} (found ${calls}, expected >= 1)`,
  );
  const copies = production.split(tail).length - 1;
  assert.ok(
    copies === 0,
    `${label}.rs must not hand-roll the ${helperName} template (found ${copies}, expected 0) — every channel must call the shared helper`,
  );
};

// (b) The marketplace command layer (install channels: tool/skill/import).
const marketplaceSrc = readFileSync(
  join(root, 'src-tauri/src/app/commands/marketplace.rs'),
  'utf8',
);
assertRoutesThroughSharedHelper('marketplace', marketplaceSrc, 'consent_sync_failure_message', INSTALL_TAIL);
assert.ok(
  !marketplaceSrc.includes('connect_consent_sync_failure_message('),
  'the install channels keep the install helper — the connect helper is the connector gate channel only',
);

// (c) The skill-gate channel's Rust leg (the connect channel).
const skillGateSrc = readFileSync(join(rustDir, 'connectors/skill_gate.rs'), 'utf8');
// Pin the Rust-side VALUE test by name — a rename or
// rewrite that left the constant referenced only from a comment would
// otherwise hollow this leg silently.
assert.ok(
  skillGateSrc.includes('fn skill_gate_consent_failure_message_keeps_the_frontend_marker'),
  'skill_gate.rs must keep the Rust-side value contract test this leg leans on (fn skill_gate_consent_failure_message_keeps_the_frontend_marker)',
);
assertRoutesThroughSharedHelper('skill_gate', skillGateSrc, 'connect_consent_sync_failure_message', CONNECT_TAIL);

console.log('consent_marker_frontend: ok');
