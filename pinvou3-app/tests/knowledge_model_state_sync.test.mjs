import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const readHelpers = () => readFileSync(
  path.join(appRoot, 'src/shared/bridge-shared-helpers.js'),
  'utf8',
);
const bridgeSource = readHelpers() + '\n' + readFileSync(
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
const webBridge = readHelpers() + '\n' + readFileSync(
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
  // kb_model:status / kb_model:progress 是 Web lane 的 never-emitted 事件（不在
  // access-policy allowed_events / RUST_FORWARDED_EVENTS），其监听器已随 dead-code
  // sweep 移除；桌面模型状态同步由 Tauri lane 的同名监听器承担（上方 tauriBridge pin）。
  assert.doesNotMatch(webBridge, /listen\("kb_model:status"/u);
  assert.doesNotMatch(webBridge, /listen\("kb_model:progress"/u);
  // dedup 后 installBridgeFeature 的 deps 里可能出现 get/set 单元格（嵌套大括号），
  // 单个正则不再可靠；改为定位安装点后在其邻域内确认 listen 已传入。
  const kmInstallAt = tauriBridge.indexOf('installBridgeFeature("knowledge-model"');
  assert.notStrictEqual(kmInstallAt, -1, 'knowledge-model feature must be installed');
  assert.match(
    tauriBridge.slice(kmInstallAt, kmInstallAt + 2000),
    /\blisten\b/u,
    'knowledge-model feature must receive listen',
  );
});
