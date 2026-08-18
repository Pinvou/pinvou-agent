import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const bridgeSource = readFileSync(
  path.join(appRoot, 'src/platform/tauri/bridge/knowledge-model.js'),
  'utf8',
);
const knowledgeCommands = readFileSync(
  path.join(appRoot, 'src-tauri/src/app/commands/knowledge.rs'),
  'utf8',
);
const remoteCommands = readFileSync(
  path.join(appRoot, 'src-tauri/src/app/commands/remote_knowledge.rs'),
  'utf8',
);
const webBridge = readFileSync(
  path.join(appRoot, 'src/platform/web/bridge.js'),
  'utf8',
);
const tauriBridge = readFileSync(
  path.join(appRoot, 'src/platform/tauri/bridge.js'),
  'utf8',
);

test('authoritative model status replaces the stale startup snapshot', () => {
  const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(bridgeSource, { window: windowObject }, { filename: 'knowledge-model.js' });

  const listeners = new Map();
  const state = {
    kbModelSetup: {
      downloading: false,
      startupLoading: false,
      startupReady: false,
      status: { installed: false, ready: false, loading: false },
      progress: null,
      error: null,
    },
  };
  let notifications = 0;
  windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['knowledge-model']({
    state,
    notify() { notifications += 1; },
    invoke: async () => null,
    listen(name, handler) { listeners.set(name, handler); },
  });

  listeners.get('kb_model:status')({
    payload: { installed: true, ready: true, loading: false, failed: false },
  });

  assert.equal(state.kbModelSetup.status.installed, true);
  assert.equal(state.kbModelSetup.startupReady, true);
  assert.equal(state.kbModelSetup.startupLoading, false);
  assert.equal(notifications, 1);
});

test('status queries and completed host downloads synchronize the desktop model', () => {
  assert.match(
    knowledgeCommands,
    /pub async fn kb_model_status\([\s\S]*model_installed\(\)[\s\S]*load_installed_embedder[\s\S]*app\.emit\("kb_model:status", &status\)/u,
  );
  assert.match(
    knowledgeCommands,
    /pub async fn kb_model_download\([\s\S]*https:\/\/127\.0\.0\.1:3210[\s\S]*remote\.download_model/u,
  );
  assert.match(
    remoteCommands,
    /pub async fn remote_kb_model_status\([\s\S]*if !remote_status\.downloading \{[\s\S]*sync_peer_installed_local_model/u,
  );
  assert.match(
    remoteCommands,
    /sync_peer_installed_local_model[\s\S]*local_model::model_installed\(\)[\s\S]*local_model::load_installed_embedder[\s\S]*app\.emit\("kb_model:status"/u,
  );
  assert.match(webBridge, /listen\("kb_model:status"/u);
  assert.match(
    tauriBridge,
    /installBridgeFeature\("knowledge-model", \{[^}]*listen: listen[^}]*\}\)/u,
  );
});
