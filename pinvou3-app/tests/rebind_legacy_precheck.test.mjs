// Source-level pins for the round-13 M1/M2 rebind fixes (review #463), the
// parts a store-level Rust test cannot observe:
//
// M1 — the REBIND_LEGACY_TABLE_* abort used to fire in the plain lane AFTER
// the codex lane had durably moved, while marker and copy claimed "nothing
// was moved". The fix runs the plain lane's planning half (candidate scan +
// legacy-table sync) BEFORE the codex lane mutates, which makes the copy
// literally true. The Rust half
// (features::sessions::tests::rebind_legacy_precheck_failure_leaves_every_lane_untouched)
// proves the planning half leaves every lane untouched; this pin proves the
// command layer calls the halves in the order that claim depends on.
//
// M2 — the boot-time parse-failed flag was a permanent in-process kill
// switch: one corrupt boot table aborted every rebind until app restart,
// even empty-plan runs and even after an out-of-band repair. The fix
// re-attempts the parse per run, so the flag (and the boot) no longer gates
// anything.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (...parts) =>
  readFileSync(new URL(['../', ...parts].join('/'), import.meta.url), 'utf8').replace(/\r\n/g, '\n');

const projectsRs = read('src-tauri', 'src', 'app', 'commands', 'projects.rs');
const bindingsRs = read('src-tauri', 'src', 'features', 'sessions', 'workspace_bindings.rs');
const storeRs = read('src-tauri', 'src', 'features', 'sessions', 'store.rs');
const modRs = read('src-tauri', 'src', 'features', 'sessions', 'mod.rs');

// M1: inside rebind_workspace_root the plain-lane plan (which performs the
// legacy-table sync and is where REBIND_LEGACY_TABLE_* aborts originate) is
// awaited BEFORE the codex lane's rebind_workspace_prefix, and the sidecar
// pass (apply) runs only after it.
const commandBody = projectsRs.slice(
  projectsRs.indexOf('pub async fn rebind_workspace_root'),
  projectsRs.indexOf('/// workspace_rebound event with the rebind geometry'),
);
const planAt = commandBody.indexOf('plan_rebind_workspace_bindings(&from, &to_display)');
const codexAt = commandBody.indexOf('rebind_workspace_prefix(&from, &to_display)');
const applyAt = commandBody.indexOf('apply_rebind_workspace_bindings(plain_plan)');
assert.ok(planAt > 0, 'the command must run the plain-lane planning half');
assert.ok(codexAt > 0, 'the command must run the codex lane');
assert.ok(applyAt > 0, 'the command must run the plain-lane sidecar pass');
assert.ok(
  planAt < codexAt && codexAt < applyAt,
  'the legacy-table precheck must complete before the codex lane mutates (M1: the abort copy says nothing was moved)',
);

// M1: the "nothing was moved" copy stays attached to BOTH legacy-table
// markers — it is only true because of the ordering pinned above.
assert.match(
  bindingsRs,
  /REBIND_LEGACY_TABLE_UNWRITABLE: [^\n]*nothing was moved/,
  'the unwritable marker keeps the (now true) nothing-moved claim',
);
assert.match(
  bindingsRs,
  /REBIND_LEGACY_TABLE_CORRUPT: [^\n]*nothing was moved/,
  'the corrupt marker carries the same claim and cause-specific remedy (M2)',
);

// M2: the parse gate describes the file NOW, not the boot — the boot-time
// kill-switch flag is gone everywhere, and the sync re-reads the table.
for (const [name, source] of [
  ['workspace_bindings.rs', bindingsRs],
  ['store.rs', storeRs],
  ['mod.rs', modRs],
]) {
  assert.ok(
    !source.includes('legacy_session_workspaces_parse_failed'),
    `${name} must not reference the removed boot-time parse-failed flag`,
  );
}
const syncBody = bindingsRs.slice(
  bindingsRs.indexOf('fn sync_legacy_session_workspaces'),
  bindingsRs.indexOf('fn migrate_legacy_session_workspaces'),
);
assert.match(
  syncBody,
  /std::fs::read_to_string\(&legacy\)/,
  'the sync must re-read the table per run (the gate describes the file, not the boot)',
);
assert.match(
  syncBody,
  /if plan\.is_empty\(\)/,
  'an empty-plan run proceeds past a corrupt/absent table',
);

console.log('rebind legacy-table precheck contract passed');
