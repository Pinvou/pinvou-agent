import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { desktopBridgeApi } from './bridge_domain_contract.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeRoot = path.join(root, 'src', 'platform', 'tauri');

function read(relativePath) {
  return fs.readFileSync(path.join(bridgeRoot, relativePath), 'utf8');
}

function extractCalls(source, callee) {
  const calls = [];
  const needle = `${callee}(`;
  let cursor = 0;
  while ((cursor = source.indexOf(needle, cursor)) !== -1) {
    const previous = source[cursor - 1] || '';
    if (/[A-Za-z0-9_$]/.test(previous)) {
      cursor += needle.length;
      continue;
    }
    let index = cursor + needle.length;
    let depth = 1;
    let quote = null;
    let escaped = false;
    let lineComment = false;
    let blockComment = false;
    for (; index < source.length && depth > 0; index += 1) {
      const char = source[index];
      const next = source[index + 1];
      if (lineComment) {
        if (char === '\n') lineComment = false;
        continue;
      }
      if (blockComment) {
        if (char === '*' && next === '/') { blockComment = false; index += 1; }
        continue;
      }
      if (quote) {
        if (escaped) escaped = false;
        else if (char === '\\') escaped = true;
        else if (char === quote) quote = null;
        continue;
      }
      if (char === '/' && next === '/') { lineComment = true; index += 1; continue; }
      if (char === '/' && next === '*') { blockComment = true; index += 1; continue; }
      if (char === '"' || char === "'" || char === '`') { quote = char; continue; }
      if (char === '(') depth += 1;
      else if (char === ')') depth -= 1;
    }
    assert.equal(depth, 0, `unclosed ${callee} call near offset ${cursor}`);
    calls.push(source.slice(cursor, index).replace(/\s+/g, ' ').trim());
    cursor = index;
  }
  return calls;
}

const protocolSources = {
  orchestration: ['bridge.js'],
  artifacts: ['bridge/artifact-tracker.js', 'bridge/artifacts.js'],
  chat: ['bridge/chat.js', 'bridge/chat-events.js', 'bridge/terminal.js'],
  dependencies: ['bridge/dependencies.js'],
  interaction: ['bridge/interaction.js'],
  knowledge: ['bridge/knowledge-model.js'],
  memory: ['bridge/memory.js'],
  monitor: ['bridge/monitor.js'],
  personas: ['bridge/personas.js'],
  remoteControl: ['bridge/remote-control.js'],
  scheduled: ['bridge/scheduled.js'],
  sessions: ['bridge/sessions.js'],
  settings: ['bridge/settings.js'],
  updater: ['bridge/updater.js'],
  voice: ['bridge/voice.js'],
  multiAgent: ['bridge/multiagent.js'],
  projects: ['bridge/projects.js'],
  computerUse: ['bridge/computer_use.js'],
};

const expectedProtocolHashes = {
  // Batch-A dead-code/dedup sweep: byte-identical helpers shared between the web and
  // tauri lanes moved verbatim into src/shared/bridge-shared-helpers.js (loaded by
  // index.html before both bridges). The moved bodies carry their invoke( calls with
  // them, so per-domain captures shrink by exactly the relocated call sites — every
  // dropped signature still executes at runtime via the shared payload, and the
  // command + payload text of each invoke is unchanged. Hashes below marked
  // "Recomputed for the shared-helper dedup" reflect that relocation only.
  // New domain: computer-use consent commands/events (desktop-only; the web
  // RPC allowlist excludes them, same policy as browser:*).
  // Recomputed when the 30s deny-suppression cooldown was removed: a deny now
  // only closes its own dialog and fresh requests re-prompt immediately (no
  // invoke/listen call-set change; comment wording is part of the digest).
  // Recomputed when the deny()/grant_required handler comments were updated
  // for the mainstream narrowing (no deny-cooldown memory, no idle-expired
  // grant re-arm; comment wording inside the listen callback span is part of
  // the digest). Recomputed again when the confirm_required handler's
  // type-preview comment was corrected to the shipped contract (full text
  // rides for every non-password Type action, short texts included).
  // Recomputed for the fresh-review fixes: the grant/confirm_required inert branches
  // now re-read authoritative status (other-window enable gap) and sameRequest compares
  // the typed-text preview (comment and expression wording inside the callback spans
  // is part of the digest; no invoke/listen call-set change).
  // Recomputed for the structured confirm payload: confirm_required builds the
  // request via buildConfirmRequest (action/button/click_count/point/text_preview/
  // text_preview_truncated passthrough + summary fallback) and dismissConfirm was
  // removed (no invoke/listen call-set change; listener body wording is part of
  // the digest).
  // Recomputed for the round-15 fix wave: secure-target chord masking (the
  // chord_masked_chars passthrough in buildConfirmRequest), the
  // background-session refresh guard, and the inert-branch optional chaining
  // inside the listener callback spans (no invoke/listen call-set change).
  computerUse: '9d0bad5bab784eb784a92b8df52ff7e83adef94cc0f6cb7bdbb99f913217e5b8',
  multiAgent: 'a6d045e87f7f5f3537fdeadb262d54622edd6dcafa2c0253f0b44e7de439315d',
  // Recomputed for the shared-helper dedup (see batch note above).
  orchestration: '341efb3b1e4a4036269559294c33b76a744bcde7c3903b9ba3525711d6182f6f',
  // Recomputed for the shared-helper dedup (see batch note above).
  artifacts: '852935f8094d353fef871d1db804c7a4ed5fbefeaf9cb6af9f0c3932f850e259',
  // Recomputed for #308 follow-ups: prefillComposer(text, append) recovery
  // entry + comment translations touching `invoke(` mentions (the extractor
  // scans raw source, so comment wording is part of the digest). Recomputed
  // again for the eighth-review fixes: zap resend gating on the in-flight
  // steer settlement, the per-sid zap entry guard, and the accompanying
  // comment wording. And again for the streaming-freeze follow-ups: the
  // settlement side table (a Promise on the queued chip poisoned the
  // subscription snapshot), steeredMidTurn bubble markers, and the hydration
  // envelope strip. And again for the r13 follow-ups: the transcript
  // fallback's legacy-chip pre-check (chat-events.js) and the zap
  // skip-resend settlement helper (chat.js). And again for the steer
  // persistence alignment: the steered-messages sidecar save/get invokes
  // (bridge.js) and the position-capture load_session (chat.js). And again
  // for the background-task indicator: the multiagent:agent_progress listen
  // that schedules the shell snapshot poll for sub-agent spawned tasks
  // (chat-events.js). Recomputed for explicit artifact presentation: a
  // successful present_artifact tool_end now emits a session-scoped request
  // that opens the preview even when the existing card is updated in place.
  // And again for the model-service error notices: chat-events.js gains
  // chat:transient_error redaction/listener body changes (the extractor
  // scans raw source, so comment wording is part of the digest). Recomputed
  // on the latest-main rebase: main's shell-task isolation changes touched
  // the same listeners, and the PR's developer comments were translated to
  // English, both of which shift the raw-source digest. Recomputed for the
  // v0.9.12 rereview fix that persists toolName/reason/risk with each
  // ToolGateDecision so history replay retains the structured audit detail.
  // Recomputed again when the shell background-task projection learned the
  // canonical v0.9.12 `bash` name while retaining legacy replay aliases. And
  // again for the streaming markdown render throttle: the chat:delta /
  // chat:tool_start / chat:done listener bodies gained the trailing-edge
  // render flush/schedule plus the unpaired-toolMeta terminal sweep — no new
  // invoke or listen entries, only body-internal edits, so this is a
  // capture-text refresh. Recomputed again for the chat:tool_end listener
  // bodies emitting the final stream html via flushPendingStreamRender
  // before resetting the stream state (same throttle invariant, still no new
  // invoke or listen entries). Recomputed again for the review-follow-up
  // comment translations inside the captured listener bodies (same
  // capture-text refresh; the comment-stripped signature list is
  // byte-identical to the previous state). Recomputed for the audit dead-code
  // cleanup: persistMessages factory removed, the steer watchdog timeout-map
  // scaffold extracted behind the same arm/clear/purge/isArmed entry points,
  // and the never-emitted remote_control:mobile_user_message / vllm-setup:phase
  // listeners dropped. The captured surface shrank by persistMessages'
  // save_session_messages/save_session_artifacts/rename_session invokes, the
  // terminal.js cancel_shell_task invoke, and the two dead listeners.
  // save_session_artifacts/rename_session stay reachable via bridge.js
  // orchestration paths (the session-switch persist flow issues both), while
  // save_session_messages had no remaining caller, so its Rust command is
  // retired with it; the exposed bridge.chat API is unchanged).
  // Recomputed for the shared-helper dedup (see batch note above).
  chat: '2258b9ed785b1a73bfd271427689c7db902edc5fa0d06eab47c28a11e7f40b86',
  // Recomputed for the shared-helper dedup (see batch note above).
  dependencies: 'bcc3fb2ec60c5e80df5ac86bc8b4e14c810aa449d5ee5f4e3bc8ab1f32ffdff3',
  // Recomputed for #445 round-8: exitPlanToYolo accepts an explicit target
  // session id (the YOLO gate passes the adjudicated sid), so the
  // exit_plan_to_yolo invoke payload text changes from
  // { sessionId: state.activeSessionId } to { sessionId: sid } — same
  // command surface, no new invoke or listen entries.
  // Recomputed for the shared-helper dedup (see batch note above).
  interaction: '9b330edc21e6db76559a368cc54fd22b930c6e3fc0f714141aaa763dcb5fd0c9',
  // Recomputed for the shared-helper dedup (see batch note above).
  knowledge: 'f9e241acb18d04c8d5d7d71b18d3d0350c3264bbc50e896b0f21b0e036286868',
  // memory recomputed for the memory-maintenance feature: organize_memory +
  // get_memory_organize_history invokes added to bridge/memory.js.
  // Recomputed for the shared-helper dedup (see batch note above).
  // Recomputed again for the dead-code sweep: the dead deleteMemoryPreference facade
  // (delete_memory_preference invoke) was removed; deleteMemoryItem('preference', ...)
  // still routes to the same command, so the backend surface is unchanged.
  memory: '30cab38634446a7bef24d559db799641d115e0f07a22a28540a5fd48a9ff7347',
  monitor: '01bf9a7c9b9b3f313cf49e975e6503627ff373caed0f4b3be07a6a98492a7c43',
  // Recomputed for the shared-helper dedup (see batch note above).
  personas: 'c168ac5ede23cb76ef6a93b5323ca4837b4d97395d5e5ca9c2eabf4eb40dd01f',
  // Recomputed for the voice error-code relay: web_access_rpc_respond gains
  // errorCode/errorCategory so structured VoiceCommandError identity reaches
  // the browser lane's trilingual mapping instead of only the message text.
  // Recomputed again for the dead-code sweep: the JS-side resetWebRelayAddress
  // facade (web_access_reset_relay invoke) was removed — no production caller,
  // and the Rust command is removed in parallel by the backend sweep.
  remoteControl: '099f4c07968f53331bac9e51b7379ab9fc8c3db4a7102d9629a652c26b5c99ae',
  // Recomputed for the shared-helper dedup (see batch note above).
  scheduled: '28247031a7401d232000c469c1eed0954c2eb1265817e7d0a1a5fd5768f5871a',
  // Recomputed for one-click full-fidelity session log export: the tauri
  // sessions bridge gains the export_session_archive invoke wrapping the
  // export_session command (web lane intentionally has no such backend).
  // Recomputed again for the normal-chat draft workspace selector:
  // create_session now carries the optional workspacePath payload
  // (bridge/sessions.js). Recomputed again for workspace-bound sessions:
  // get_session_workspace_binding query + bound-draft staged mode application
  // (set_plan_mode_next / exit_plan_to_yolo) at materialization.
  // Recomputed again for the single-entry workspace picker: create_session now
  // also carries the keychain snapshot (workspaceRoots) and project ownership
  // (projectId) captured from the draft, and the draft staging gains
  // draftWorkspaceRoots/draftProjectId (bridge/sessions.js).
  sessions: '2e6d8899b5fee3e6cf74a3a048ec2f48d6fa57872b6f0e59b1690fb0e6a645fa',
  settings: 'a5c68eadcad49dd2f3e58157d0262209610696fb0e37f0b8e6888a97199729b5',
  // Recomputed for the audit dead-code cleanup: the never-emitted
  // remote_control:status / remote_control:session_created listeners were
  // removed (update:progress stays — pinned by tests/updater_progress_state).
  // Recomputed for the shared-helper dedup (see batch note above).
  updater: '30a3762ee45f378d476efec934dc4be50522a02a5b9a97111191496510d4d7d4',
  // Recomputed for the comment-only English translation of the voice bridge
  // (PR-added Chinese comments inside the postprocess_voice_text invoke
  // span are part of the hashed source; no invoke/listen surface changed).
  voice: '2a2e8d12150ca86bb970ad099e7b72ab6491768bbc42354cd5ecc800c891c733',
  // Recomputed for the rebind carryover feed-back (review #463 F-Major):
  // rebind_workspace_root gains the optional previousPostBusySessionIds
  // payload — the dialog's previous report fed back on retry, honored by the
  // backend only as a reporting reclassification inside its own to-lane
  // retry population. Same command surface, no new invoke or listen entries.
  // Recomputed again after the single-entry workspace picker merge: on top of
  // the rebind carryover feed-back the domain also gains
  // ensure_folder_projects / update_project(roots, lastPrimaryRoot) /
  // projects_set_never_materialize / align_session_to_project
  // (bridge/projects.js).
  projects: '04a4d40cc4d870c691ae9c524241f0b340ab62afdef413af9e4a249ae8771413',};

for (const [domain, files] of Object.entries(protocolSources)) {
  const signatures = files.flatMap(file => {
    const source = read(file);
    return [
      ...extractCalls(source, 'invoke').map(call => `${file}:invoke:${call}`),
      ...extractCalls(source, 'listen').map(call => `${file}:listen:${call}`),
    ];
  });
  const hash = crypto.createHash('sha256').update(signatures.join('\n')).digest('hex');
  if (!expectedProtocolHashes[domain]) console.log(`${domain}: ${hash}`);
  else assert.equal(hash, expectedProtocolHashes[domain], `${domain} bridge protocol changed`);
}

// The shared payload base carries the invoke/listen bodies that the batch dedup
// relocated out of the per-lane files, so the domain hashes above no longer
// cover that text. Hash the shared file's surface with the same extractor to
// pin payload edits inside the shared base the same way the lane files are.
const expectedSharedBaseHash = '4e63e15a4ec6b41f2f38a667d9e94d9761c230ab927e91d0c5ae49f3c4abedb4';
const sharedBaseSource = fs.readFileSync(path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8');
const sharedBaseSignatures = [
  ...extractCalls(sharedBaseSource, 'invoke').map(call => `shared/bridge-shared-helpers.js:invoke:${call}`),
  ...extractCalls(sharedBaseSource, 'listen').map(call => `shared/bridge-shared-helpers.js:listen:${call}`),
];
const sharedBaseHash = crypto.createHash('sha256').update(sharedBaseSignatures.join('\n')).digest('hex');
if (!expectedSharedBaseHash) console.log(`sharedBase: ${sharedBaseHash} (${sharedBaseSignatures.length} signatures)`);
else assert.equal(sharedBaseHash, expectedSharedBaseHash, 'shared bridge payload protocol changed');

const featureRegistry = new Proxy({}, {
  get() {
    return () => new Proxy({}, { get: () => function () {} });
  },
});
const windowObject = {
  __TAURI__: {
    core: { invoke: async () => null },
    event: { listen: async () => function () {} },
    dialog: { open: async () => null },
  },
  __PINVOU_TAURI_BRIDGE_FEATURES__: featureRegistry,
  location: { search: '' },
  performance: { now: () => 0 },
  setTimeout,
  clearTimeout,
};
const context = vm.createContext({
  window: windowObject,
  document: { readyState: 'loading', addEventListener() {} },
  console,
  setTimeout,
  clearTimeout,
  structuredClone,
  URL,
  Blob,
});
// bridge.js delegates shared helpers to window.PinvouBridgeShared; index.html loads
// the shared payload before the bridges, so the harness loads it first too.
vm.runInContext(fs.readFileSync(path.join(root, 'src', 'shared', 'bridge-shared-helpers.js'), 'utf8'),
  context, { filename: 'shared/bridge-shared-helpers.js' });
vm.runInContext(read('bridge.js'), context, { filename: 'bridge.js' });

const api = windowObject.TauriBridge;
assert.deepEqual(Object.keys(api).sort(), ['available', ...Object.keys(desktopBridgeApi)].sort()); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
for (const [domain, methods] of Object.entries(desktopBridgeApi)) {
  assert.deepEqual(Object.keys(api[domain]).sort(), methods.sort(), `${domain} API surface changed`); // eslint-disable-line unicorn/require-array-sort-compare -- lexicographic string order is the assertion's expectation
}
assert.equal(api.sendMessage, undefined, 'flat compatibility facade must not return');
assert.equal(api.getState, undefined, 'flat state facade must not return');

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const absolute = path.join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(absolute);
    return /\.(?:js|jsx)$/.test(entry.name) ? [absolute] : [];
  });
}
for (const file of sourceFiles(path.join(root, 'src'))) {
  if (file.startsWith(bridgeRoot)) continue;
  const source = fs.readFileSync(file, 'utf8');
  assert.doesNotMatch(
    source,
    /\bbridge\.[A-Za-z_$][\w$]*\s*\(/,
    `${path.relative(root, file)} must not call the removed flat bridge facade`,
  );
  if (file.startsWith(path.join(root, 'src', 'features'))) {
    assert.doesNotMatch(
      source,
      /\b(?:window|globalThis)\s*\.\s*__TAURI__\b/,
      `${path.relative(root, file)} must use the platform Tauri client`,
    );
  }
  for (const match of source.matchAll(/\bbridge\.([A-Za-z_$][\w$]*)\.([A-Za-z_$][\w$]*)/g)) {
    const [, domain, method] = match;
    assert.equal(typeof api[domain]?.[method], 'function', `${path.relative(root, file)} uses unknown bridge API ${domain}.${method}`);
  }
}

const clientSource = read('client.js');
const client = await import(`data:text/javascript;base64,${Buffer.from(clientSource).toString('base64')}`);
const previousTauri = globalThis.__TAURI__;
const nativeCalls = [];
class PhysicalPosition {
  constructor(x, y) { this.x = x; this.y = y; }
}
const currentWindow = { label: 'main' };
globalThis.__TAURI__ = {
  core: { invoke: async (command, payload) => { nativeCalls.push(['invoke', command, payload]); return 'ok'; } },
  event: {
    listen: async (name, handler) => { nativeCalls.push(['listen', name, handler]); return () => {}; },
    emit: async (name, payload) => { nativeCalls.push(['emit', name, payload]); },
  },
  window: {
    getCurrentWindow: () => currentWindow,
    currentMonitor: async () => ({ name: 'primary' }),
    availableMonitors: async () => [{ name: 'primary' }],
    PhysicalPosition,
  },
};
try {
  assert.equal(client.isTauriAvailable(), true);
  assert.equal(await client.invokeTauri('protocol_probe', { value: 1 }), 'ok');
  await client.listenTauri('protocol:event', () => {});
  await client.emitTauri('protocol:emit', { value: 2 });
  assert.equal(client.getCurrentTauriWindow(), currentWindow);
  assert.deepEqual(await client.currentTauriMonitor(), { name: 'primary' });
  assert.deepEqual(await client.availableTauriMonitors(), [{ name: 'primary' }]);
  const position = client.createPhysicalPosition(10.6, -2.4);
  assert.equal(position.x, 11);
  assert.equal(position.y, -2);
  assert.deepEqual(nativeCalls.slice(0, 3).map(call => call.slice(0, 2)), [
    ['invoke', 'protocol_probe'],
    ['listen', 'protocol:event'],
    ['emit', 'protocol:emit'],
  ]);
} finally {
  if (previousTauri === undefined) delete globalThis.__TAURI__;
  else globalThis.__TAURI__ = previousTauri;
}
console.log('bridge domain API and protocol contracts passed');
