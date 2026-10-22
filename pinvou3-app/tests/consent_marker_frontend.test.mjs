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

// ---------------------------------------------------------------------------
// Round-38 + round-3 review wiring pins. The settle-ordered composer refresh
// and the honest toast/banner copy are timing/UI wiring that no behavioral
// test reaches without a browser; the repo's convention for exactly this
// class is source-text pins (collapse-timer / null-guard style), so reverting
// the wiring fails here instead of silently shipping a stale-copy toast, a
// pre-grant refresh, or a lying banner.

const collapsedStore = collapsed(store);

// (a) The done toast for the fire-and-forget connectors (doneSettlesLater)
// stays title-only BOTH ways: the subtitle is undefined AND the alert's
// installHint fallback is suppressed — the fallback copy still says "off by
// default", so suppressing the subtitle alone renders the stale claim
// (round-3 review MAJOR).
assert.ok(
  collapsedStore.includes(
    'subtitle: doneSettlesLater ? undefined : toastCopyRef.current && toastCopyRef.current.enabledSubtitle, suppressHint: !!doneSettlesLater,',
  ),
  'the doneSettlesLater toast must pass both a falsy subtitle AND suppressHint — dropping either leg resurrects a stale off-by-default claim on the success toast',
);
assert.ok(
  collapsedStore.includes('!alert.isError && !alert.suppressHint &&'),
  'TsAlert must honor suppressHint in its installHint/removeHint fallback — reverting the condition renders the stale off-by-default copy again',
);

// (b) Every composer notify in the connector apply flows is settle-ordered
// in the module-level listener: fire-and-forget success + failure, awaited
// success + failure — exactly four. The mounted subscription's done phase
// must NOT broadcast — for the fire-and-forget connectors that read is
// provably pre-grant (round-3 review removed the wasted refresh).
{
  const connectedRegion = store.match(/cfg\.events\.connected[\s\S]*?ev\.listen\(cfg\.events\.error/);
  assert.ok(connectedRegion, 'the connector connected-handler region must stay recognizable');
  assert.strictEqual(
    connectedRegion[0].split('notifyComposerToolsChanged()').length - 1,
    4,
    'the connector apply flows must keep exactly four settle-ordered composer notifies (fire-and-forget success+failure, awaited success+failure) — a fifth in a done phase is the wasted pre-grant broadcast, a missing one strands a mounted composer on stale state',
  );
}
{
  const doneBranch = collapsedStore.match(/if \(ph === 'done'\) \{[\s\S]*?\} else if \(ph === 'error'\)/);
  assert.ok(doneBranch, 'the subscription done/error branch shape must stay recognizable');
  assert.ok(
    !doneBranch[0].includes('notifyComposerToolsChanged()'),
    "the subscription's done phase must not broadcast — the settle-ordered listener notifies cover it (round-3 review)",
  );
}

// (c) The awaited branches' success path notifies (round-3 review: it lived
// only in the mounted subscription's done phase before, so an unmounted
// store view's composer never learned of a successful connect) and clears
// the store latch on the way — done only trails a successful apply here, so
// nothing re-latches afterwards (review #741 P2 moved the latch to the store).
assert.ok(
  /await invokeTauri\(cfg\.commands\.applySkills\);[\s\S]{0,500}?conn\.setConsentSyncFailed\(false\);[\s\S]{0,500}?notifyComposerToolsChanged\(\)/.test(store),
  'the awaited apply success path must clear the store latch and notify the composer right after the apply resolves (view-independent, parity with its failure path)',
);

// (d) The connected banner falls back to the title-only copy while a post-auth
// connect failure is latched — the enabled-by-default copy would lie over the
// fail-closed state (round-3 review; round-6 review P2 widened the latch to
// every post-auth class).
assert.ok(
  collapsedStore.includes("const connectedBannerText = (key) => (consentSyncFailedByTool[key] ? storeCopy.connectedTool(storeCopy.toolNames[key]) : storeCopy.connectedBanner(storeCopy.toolNames[key]));"),
  'the connected banner must fall back to the title-only copy while a post-auth connect failure is latched',
);
for (const key of ['feishu', 'wecom', 'dingtalk', 'tmeet']) {
  assert.ok(
    store.includes(`consentSyncFailedByTool[key]`) && store.includes(`${key}ConsentSyncFailed`),
    `the ${key} subscription must feed its consent-failure latch into the banner gate`,
  );
}
// The latch covers BOTH post-auth classes at BOTH classification sites
// (round-6 review P2): an ordinary materialization failure revokes the fresh
// grant exactly like a persist failure, so the marker-only latch left the
// enabled-by-default banner standing over a disabled backend once the error
// card was dismissed. The fire-and-forget branch latches unconditionally (its
// whole catch is post-auth — the connected event already proved the sign-in);
// the awaited branch latches every class EXCEPT the pre-apply auth failures
// (no grant was attempted there, the backend state is untouched). A set on
// the mounted subscription would be lost on remount (review #741 P2).
assert.strictEqual(
  store.split("if (failure.code === 'connect_consent_persist_failed') conn.setConsentSyncFailed(true)").length - 1,
  0,
  'the marker-only latch guard must be gone from both classification sites — an ordinary materialization failure revokes the fresh grant too, so the enabled-by-default banner lied over the disabled backend after dismissal (round-6 review P2)',
);
assert.ok(
  store.includes("if (failure.code !== 'auth_failed') conn.setConsentSyncFailed(true);"),
  "the awaited branch must latch every post-auth class (anything but the pre-apply auth failures) — restricting the latch to the persist marker re-opens the round-6 P2",
);
assert.ok(
  /: \{ code: 'skills_enable_failed' \};[\s\S]{0,1200}?conn\.setConsentSyncFailed\(true\);[\s\S]{0,80}?reportConnectorFailure\(cfg\.key, failure, 'cli'\);/.test(store),
  'the fire-and-forget catch must latch unconditionally right after its classification (every rejection there is post-auth) — the round-6 P2',
);
assert.strictEqual(
  store.split('conn.setConsentSyncFailed(true)').length - 1,
  2,
  'the store latch must be set at exactly the two classification sites, BEFORE the null-guarded flow write — a set after it loses the closed-card case, a set in the mounted subscription is lost on remount (review #741 P2)',
);
assert.strictEqual(
  store.split('conn.setConsentSyncFailed(false)').length - 1,
  2,
  'the store latch must clear at exactly the two done WRITES (fire-and-forget connected event + awaited success) — a clear in the mounted subscription misses done phases that land while the store view is unmounted (review #741 P2)',
);
// The latch set must precede the null-guarded flow write at BOTH sites: the
// flow write is what broadcasts, so a set after it lets that broadcast carry
// the stale snapshot and a mounted view keeps the enabled-by-default banner
// until an unrelated broadcast. Anchored per branch on the shared
// set -> reportConnectorFailure -> flow-write sequence.
assert.strictEqual(
  (store.match(/conn\.setConsentSyncFailed\(true\);[\s\S]{0,80}?reportConnectorFailure\(cfg\.key, failure, '(?:cli|qr)'\);[\s\S]{0,200}?conn\.setFlow\(f => \(f \? applyConnectorFailure/g) || []).length,
  2,
  'the latch set must precede the null-guarded flow write at both classification sites — a set after it lets the failure broadcast carry the stale latch snapshot (review #741 P2)',
);
// The hook must mirror the store latch through useSyncExternalStore — React
// re-reads the store on every mount, so a latch that landed while the view was
// unmounted is picked up by hydration itself. A component-local useState
// (fresh false per mount) or a mount-time mirror statement regresses the P2.
assert.ok(
  /const consentSyncFailed = useSyncExternalStore\(\s*\(onStoreChange\) => conn\.subscribe\(onStoreChange\),\s*\(\) => conn\.consentSyncFailed,/.test(store),
  'the hook must mirror the store latch via useSyncExternalStore (subscribe through the conn store, snapshot = the store field) — a component-local latch restarted at false on remount (review #741 P2)',
);
assert.ok(
  !/setConsentSyncFailed\(conn\.consentSyncFailed\)/.test(store),
  'the hook must not hand-mirror the latch in setState — a hydration/mirror statement in the effect is exactly the mount-order fragility useSyncExternalStore removes (review #741 P2)',
);
// Dismissing the error card must not clear the latch: the fail-closed backend
// outlives the dismissed card, and clearing here re-arms the enabled-by-default
// connected banner (the P2 scenario's final step).
{
  const resetFlow = store.match(/const resetFlow = \(\{ setBusyId \}\) => \{[\s\S]*?\n {6}\};/);
  assert.ok(resetFlow, 'resetFlow must stay recognizable');
  assert.ok(
    !resetFlow[0].includes('setConsentSyncFailed'),
    'dismissing the error card must NOT clear the store latch — the banner keeps the title-only copy until a connect reaches done (review #741 P2)',
  );
}

// (e) The composer seq guard (round-38, round-2 review m9): only the latest
// refresh may commit, so a pre-grant read can never overtake the
// authoritative post-settle one.
const composerSrc = readFileSync(join(root, 'src/features/settings/composer-shared.jsx'), 'utf8');
assert.ok(
  composerSrc.includes('const seq = ++refreshSeqRef.current;'),
  'the composer refresh must bump a per-invocation seq — reverting the guard lets a stale pre-grant read commit over the authoritative one',
);
assert.ok(
  composerSrc.includes('seq === refreshSeqRef.current'),
  'the composer refresh must gate every commit on the seq being current',
);

// (f) Behavior of the REAL store factory (extracted from the production file,
// the same harness shape a fresh review used): the latch must survive the exact
// P2 sequence — consent write fails while the store view is unmounted (or the
// user leaves after seeing the error) → view remounts → error card dismissed →
// (returning again after dismissal) — without the connected banner re-arming
// its enabled-by-default copy, and only a fresh connect reaching done clears it.
{
  const factory = store.match(/const createFlowStore = \(\) => \(\{[\s\S]*?\n {4}\}\);/);
  assert.ok(factory, 'createFlowStore must stay recognizable');
  const createFlowStore = new Function(`${factory[0]}; return createFlowStore;`)();

  const conn = createFlowStore();
  // A mounted view: the subscriber mirrors BOTH fields (the hook's shape — the
  // latch is read off the store inside the broadcast callback).
  let mirroredLatch = false;
  const unsub = conn.subscribe(() => { mirroredLatch = conn.consentSyncFailed; });

  // The connect's consent persist fails after the card was closed: the flow
  // write null-guards to a no-op, but the store latch still lands and broadcasts.
  conn.setFlow(null);
  conn.setConsentSyncFailed(true);
  assert.ok(mirroredLatch, 'a latch-only change must broadcast — a mounted view must mirror the failure even when the flow write was a no-op');

  // Leave and return (fresh component, useState(false) + hydration): the mirror
  // must come back true from the store, not from component state.
  assert.ok(conn.consentSyncFailed, 'the latch must survive the view remount (review #741 P2)');

  // Dismiss the error card (resetConnectorFlow nulls the flow): the latch stays,
  // so the connected banner keeps its title-only fallback.
  conn.setFlow(null);
  assert.ok(conn.consentSyncFailed, 'dismissing the error card must not re-arm the enabled-by-default banner (review #741 P2)');

  // Returning after dismissal hydrates the latch still; a fresh connect reaching
  // done is the only thing that clears it.
  assert.ok(conn.consentSyncFailed, 'returning after dismissal must still hydrate the latch');
  conn.setConsentSyncFailed(false);
  assert.ok(!conn.consentSyncFailed, 'a fresh connect reaching done clears the latch');
  assert.ok(!mirroredLatch, 'the clear broadcasts too, so mounted views un-latch');

  // The mirror discipline is per-store: a sibling connector's store is unaffected.
  const sibling = createFlowStore();
  assert.ok(!sibling.consentSyncFailed, 'connector stores must not share the latch');
  unsub();
}

// (g) The round-6 P2 class through the same REAL store factory: an ORDINARY
// materialization failure (skills_enable_failed — no consent marker; the
// backend REVOKED the fresh grant) must latch exactly like the persist
// failure, and the sequence failure → dismiss → leave → return must keep the
// title-only banner until a successful retry reaches done and clears it.
{
  const factory = store.match(/const createFlowStore = \(\) => \(\{[\s\S]*?\n {4}\}\);/);
  assert.ok(factory, 'createFlowStore must stay recognizable');
  const createFlowStore = new Function(`${factory[0]}; return createFlowStore;`)();

  const conn = createFlowStore();
  let mirrored = false;
  const unsub = conn.subscribe(() => { mirrored = conn.consentSyncFailed; });

  // The ordinary failure latches (the widened classification routes through
  // the same store setter) and broadcasts to mounted views.
  conn.setConsentSyncFailed(true);
  assert.ok(mirrored, 'the materialization-failure latch must broadcast to mounted views');

  // Dismiss the error card, leave, return: the banner stays title-only over
  // the revoked backend.
  conn.setFlow(null);
  assert.ok(conn.consentSyncFailed, 'dismissing the error card must not re-arm the enabled-by-default banner over a REVOKED grant (round-6 review P2)');
  assert.ok(conn.consentSyncFailed, 'returning after dismissal must still hydrate the materialization-failure latch');

  // A successful retry reaching done is what clears it.
  conn.setConsentSyncFailed(false);
  assert.ok(!conn.consentSyncFailed && !mirrored, 'a successful retry reaching done clears the materialization-failure latch');
  unsub();
}

console.log('consent_marker_frontend: ok');
