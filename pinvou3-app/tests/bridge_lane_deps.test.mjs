import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import * as acorn from 'acorn';

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
// The round-2 review extended the pins from the four repaired clusters to the
// full audited surface: EVERY create("cluster", {…}) literal in BOTH lanes is
// pinned key-exactly (EXPECTED_CLUSTER_KEYS below), and FORWARDER_DEP_NAMES
// is pinned list-exactly. A literal key the base neither destructures nor
// forwards is silently ignored dead wiring, so extra keys fail too.
//
// Re-derivation procedure when a pin fires (full AST audit):
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
//      export has none, then update REQUIRED and EXPECTED_CLUSTER_KEYS.

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


// Deterministic identifier ordering shared by every sorted comparison below.
const cmpNames = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
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
  .filter((l) => /^ {2}(async )?function /.test(l))
  .length;
// AST-extracted (round-3 review): the raw-source regex was comment-blind —
// a name commented out of FORWARDER_DEP_NAMES is a runtime removal (the
// base binding is used instead) — and the comment stripper itself cannot
// be trusted on this file: a regex literal containing a quote
// (`/[&<>"']/g`, escapeHtml) parks it in string mode for the rest of the
// file, so line comments inside that span pass through verbatim. acorn is
// the repo's parser dependency (see scripts/audit-compat.mjs).
const helpersAst = acorn.parse(helpers, { ecmaVersion: 'latest', sourceType: 'script' });
const forwarderNames = [];
const setterNames = [];
(function walkNode(node) {
  if (!node || typeof node.type !== 'string') return;
  if (node.type === 'VariableDeclarator' && node.id?.type === 'Identifier') {
    if (node.id.name === 'FORWARDER_DEP_NAMES' && node.init?.type === 'ArrayExpression') {
      for (const el of node.init.elements) {
        assert.equal(el?.type, 'Literal', 'FORWARDER_DEP_NAMES must stay a plain string array');
        forwarderNames.push(el.value);
      }
    }
    if (node.id.name === 'forwarderSetters' && node.init?.type === 'ObjectExpression') {
      for (const prop of node.init.properties) {
        assert.equal(prop.type, 'Property', 'forwarderSetters must stay a plain method table');
        setterNames.push(prop.key.name);
      }
    }
  }
  for (const key of Object.keys(node)) {
    const child = node[key];
    if (Array.isArray(child)) child.forEach(walkNode);
    else if (child && typeof child.type === 'string') walkNode(child);
  }
})(helpersAst);
const snapshot = {
  sharedFnCount,
  forwarderNames: forwarderNames.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)),
  clusters: {},
};
const clusterRe = /"([\w:]+)": function \(deps\) \{\n\s*const b = sharedBridgeBase\(ensureCells\(deps\)\);\n\s*return Object\.freeze\(\{([^}]*)\}\);/g;
let m;
while ((m = clusterRe.exec(helpers)) !== null) {
  snapshot.clusters[m[1]] = [...m[2].matchAll(/([\w$]+): b\.([\w$]+)/g)].map((p) => `${p[1]}:${p[2]}`);
}
const EXPECTED_SURFACE = {
  sharedFnCount: 308,
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
for (const cluster of Object.keys(REQUIRED)) {
  assert.ok(
    snapshot.clusters[cluster],
    `cluster ${cluster} disappeared from the CLUSTERS table — re-derive REQUIRED`,
  );
}



// ── Full literal surface: every cluster, both lanes, key-exact ───────────────
// REQUIRED above pins only the four repaired clusters' load-bearing minimums.
// The audited complete surface follows. Expected sets were derived by the
// scope-aware audit described in the header (union ∖ FORWARDER_DEP_NAMES
// classification; the round-2 wave also removed the 13 then-inert keys from 7
// literals, so the pinned sets contain no dead wiring). Same-named literals in
// the two lanes (auxChat) must stay identical.
const EXPECTED_CLUSTER_KEYS = {
  'auxChat': [
    'auxChatDispatch',
    'bt',
    'ensureSessionBufferLoaded',
    'invoke',
    'isBusyFor',
    'purgeSessionBuffer',
    'sessionStates',
    'state',
    'touchSessionBuffer',
  ],
  'tauriArtifactTracker': ['DELIVERABLE_EXTS', 'invoke', 'isScheduledRunSession', 'notify', 'state'],
  'tauriArtifacts': [
    'addAttachmentByPath',
    'addSystemItem',
    'artifactInfo',
    'basename',
    'bt',
    'dialogOpen',
    'invoke',
    'isDeliverable',
    'sessionStates',
    'state',
  ],
  'tauriChat': [
    'addChatItem',
    'autoCreateScheduledTaskDraft',
    'bt',
    'doSendFor',
    'ensureSessionBufferLoaded',
    'flushPendingTextBlock',
    'flushQueued',
    'getBuffer',
    'invoke',
    'itemIdSeq',
    'notify',
    'pendingAssistantBlocks',
    'reconcileRemoteTurn',
    'recordAuthoritySyncDiagnostic',
    'runSyncOnSession',
    'sendMessage',
    'sessionStates',
    'state',
    'touchSessionBuffer',
  ],
  'tauriChat:31666': ['sid', 'state'],
  'tauriChatEvents': [
    'addChatItem',
    'autoCreateScheduledTaskDraft',
    'bt',
    'finalizeAssistantStreamBeforeReasoning',
    'flushPendingTextBlock',
    'itemIdSeq',
    'pendingAssistantBlocks',
    'pendingAssistantText',
    'pendingStreamNotifications',
    'state',
  ],
  'tauriDependencies': ['invoke', 'notify', 'state'],
  'tauriInteraction': [
    'addSystemItem',
    'applyAuthoritativeModeState',
    'bt',
    'currentDraftModeState',
    'flushAssistantMessageToHistory',
    'invoke',
    'notify',
    'pushUserEcho',
    'runSyncOnSession',
    'sendMessage',
    'setDraftMode',
    'state',
  ],
  'tauriKnowledgeModel': ['invoke', 'notify', 'state'],
  'tauriMain': [
    'BT_TABLE',
    'PINVOU_SCENE_EVENTS_STORAGE_PREFIX',
    'addChatItem',
    'extractArtifactPaths',
    'getBuffer',
    'invoke',
    'isDeliverable',
    'notify',
    'personaPlaceholderTitles',
    'recordAuthoritySyncDiagnostic',
    'runSyncOnSession',
    'scheduledRunSessionOwners',
    'sessionStates',
    'startThinking',
    'state',
  ],
  'tauriMain:99856': ['addChatItem', 'bt', 'pe'],
  'tauriMemory': [
    'addChatItem',
    'addSystemItem',
    'bt',
    'candidateCardPatcher',
    'invoke',
    'itemIdSeq',
    'memoryOverviewSeq',
    'notify',
    'runOnSession',
    'state',
    'timeStr',
  ],
  'tauriMonitor': [
    'bt',
    'clearMonitorBaseline',
    'gpuUtilHistory',
    'invoke',
    'monitorBaseline',
    'monitorIntervalId',
    'monitorPollInFlight',
    'notify',
    'sessionStates',
    'state',
  ],
  'tauriPersonas': [
    'addChatItem',
    'addSystemItem',
    'bt',
    'deletedPersonaIds',
    'ensureSession',
    'invoke',
    'isDefaultChatTitle',
    'itemIdSeq',
    'lastEquippedSid',
    'mountedCollectionDraftTarget',
    'mountedCollectionUpdate',
    'notify',
    'personaPlaceholderTitles',
    'personaPoolCache',
    'personaSyncSeq',
    'runOnSession',
    'sessionStates',
    'state',
    'timeStr',
  ],
  'tauriScheduled': [
    'SCHEDULED_LINK_POLL_DEADLINE_MS',
    'SCHEDULED_LINK_POLL_FAST_ATTEMPTS',
    'SCHEDULED_LINK_POLL_FAST_MS',
    'SCHEDULED_LINK_POLL_SLOW_MS',
    'SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY',
    'addSystemItem',
    'bt',
    'createNewSession',
    'forgetScheduledTaskTemplateSource',
    'invoke',
    'isScheduledRunTerminal',
    'notify',
    'persistScheduledTaskTemplateSources',
    'prefillComposer',
    'rememberScheduledRunOwner',
    'runSyncOnSession',
    'scheduledRecentRunsRequestToken',
    'scheduledRunEventRefreshTimer',
    'scheduledRunShortcutRefreshes',
    'scheduledTaskAutoCreateInFlight',
    'scheduledTaskAutoCreateSeq',
    'scheduledTaskBackendInput',
    'scheduledTaskPendingLoads',
    'scheduledTaskRefreshInFlight',
    'scheduledTaskRequestTokens',
    'scheduledTaskSelectionGeneration',
    'scheduledTaskTemplateSources',
    'state',
  ],
  'tauriSessions': [
    'MAX_EVICTED_SESSION_DRAFTS',
    'MAX_EVICTED_SESSION_DRAFT_CHARS',
    'MAX_SCHEDULED_RUN_SESSION_OWNERS',
    'addSystemItem',
    'basename',
    'bt',
    'currentStreamId',
    'currentStreamText',
    'enterDraft',
    'evictedSessionDrafts',
    'historyRefreshPending',
    'historyRefreshTimer',
    'invalidateScheduledRecentRunsForSession',
    'invoke',
    'itemIdSeq',
    'loadScheduledTaskRecentRuns',
    'notify',
    'openScheduledRunChatOnce',
    'pendingAssistantBlocks',
    'pendingAssistantText',
    'personaPlaceholderTitles',
    'pruneScheduledSessionBuffers',
    'pruneSessionBuffers',
    'purgeSessionBuffer',
    'refreshHistoryList',
    'reportSidecarReadFailure',
    'restoreEvictedSessionDraft',
    'restoreSteeredWorkingSet',
    'saveSteeredWorkingSet',
    'scheduledRunOpenInFlight',
    'scheduledRunOwnerTouchClock',
    'scheduledRunSessionOwners',
    'sessionBufferTouchClock',
    'sessionStates',
    'sessionSwitchRequestToken',
    'setScheduledTaskError',
    'state',
    'switchToSessionInternal',
    'toolMeta',
    'userMessageDisplayText',
  ],
  'tauriSessions:47342': ['state'],
  'tauriSettings': [
    'invoke',
    'loadSessionModel',
    'modelsLoadSeq',
    'notify',
    'setActiveModel',
    'settingsWriteQueue',
    'state',
  ],
  'tauriTerminal': [
    'SHELL_TOOL_NAMES',
    'addChatItem',
    'bt',
    'invoke',
    'latestShellToolIsWaitObserver',
    'normalizeTerminalTail',
    'notify',
    'runSyncOnSession',
    'shellPollState',
    'state',
  ],
  'tauriUpdater': ['invoke', 'notify', 'state'],
  'tauriVoice': [
    'VOICE_DEVICE_REQUEST_TIMEOUT_MS',
    'activeVoiceInput',
    'bt',
    'finishVoiceInput',
    'notify',
    'state',
    'stopMediaTracks',
    'voiceOperations',
  ],
  'web': [
    'BT_TABLE',
    'DELIVERABLE_EXTS',
    'MAX_EVICTED_SESSION_DRAFTS',
    'MAX_EVICTED_SESSION_DRAFT_CHARS',
    'MAX_SCHEDULED_RUN_SESSION_OWNERS',
    'PINVOU_SCENE_EVENTS_STORAGE_PREFIX',
    'SCHEDULED_LINK_POLL_DEADLINE_MS',
    'SCHEDULED_LINK_POLL_FAST_ATTEMPTS',
    'SCHEDULED_LINK_POLL_FAST_MS',
    'SCHEDULED_LINK_POLL_SLOW_MS',
    'SCHEDULED_TEMPLATE_SOURCE_STORAGE_KEY',
    'SHELL_TOOL_NAMES',
    'VOICE_DEVICE_REQUEST_TIMEOUT_MS',
    'activeVoiceInput',
    'addAttachmentByPath',
    'addChatItem',
    'applyAuthoritativeModeState',
    'artifactInfo',
    'candidateCardPatcher',
    'clearMonitorBaseline',
    'currentDraftModeState',
    'currentStreamId',
    'currentStreamText',
    'deletedPersonaIds',
    'dialogOpen',
    'doSendFor',
    'ensureSession',
    'ensureSessionBufferLoaded',
    'enterDraft',
    'evictedSessionDrafts',
    'finalizeAssistantStreamBeforeReasoning',
    'finishVoiceInput',
    'flushPendingTextBlock',
    'flushQueued',
    'forgetScheduledTaskTemplateSource',
    'gpuUtilHistory',
    'historyRefreshPending',
    'historyRefreshTimer',
    'invoke',
    'itemIdSeq',
    'lastEquippedSid',
    'latestShellToolIsWaitObserver',
    'loadSessionModel',
    'memoryOverviewSeq',
    'modelsLoadSeq',
    'monitorBaseline',
    'monitorIntervalId',
    'monitorPollInFlight',
    'mountedCollectionDraftTarget',
    'mountedCollectionUpdate',
    'normalizeTerminalTail',
    'notify',
    'openScheduledRunChatOnce',
    'pendingAssistantBlocks',
    'pendingAssistantText',
    'pendingStreamNotifications',
    'persistScheduledTaskTemplateSources',
    'personaPlaceholderTitles',
    'personaPoolCache',
    'personaSyncSeq',
    'pruneScheduledSessionBuffers',
    'pruneSessionBuffers',
    'purgeSessionBuffer',
    'pushUserEcho',
    'reconcileRemoteTurn',
    'recordAuthoritySyncDiagnostic',
    'refreshHistoryList',
    'reportSidecarReadFailure',
    'restoreEvictedSessionDraft',
    'runSyncOnSession',
    'scheduledRecentRunsRequestToken',
    'scheduledRunEventRefreshTimer',
    'scheduledRunOpenInFlight',
    'scheduledRunOwnerTouchClock',
    'scheduledRunSessionOwners',
    'scheduledRunShortcutRefreshes',
    'scheduledTaskAutoCreateInFlight',
    'scheduledTaskAutoCreateSeq',
    'scheduledTaskBackendInput',
    'scheduledTaskPendingLoads',
    'scheduledTaskRefreshInFlight',
    'scheduledTaskRequestTokens',
    'scheduledTaskSelectionGeneration',
    'scheduledTaskTemplateSources',
    'sendMessage',
    'sessionBufferTouchClock',
    'sessionStates',
    'sessionSwitchRequestToken',
    'setActiveModel',
    'setDraftMode',
    'settingsWriteQueue',
    'shellPollState',
    'state',
    'stopMediaTracks',
    'switchToSessionInternal',
    'toolMeta',
    'voiceOperations',
  ],
  'web:158313': ['state'],
  'web:189622': ['addChatItem', 'bt', 'pe'],
  'web:247496': ['sid', 'state'],
};

const EXPECTED_FORWARDER_DEP_NAMES = [
  'addSystemItem',
  'autoCreateScheduledTaskDraft',
  'basename',
  'bt',
  'createNewSession',
  'extractArtifactPaths',
  'flushAssistantMessageToHistory',
  'getBuffer',
  'invalidateScheduledRecentRunsForSession',
  'isBusyFor',
  'isDefaultChatTitle',
  'isDeliverable',
  'isScheduledRunSession',
  'isScheduledRunTerminal',
  'loadScheduledTaskRecentRuns',
  'prefillComposer',
  'rememberScheduledRunOwner',
  'runOnSession',
  'setScheduledTaskError',
  'startThinking',
  'timeStr',
  'touchSessionBuffer',
  'userMessageDisplayText',
];

// Blanket comment/string stripping keeps the extractor honest: literals may
// contain comments (aux-chat's does) and quoted braces.
function stripCommentsForScan(source) {
  let out = '';
  let i = 0;
  let mode = 'code';
  while (i < source.length) {
    const c = source[i];
    const n = source[i + 1];
    if (mode === 'code') {
      if (c === '/' && n === '/') { mode = 'line'; out += '  '; i += 2; continue; }
      if (c === '/' && n === '*') { mode = 'block'; out += '  '; i += 2; continue; }
      if (c === "'" || c === '"' || c === '`') mode = c;
      out += c; i++; continue;
    }
    if (mode === 'line') { if (c === '\n') { mode = 'code'; out += c; } else out += ' '; i++; continue; }
    if (mode === 'block') {
      if (c === '*' && n === '/') { mode = 'code'; out += '  '; i += 2; }
      else { out += c === '\n' ? '\n' : ' '; i++; }
      continue;
    }
    if (c === '\\') { out += c + (n ?? ''); i += 2; continue; }
    if (c === mode) mode = 'code';
    out += c; i++;
  }
  return out;
}

function extractLiteralKeys(source, createIdx) {
  const start = source.indexOf('{', createIdx);
  assert.ok(start !== -1, 'create( without a literal body');
  let depth = 0;
  let inQuote = null;
  for (let i = start; i < source.length; i++) {
    const c = source[i];
    if (inQuote) {
      if (c === '\\') { i++; continue; }
      if (c === inQuote) inQuote = null;
      continue;
    }
    if (c === "'" || c === '"' || c === '`') { inQuote = c; continue; }
    if (c === '{') depth++;
    else if (c === '}') {
      depth--;
      if (depth === 0) {
        const body = source.slice(start + 1, i);
        const fragments = [];
        let d = 0;
        let cur = '';
        let q = null;
        for (const ch of body) {
          if (q) { if (ch === '\\') continue; if (ch === q) q = null; continue; }
          if (ch === "'" || ch === '"' || ch === '`') { q = ch; continue; }
          if (ch === '{' || ch === '(' || ch === '[') d++;
          else if (ch === '}' || ch === ')' || ch === ']') d--;
          else if (ch === ',' && d === 0) { fragments.push(cur); cur = ''; continue; }
          cur += ch;
        }
        fragments.push(cur);
        return fragments.map((f) => f.trim()).filter(Boolean).map((f) => {
          const m = f.match(/^(?:async )?([A-Za-z_$][\w$]*)\s*:/)
            || f.match(/^([A-Za-z_$][\w$]*)$/)
            || f.match(/^(?:async )?([A-Za-z_$][\w$]*)\s*\(/);
          assert.ok(m, `unparseable deps literal fragment: ${JSON.stringify(f.slice(0, 80))}`);
          return m[1];
        });
      }
    }
  }
  assert.fail('unbalanced deps literal');
}

function walkLaneScripts(dir, out) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walkLaneScripts(p, out);
    else if (entry.name.endsWith('.js')) out.push(p);
  }
  return out;
}

const laneFiles = ['src/platform/tauri', 'src/platform/web'].flatMap(
  (dir) => walkLaneScripts(path.join(root, dir), []),
);
// Per-lane site presence (round-3 review): `seen` keyed by cluster name only
// tolerated a whole-lane site deletion whenever the other lane still created
// the same cluster — exactly the web-drift bug class this gate exists for.
// The map below pins the audited lane set of every cluster; nested
// `<base>:<offset>` alias keys are pinned like any other site.
const EXPECTED_CLUSTER_LANES = {
  'auxChat': ['tauri', 'web'],
  'tauriArtifactTracker': ['tauri'],
  'tauriArtifacts': ['tauri'],
  'tauriChat': ['tauri'],
  'tauriChat:31666': ['tauri'],
  'tauriChatEvents': ['tauri'],
  'tauriDependencies': ['tauri'],
  'tauriInteraction': ['tauri'],
  'tauriKnowledgeModel': ['tauri'],
  'tauriMain': ['tauri'],
  'tauriMain:99856': ['tauri'],
  'tauriMemory': ['tauri'],
  'tauriMonitor': ['tauri'],
  'tauriPersonas': ['tauri'],
  'tauriScheduled': ['tauri'],
  'tauriSessions': ['tauri'],
  'tauriSessions:47342': ['tauri'],
  'tauriSettings': ['tauri'],
  'tauriTerminal': ['tauri'],
  'tauriUpdater': ['tauri'],
  'tauriVoice': ['tauri'],
  'web': ['web'],
  'web:158313': ['web'],
  'web:189622': ['web'],
  'web:247496': ['web'],
};
const seen = new Map();
for (const file of laneFiles) {
  const raw = fs.readFileSync(file, 'utf8');
  // stripCommentsForScan is length-preserving, so AST offsets on the raw
  // source index the stripped text 1:1. Site discovery itself must use the
  // AST (round-3 review): the stripper's regex-literal blind spot could
  // surface commented-out create( lines as phantom sites, and a stuck
  // string mode could hide real ones.
  const source = stripCommentsForScan(raw);
  let ast;
  try {
    ast = acorn.parse(raw, { ecmaVersion: 'latest', sourceType: 'script' });
  } catch (err) {
    // ESM lane modules (e.g. startup-window.js) need module mode; every
    // other file must parse as a classic script like the lanes load.
    if (String(err).includes('import') || String(err).includes('export')) {
      ast = acorn.parse(raw, { ecmaVersion: 'latest', sourceType: 'module' });
    } else {
      throw err;
    }
  }
  const sites = [];
  (function walkNode(node) {
    if (!node || typeof node.type !== 'string') return;
    if (node.type === 'CallExpression'
      && ((node.callee?.type === 'Identifier' && node.callee.name === 'create')
        || (node.callee?.type === 'MemberExpression'
          && !node.callee.computed && node.callee.property?.name === 'create'))
      && node.arguments[0]?.type === 'Literal' && typeof node.arguments[0]?.value === 'string') {
      sites.push({ name: node.arguments[0].value, start: node.start });
    }
    for (const key of Object.keys(node)) {
      const child = node[key];
      if (Array.isArray(child)) child.forEach(walkNode);
      else if (child && typeof child.type === 'string') walkNode(child);
    }
  })(ast);
  for (const site of sites) {
    const rel = path.relative(root, file);
    const keys = extractLiteralKeys(source, site.start);
    const expected = EXPECTED_CLUSTER_KEYS[site.name];
    assert.ok(
      expected,
      `unknown bridge cluster "${site.name}" created in ${rel} — add its audited ` +
      'key set to EXPECTED_CLUSTER_KEYS (header comment has the procedure)',
    );
    assert.deepEqual(
      [...keys].sort(cmpNames),
      [...expected].sort(cmpNames),
      `the "${site.name}" deps literal in ${rel} drifted from the audited surface — ` +
      're-run the dep audit (header comment) and update EXPECTED_CLUSTER_KEYS; ' +
      'a missing key binds undefined in the shared bodies, an extra key is ' +
      'silently ignored dead wiring',
    );
    const lane = rel.startsWith('src/platform/tauri/') ? 'tauri' : 'web';
    if (!seen.has(site.name)) seen.set(site.name, new Set());
    seen.get(site.name).add(lane);
  }
}
for (const [cluster, lanes] of Object.entries(EXPECTED_CLUSTER_LANES)) {
  const got = [...(seen.get(cluster) ?? [])].sort(cmpNames);
  assert.deepEqual(
    got,
    [...lanes].sort(cmpNames),
    `cluster "${cluster}" must have a create( site in exactly these lanes ` +
    `${lanes.join('+')} (found: ${got.join('+') || 'none'}) — a lane that ` +
    'loses its literal silently drifts from the shared body it should pin',
  );
}
for (const cluster of seen.keys()) {
  assert.ok(
    EXPECTED_CLUSTER_LANES[cluster],
    `cluster "${cluster}" gained an unexpected lane presence — update ` +
    'EXPECTED_CLUSTER_LANES after re-running the dep audit',
  );
}
for (const cluster of Object.keys(EXPECTED_CLUSTER_KEYS)) {
  assert.ok(
    seen.has(cluster),
    `cluster "${cluster}" has no create( site left in either lane — ` +
    'remove it from EXPECTED_CLUSTER_KEYS or restore the literal',
  );
}

// FORWARDER_DEP_NAMES routes same-named lane forwarders past the base
// bindings; the list is load-bearing wiring, so pin it exactly (the old
// >= 20 length bound tolerated removals that would make lanes passing the
// name throw "forwarderSetters[name] is not a function" at create() time).
assert.deepEqual(
  forwarderNames,
  EXPECTED_FORWARDER_DEP_NAMES,
  'FORWARDER_DEP_NAMES drifted from the audited list — re-run the dep audit ' +
  '(header comment); lanes passing a removed name crash at create() time',
);

// forwarderSetters must implement exactly the names FORWARDER_DEP_NAMES
// routes (round-3 review): a name listed but missing its setter throws
// "forwarderSetters[name] is not a function" at create() time, and a setter
// with no routed name is dead wiring the list pin cannot see. Both lists
// come from the AST walk above.
assert.deepEqual(
  setterNames.sort(cmpNames),
  [...forwarderNames].sort(cmpNames),
  'forwarderSetters keys and FORWARDER_DEP_NAMES must match exactly — a ' +
  'listed name without a setter crashes every lane at create() time',
);

console.log('bridge lane cluster dep contracts passed');
