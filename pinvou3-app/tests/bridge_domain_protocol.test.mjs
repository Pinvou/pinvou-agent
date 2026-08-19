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
};

const expectedProtocolHashes = {
  multiAgent: '6896409bb406f1ba54ca496dd78c04549cc78ff5490ae771d9cf60586368a8a8',
  orchestration: 'e5e333aca4d1fb7e8ed32f879d3b310c01cd0845d0e5cdc2b5ed1e95aee3ea31',
  artifacts: '8ea6cbffcc1a768f79cd218dc02d2f211456fe09c7f2aa233587603ba34c4f4e',
  chat: '090ca99a3aaf1977bf8274ddbaa22f863ae77e50ae72cea6ad0724ea5167c28a',
  dependencies: '257468e4f9e2e9270de6ef75f685d5eafcd000226d44cfb81a1b04c0e7615707',
  interaction: '255474b88418fcaf371b77b27545fbfb529fbbceb1af9010ac49881082384270',
  knowledge: '96d4f6a1534f0aedf10714e7b945e0a29579b224ec98cbf27659237b5aac59b6',
  memory: 'd92cbabf27c277a64b743e7af25b48d8b8b65513e33aeb0f38c906d4b300616b',
  monitor: '01bf9a7c9b9b3f313cf49e975e6503627ff373caed0f4b3be07a6a98492a7c43',
  personas: '75845688a405c5dc3590e176238969d846d61810eaeb3d632a0a741d30370239',
  remoteControl: '3e8d54d1051d1f59d5b9f41440b73444b82594391e47f4cbce56904afd72fb81',
  scheduled: '239292d75c308973053cc0091e0ac9437191bf2375fd5fd8181ea26f4f749900',
  sessions: '43be5b929743f00818492d51d7a725860c71b9a52e7fdcbe72606426472409c0',
  settings: 'ea86f1145e200d58de340bf76650eee96fd8169168872ed71def166d42ef2208',
  updater: '53562c8fe6547a6c422d112d34769d3ac79abeec27633c32b5658605072c9fe2',
  voice: 'ffb4ae594da3d6fa334e753e46328e9f1930e891e34c1b59ef2688a19f45cbd8',
};

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
vm.runInContext(read('bridge.js'), context, { filename: 'bridge.js' });

const api = windowObject.TauriBridge;
assert.deepEqual(Object.keys(api).sort(), ['available', ...Object.keys(desktopBridgeApi)].sort());
for (const [domain, methods] of Object.entries(desktopBridgeApi)) {
  assert.deepEqual(Object.keys(api[domain]).sort(), methods.sort(), `${domain} API surface changed`);
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
