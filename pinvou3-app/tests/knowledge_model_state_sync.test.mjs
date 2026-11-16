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
const sharedKnowledgeHostFeature = readFileSync(
  path.join(appRoot, 'src-tauri/src/features/shared_knowledge_host/mod.rs'),
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
    /pub async fn kb_model_download\([\s\S]*item\.endpoint == LOCAL_ENDPOINT[\s\S]*remote\.download_model/u,
  );
  // The kb_model_download host refresh filter, the owner register/rebind,
  // and the host state report share the same LOCAL_ENDPOINT constant (the
  // dead-code sweep folded three 127.0.0.1:3210 literals into it); the
  // endpoint value is anchored at its declaration in
  // features/shared_knowledge_host/mod.rs.
  assert.match(
    sharedKnowledgeHostFeature,
    /pub\(crate\) const LOCAL_ENDPOINT: &str = "https:\/\/127\.0\.0\.1:3210"/u,
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

test('cancelled model downloads settle as neutral state and cancel returns a promise', async () => {
  const windowObject = { __PINVOU_TAURI_BRIDGE_FEATURES__: {} };
  vm.runInNewContext(bridgeSource, { window: windowObject }, { filename: 'knowledge-model.js' });

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
  const feature = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['knowledge-model']({
    state,
    notify() {},
    invoke: async (command) => {
      // tauri 对 Result<_, String> 以原始字符串拒绝，不是 Error 对象——
      // 仿真这一拒绝形态正是本钉的目的，故豁免 reject-errors 规则。
      // eslint-disable-next-line prefer-promise-reject-errors -- raw-string rejection is the tauri contract under test
      if (command === 'kb_model_download') return Promise.reject('cancelled');
      // 取消命令本身也拒绝（web/旧后端的 policy-denial 形态）：cancelKbModel
      // 必须吞掉并 resolve，"永不 reject" 在拒绝式 mock 下才有牙。
      // eslint-disable-next-line prefer-promise-reject-errors -- raw-string rejection is the tauri contract under test
      if (command === 'kb_model_cancel') return Promise.reject('commandNotAllowed: kb_model_cancel');
      if (command === 'kb_model_status') return { installed: true, ready: false, loading: false };
      return null;
    },
    listen() {},
  });

  // 用户取消（后端哨兵 "cancelled"）不是失败：记为中性 cancelled 态而非红色
  // error，downloading 复位，拒绝仍原样上抛给调用方。
  await assert.rejects(
    () => feature.downloadKbModel(false),
    (error) => String(error) === 'cancelled',
  );
  assert.equal(state.kbModelSetup.cancelled, true);
  assert.equal(state.kbModelSetup.error, null);
  assert.equal(state.kbModelSetup.downloading, false);

  // 失败路径不受影响：非取消拒绝仍写 error。
  state.kbModelSetup.downloading = false;
  const failing = windowObject.__PINVOU_TAURI_BRIDGE_FEATURES__['knowledge-model']({
    state,
    notify() {},
    invoke: async (command) => {
      // eslint-disable-next-line prefer-promise-reject-errors -- raw-string rejection is the tauri contract under test
      if (command === 'kb_model_download') return Promise.reject('网络错误');
      if (command === 'kb_model_status') return null;
      return null;
    },
    listen() {},
  });
  await assert.rejects(
    () => failing.downloadKbModel(false),
    (error) => String(error) === '网络错误',
  );
  assert.equal(state.kbModelSetup.error, '网络错误');
  assert.equal(state.kbModelSetup.cancelled, false);

  // 取消等待语义：cancelKbModel 永不 reject——上面的 mock 连 kb_model_cancel
  // 本身都以 policy-denial 拒绝，吞噬在此钉死。web lane 的内联副本同形，
  // 其存在性由 web_bridge_domain_contract 钉。
  assert.equal(typeof feature.cancelKbModel().then, 'function');
  await feature.cancelKbModel();
});
