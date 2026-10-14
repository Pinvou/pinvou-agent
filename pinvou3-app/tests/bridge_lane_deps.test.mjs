import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// ── Why this test exists ─────────────────────────────────────────────────────
// The bridge mirror merge moved ~300 shared function bodies into
// src/shared/bridge-shared-helpers.js; each lane cluster now passes a deps
// literal whose missing names silently bind `undefined`
// (sharedBridgeBase destructures the union of all cluster dep names). A deps
// literal that omits a name its cluster's bodies dereference crashes at
// runtime on the affected lane — invisible to node tests, which instantiate
// clusters with stub deps. PR #701 shipped two such crashes (tauriMonitor
// polled with state/invoke/notify/bt/sessionStates undefined; tauriChat's
// sendMessageToSession called restoreEvictedSessionDraft through the base
// getBuffer/touchSessionBuffer bodies). This test pins the load-bearing deps
// of every cluster that was repaired, and alarms when the shared surface
// changes so the pins can be consciously re-derived.
//
// Re-derivation procedure when the drift alarm fires (full AST audit):
//   1. Parse src/shared/bridge-shared-helpers.js with a JS parser (acorn).
//   2. For each cluster in the CLUSTERS table, take each exported function
//      and walk the intra-base call graph from it; collect the union dep
//      names referenced (excluding member-property positions, function
//      params at any depth, and locally declared names). A name is SATISFIED
//      when the literal provides it AND (it is in FORWARDER_DEP_NAMES or it
//      is not a base function declaration) — provided forwarders replace the
//      base binding, so the base body's refs and callees do not apply.
//      Provided names that are base declarations but NOT forwarders are
//      inert (silently ignored) and do not satisfy anything.
//   3. Missing(C, export) = collected refs not satisfied. Fix literals (or
//      provide main-scope forwarders via context) until every consumed
//      export has none, then update REQUIRED below to the load-bearing
//      entries and refresh the surface snapshot.

const read = (p) => fs.readFileSync(path.join(root, p), 'utf8');

// ── Load-bearing deps pinned per repaired cluster ────────────────────────────
// Each entry is the minimal set of deps literal keys the repair wave added;
// removing any of them reintroduces a runtime crash on the desktop lane.
const REQUIRED = {
  tauriMonitor: ['state', 'sessionStates', 'notify', 'invoke', 'bt'],
  // getBuffer/touchSessionBuffer forwarders route sendMessageToSession's
  // buffer path to the main-scope implementations (restoreEvictedSessionDraft,
  // sessionBufferTouchClock, prune*, freshBuffer stay out of the literal).
  tauriChat: ['getBuffer', 'touchSessionBuffer'],
  // isDeliverable forwarder cuts the base body's DELIVERABLE_EXTS ref inside
  // rerenderFromMessagesCore; personaPlaceholderTitles is read directly by
  // persistMessagesFor.
  tauriMain: ['isDeliverable', 'personaPlaceholderTitles'],
  // flushAssistantMessageToHistory's export needs these three even though the
  // lane currently consumes only cancelStreamRenderTimers — the export is
  // part of the cluster's public surface and must stay resolvable.
  tauriChatEvents: ['bt', 'flushPendingTextBlock', 'autoCreateScheduledTaskDraft'],
};

const LITERAL_FILE = {
  tauriMonitor: 'src/platform/tauri/bridge/monitor.js',
  tauriChat: 'src/platform/tauri/bridge/chat.js',
  tauriMain: 'src/platform/tauri/bridge.js',
  tauriChatEvents: 'src/platform/tauri/bridge/chat-events.js',
};

// The four literals are single-line, so a line-anchored extraction is exact.
function extractProvidedKeys(file, cluster) {
  const source = read(file);
  const lines = source.split('\n');
  const line = lines.find((l) => l.includes(`create("${cluster}"`));
  assert.ok(line, `create("${cluster}" literal not found in ${file}`);
  const start = line.indexOf('{', line.indexOf(`create("${cluster}"`));
  // brace-balanced scan with quote awareness (no template literals inside
  // these literals — the shared surface alarm below forces re-review if the
  // literal shape ever changes)
  let depth = 0;
  let inQuote = null;
  for (let i = start; i < line.length; i++) {
    const c = line[i];
    if (inQuote) {
      if (c === '\\') i++;
      else if (c === inQuote) inQuote = null;
      continue;
    }
    if (c === "'" || c === '"') { inQuote = c; continue; }
    if (c === '{') depth++;
    else if (c === '}') {
      depth--;
      if (depth === 0) {
        const body = line.slice(start + 1, i);
        const keys = [];
        let d = 0;
        let cur = '';
        let q = null;
        for (const ch of body) {
          if (q) { if (ch === '\\') continue; if (ch === q) q = null; continue; }
          if (ch === "'" || ch === '"') { q = ch; continue; }
          if (ch === '{' || ch === '(' || ch === '[') d++;
          else if (ch === '}' || ch === ')' || ch === ']') d--;
          else if (ch === ',' && d === 0) { keys.push(cur); cur = ''; continue; }
          cur += ch;
        }
        keys.push(cur);
        return keys.map((k) => k.trim()).filter(Boolean).map((k) => {
          const m = k.match(/^([A-Za-z_$][\w$]*)\s*:/);
          return m ? m[1] : k.replace(/\s*=\s*.*$/, '').trim();
        }).filter((k) => /^[A-Za-z_$][\w$]*$/.test(k));
      }
    }
  }
  assert.fail(`unbalanced create("${cluster}" literal in ${file}`);
}

// ── Assertions ───────────────────────────────────────────────────────────────
for (const [cluster, required] of Object.entries(REQUIRED)) {
  const provided = extractProvidedKeys(LITERAL_FILE[cluster], cluster);
  const missing = required.filter((name) => !provided.includes(name));
  assert.deepEqual(
    missing,
    [],
    `bridge cluster ${cluster} is missing required deps in ${LITERAL_FILE[cluster]}: ` +
    `${missing.join(', ')} — the shared bodies dereference them and will throw ` +
    `at runtime on the desktop lane (see the header comment for the audit procedure)`,
  );
}

// ── Shared-surface drift alarm ───────────────────────────────────────────────
// If any of these snapshots change, a shared body, export, or forwarder was
// added/removed: re-run the audit from the header comment and update REQUIRED.
const helpers = read('src/shared/bridge-shared-helpers.js');
// count function declarations INSIDE sharedBridgeBase only (the base span
// runs from its declaration to the CLUSTERS table; the AST-verified count is
// the contract — re-derive with the dep audit when it changes)
const baseStart = helpers.indexOf('function sharedBridgeBase');
const baseEnd = helpers.indexOf('const CLUSTERS = {');
const sharedFnCount = helpers.slice(baseStart, baseEnd)
  .split('\n')
  .filter((l) => /^  (async )?function /.test(l))
  .length;
const forwarders = (helpers.match(/const FORWARDER_DEP_NAMES = \[([\s\S]*?)\];/) ?? [])[1] ?? '';
const forwarderNames = (forwarders.match(/[A-Za-z_$][\w$]*/g) ?? []).filter((n) => n !== 'FORWARDER_DEP_NAMES');
const snapshot = {
  sharedFnCount,
  forwarderNames: forwarderNames.sort(),
  clusters: {},
};
const clusterRe = /"([\w:]+)": function \(deps\) \{\n\s*const b = sharedBridgeBase\(ensureCells\(deps\)\);\n\s*return Object\.freeze\(\{([^}]*)\}\);/g;
let m;
while ((m = clusterRe.exec(helpers)) !== null) {
  snapshot.clusters[m[1]] = [...m[2].matchAll(/([\w$]+): b\.([\w$]+)/g)].map((p) => `${p[1]}:${p[2]}`);
}
const EXPECTED_SURFACE = {
  sharedFnCount: 308,
  clusterCount: Object.keys(snapshot.clusters).length,
};

assert.equal(
  snapshot.sharedFnCount,
  EXPECTED_SURFACE.sharedFnCount,
  `sharedBridgeBase now declares ${snapshot.sharedFnCount} functions (was ${EXPECTED_SURFACE.sharedFnCount}). ` +
  'New or removed shared bodies change which deps each cluster needs — re-run the dep audit ' +
  '(header comment) and update REQUIRED in this test.',
);
// 19 = the standard factories matched by the freeze-shape regex above; the
// nested `<base>:<offset>` keys are one-line aliases and are not captured.
assert.equal(
  Object.keys(snapshot.clusters).length,
  19,
  `the CLUSTERS table changed (${Object.keys(snapshot.clusters).length} standard factories) — ` +
  're-run the dep audit (header comment) and update REQUIRED in this test',
);
for (const [cluster, required] of Object.entries(REQUIRED)) {
  assert.ok(
    snapshot.clusters[cluster],
    `cluster ${cluster} disappeared from the CLUSTERS table — re-derive REQUIRED`,
  );
}
assert.ok(forwarderNames.length >= 20, 'FORWARDER_DEP_NAMES list unexpectedly shrank');

console.log('bridge lane cluster dep contracts passed');
