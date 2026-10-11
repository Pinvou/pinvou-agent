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

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const productionOf = (src) => src.split('#[cfg(test)]')[0];

// Round-37 F3 (review #455): the literal lives ONCE, in connector-ui-state.js
// (mirroring scope::CONSENT_SYNC_FAILURE_MARKER); the three JS matchers import
// the const. The pin asserts the single definition and that every matcher
// site imports it — one-site drift cannot survive.
const uiState = readFileSync(
  join(root, 'src/features/tools/connector-ui-state.js'),
  'utf8',
);
const MARKER = 'persisting their default-off consent state failed';
assert.ok(
  uiState.includes(`export const CONSENT_SYNC_FAILURE_MARKER =\n  '${MARKER}'`)
    || uiState.includes(`export const CONSENT_SYNC_FAILURE_MARKER = '${MARKER}'`),
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

// Round-19 review: the two sides above were independent frozen strings, so a
// backend-only reword (Rust const + updated Rust value test) passed every leg
// while ToolStoreView's includes() silently stopped matching. Cross-compare:
// extract the Rust constant's VALUE from scope.rs and require it to equal the
// JS literal — a reword now fails one side or the other until both move.
const rustDir = join(root, 'src-tauri/src/features');
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

// Helper-design rework (PR #517 CI fix): supersedes the counted-===5 shape
// pins over marketplace.rs below (the emit sites had legitimately collapsed
// to 3, leaving those legs permanently red). The consent-failure copy is now
// SINGLE-SOURCED: the sentence lives once, inside the shared scope.rs helper
// consent_sync_failure_message(subject, error) which interpolates the marker
// between the subject and the fixed tail; every emit site (the marketplace
// command layer and the skill gate) calls the helper instead of hand-rolling
// the wording. The rounds 19/20/26 rationale is preserved structurally: the
// helper IS the marker interpolation, so a wording without the marker is
// impossible by construction (round 20), one-site drift cannot survive
// (round 19), and any hand-rolled copy fails the exact-count legs below
// (round 26's no-silent-degradation guarantee). The per-channel call counts
// are LOWER BOUNDS so deleting an emit site still fails a leg while a
// legitimately added fourth channel routed through the helper does not.
const TEMPLATE_TAIL = 'new sessions will enable it by default';

// (a) The template tail must exist exactly once in scope.rs PRODUCTION —
// inside the helper. scope.rs cannot use the plain `productionOf` split
// above: test-only items (`#[cfg(test)] fn ...` failpoints and test helpers)
// are scattered through the file BEFORE the marker/helper region, so its
// first `#[cfg(test)]` is not the test MODULE — anchor the production cut at
// the module instead.
const scopeTestsIdx = scopeSrc.search(/#\[cfg\(test\)\]\s*mod tests\b/);
assert.ok(
  scopeTestsIdx > 0,
  'scope.rs must keep its test module after the production body',
);
const scopeProduction = scopeSrc.slice(0, scopeTestsIdx);
assert.ok(
  scopeSrc.includes('fn consent_sync_failure_message'),
  'scope.rs must define the shared consent_sync_failure_message helper',
);
assert.ok(
  scopeProduction.split(TEMPLATE_TAIL).length - 1 === 1,
  'the consent-failure template must exist exactly once in scope.rs production — inside the shared helper',
);

// (a2) Round-27 review: the helper-design comment's "a wording without the
// marker is impossible by construction" overstates — every leg above
// survives a helper-body reword that drops the interpolation (const defined,
// tail present once, call counts intact) while ToolStoreView's includes()
// silently stops matching and every consent failure degrades to the generic
// skills_enable_failed code — exactly the degradation this file exists to
// prevent. Pin the interpolation inside the helper's own slice (bounded by
// the next fn item): a dropped interpolation fails here by construction.
const helperStart = scopeProduction.indexOf('fn consent_sync_failure_message');
assert.ok(helperStart > 0, 'the shared helper must exist in scope.rs production');
const afterHelper = scopeProduction.slice(helperStart);
const nextFnIdx = afterHelper.slice(1).search(/\n(?:pub(?:\(crate\))?\s+)?fn\s/);
const helperBody = nextFnIdx === -1 ? afterHelper : afterHelper.slice(0, nextFnIdx + 1);
assert.ok(
  helperBody.includes('{CONSENT_SYNC_FAILURE_MARKER}'),
  'consent_sync_failure_message must interpolate {CONSENT_SYNC_FAILURE_MARKER} — a wording without it degrades every consent failure to the generic error code',
);

// (b) Round-19 review: the marketplace command layer carries the post-landing
// consent-failure channels (tool/skill/import). Round-26 review: exact
// equality, not a lower bound — the shape and marker-interpolation counts
// over the hand-rolled format! sites used to be pinned at 5 so a site could
// not keep the wording while dropping the marker. Under the helper design
// that becomes: every channel calls the helper (lower bound 3), and the
// template tail is hand-rolled exactly ONCE — the round-24 minor 6 join-arm
// copy ("its consent state could not be applied (background task failed)"),
// a deliberately distinct middle wording that stays byte-for-byte. Any OTHER
// hand-rolled copy (marker interpolated or not) pushes the count past 1 and
// fails here; deleting an emit site drops one of the two legs.
const marketplaceSrc = readFileSync(
  join(root, 'src-tauri/src/app/commands/marketplace.rs'),
  'utf8',
);
const marketplaceProduction = productionOf(marketplaceSrc);
const helperCalls = marketplaceProduction.split('consent_sync_failure_message(')
  .length - 1;
assert.ok(
  helperCalls >= 3,
  `marketplace.rs emit sites must route through consent_sync_failure_message (found ${helperCalls}, expected >= 3)`,
);
const templateCopies = marketplaceProduction.split(TEMPLATE_TAIL).length - 1;
assert.ok(
  templateCopies === 1,
  `marketplace.rs must keep exactly the one distinct join-arm consent-failure copy (found ${templateCopies}, expected 1) — every other channel must call the shared helper`,
);

// (c) Round-35 minor 1 (review #455): per-channel Rust legs, reworked for the
// helper design — skill_gate.rs production routes its emit site through the
// shared helper (its old hand-rolled-template pin died with the two-hop
// shape); ima.rs has no production template since deny-first (a refused gate
// aborts before anything lands, so ima connect surfaces the raw refusal —
// see the comment on consent_failure_marker_matches_the_frontend_contract
// in ima.rs) and stays reference-only.
for (const [file, minHelperCalls, valueTest] of [
  [
    'connectors/skill_gate.rs',
    1,
    'fn skill_gate_consent_failure_message_keeps_the_frontend_marker',
  ],
  [
    'connectors/ima.rs',
    0,
    'fn consent_failure_marker_matches_the_frontend_contract',
  ],
]) {
  const src = readFileSync(join(rustDir, file), 'utf8');
  assert.ok(
    src.includes('scope::CONSENT_SYNC_FAILURE_MARKER'),
    `${file} must reference the shared consent marker constant`,
  );
  // Round-18 review (P3): the reference leg can be satisfied by the Rust
  // test module alone, so pin the Rust-side VALUE test by name too — a
  // rename or rewrite that left the constant referenced only from a comment
  // would otherwise hollow this leg silently.
  assert.ok(
    src.includes(valueTest),
    `${file} must keep the Rust-side value contract test this leg leans on (${valueTest})`,
  );
  if (minHelperCalls > 0) {
    // Round-17 review: grep the PRODUCTION region only (up to the test
    // module) — the test module carries its own hand-built copy of the
    // template for the Rust-side message pin, so a whole-file grep would
    // keep passing after the production emit site drifted. Helper legs: the
    // production region must call the shared helper (lower bound, same
    // deletion/insertion tradeoff as the marketplace leg) and must not
    // hand-roll the template at all.
    const production = productionOf(src);
    const channelHelperCalls = production.split('consent_sync_failure_message(')
      .length - 1;
    assert.ok(
      channelHelperCalls >= minHelperCalls,
      `${file} production must route its consent-failure emit site through the shared helper (found ${channelHelperCalls}, expected >= ${minHelperCalls})`,
    );
    assert.ok(
      production.split(TEMPLATE_TAIL).length - 1 === 0,
      `${file} production must not hand-roll the consent-failure template — call the shared helper`,
    );
  }
}

console.log('consent_marker_frontend: ok');
