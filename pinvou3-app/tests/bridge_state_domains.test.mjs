// Contract: a desktop bridge feature that writes bridge-state fields must be
// wired through the whole chain, or the feature is dead at runtime:
//
//   bridge/<feature>.js writes state.<field>
//     -> STATE_SLICE_FIELDS declares the domain with those fields
//        (snapshotStateSlice throws on unknown domains)
//     -> APP_BRIDGE_STATE_DOMAINS subscribes the domain in the app
//        (useBridgeState never surfaces unsubscribed domains)
//
// The projects feature shipped the feature file without either registration
// and every lane stayed green — this file locks the mapping so that cannot
// recur. Source-regex based on purpose: the constants live in classic scripts
// (bridge.js) and the app bundle (main.jsx) with no module boundary to import.
//
// Scope note: older features still write ephemeral fields that no slice
// declares (state.activeTurnTimelineId, state.modeLane, state.modeDefaults,
// state.pendingDraftMultiAgent, state.pinvouSceneEvents, state.steeredMessages,
// state.remoteControl in updater.js). None of them is consumed through
// state.getMany today; registering or removing them is its own cleanup, not
// silently bundled here.
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';

import { desktopOnlyBridgeDomains } from './bridge_domain_contract.mjs';

const read = (relative) =>
  readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

function parseSliceFields(source) {
  const match = source.match(/const STATE_SLICE_FIELDS = \{([\s\S]*?)\n {2}\};/);
  assert.ok(match, 'STATE_SLICE_FIELDS not found in bridge.js');
  const domains = {};
  for (const entry of match[1].matchAll(/(\w+): \[([^\]]*)\]/g)) {
    domains[entry[1]] = entry[2]
      .split(',')
      .map((field) => field.trim().replace(/^"|"$/g, ''))
      .filter(Boolean);
  }
  return domains;
}

function parseAppDomains(source) {
  const match = source.match(/const APP_BRIDGE_STATE_DOMAINS = \[([^\]]*)\]/);
  assert.ok(match, 'APP_BRIDGE_STATE_DOMAINS not found in main.jsx');
  return match[1]
    .split(',')
    .map((domain) => domain.trim().replace(/^'|'$/g, ''))
    .filter(Boolean);
}

test('STATE_SLICE_FIELDS domains and app subscription domains match', () => {
  const sliceDomains = Object.keys(parseSliceFields(read('platform/tauri/bridge.js')));
  const appDomains = parseAppDomains(read('app/main.jsx'));
  assert.deepEqual(
    [...appDomains].sort((a, b) => a.localeCompare(b)),
    [...sliceDomains].sort((a, b) => a.localeCompare(b)),
    'every subscribed domain must be registered in STATE_SLICE_FIELDS and vice versa',
  );
});

test('projects domain is fully wired (feature -> slice -> app)', () => {
  const feature = read('platform/tauri/bridge/projects.js');
  const sliceFields = parseSliceFields(read('platform/tauri/bridge.js'));
  const appDomains = parseAppDomains(read('app/main.jsx'));

  assert.ok(
    feature.includes('state.projectsList'),
    'projects feature must publish its snapshot on state.projectsList',
  );
  const featureWrites = [...feature.matchAll(/state\.(\w+)\s*=/g)].map((entry) => entry[1]);
  assert.deepEqual(
    [...new Set(featureWrites)].sort((a, b) => a.localeCompare(b)),
    ['projectsList'],
    'projects feature must only write the fields its domain declares',
  );
  assert.ok(
    sliceFields.projects.includes('projectsList'),
    'STATE_SLICE_FIELDS.projects must declare projectsList',
  );
  assert.ok(
    appDomains.includes('projects'),
    'APP_BRIDGE_STATE_DOMAINS must subscribe the projects domain',
  );
});

// Web 端的 fields 注册表与桌面订阅列表必须同步:否则 web 启动期
// getMany(APP_BRIDGE_STATE_DOMAINS) 抛 "Unknown Tauri bridge state slice",
// 整个 WebUI 冒烟超时(#448 的根因,栈内所有 PR 的 frontend-test 全红)。
function parseWebFields(source) {
  // fields 表以两空格缩进的 `};` 收尾;锚到四空格会越表捕获进 stablePick,
  // 那段将来一旦出现「对象字面量+数组」就会混入幻影条目(finding 30)。
  const match = source.match(/const fields = \{([\s\S]*?)\n {2}\};/);
  assert.ok(match, 'fields registry not found in web domain-adapter.js');
  const domains = {};
  for (const entry of match[1].matchAll(/(\w+): \[([^\]]*)\]/g)) {
    domains[entry[1]] = entry[2]
      .split(',')
      .map((field) => field.trim().replace(/^"|"$/g, ''))
      .filter(Boolean);
  }
  return domains;
}

test('web domain-adapter fields registry covers every subscribed domain', () => {
  const webFields = parseWebFields(read('platform/web/bridge/domain-adapter.js'));
  const appDomains = parseAppDomains(read('app/main.jsx'));
  const sliceFields = parseSliceFields(read('platform/tauri/bridge.js'));
  const webBridge = read('platform/web/bridge.js');
  const missing = appDomains.filter((domain) => !webFields[domain]);
  assert.deepEqual(
    missing,
    [],
    `web fields registry must cover every APP_BRIDGE_STATE_DOMAINS entry: ${missing.join(', ')}`,
  );
  // 桌面专属域(projects)在 Web 上挂空桩。桩字段必须与桌面切片逐键一致——
  // 少一个键,该字段读取就是静默 undefined(只能靠消费侧防御兜底,finding 23);
  // 字段集取自契约模块而非手维护清单,避免与 bridge_domain_contract 漂移
  // (finding 21)。multiAgent 这类无状态切片的桌面专属域不在此约束内。
  // 其它双端域允许字段差异(web 刻意省略桌面专属字段),播种由
  // web_bridge_domain_contract 另行覆盖,不重复。
  const stubbed = desktopOnlyBridgeDomains.filter((domain) => appDomains.includes(domain));
  assert.ok(stubbed.length > 0, 'expected at least one subscribed desktop-only domain to lock');
  for (const domain of stubbed) {
    const declared = webFields[domain] || [];
    assert.ok(
      declared.length > 0,
      `web stub for desktop-only domain "${domain}" must declare its fields (empty list vacuates the seed check)`,
    );
    assert.deepEqual(
      [...declared].sort((a, b) => a.localeCompare(b)),
      [...(sliceFields[domain] || [])].sort((a, b) => a.localeCompare(b)),
      `web stub fields for "${domain}" must mirror STATE_SLICE_FIELDS exactly`,
    );
    for (const field of declared) {
      // 行首锚定,避免子串匹配命中注释。
      assert.ok(
        new RegExp(`^\\s+${field}:`, 'm').test(webBridge),
        `web bridge state must seed field "${field}" for desktop-only domain "${domain}"`,
      );
    }
  }
  // projects 桩必须与桌面快照同形 { projects, assignments, loadedAt }
  // (tauri/bridge/projects.js),分组直接读 .projects/.assignments。
  const projectsSeed = webBridge.match(/projectsList:\s*\{([^}]*)\}/);
  assert.ok(projectsSeed, 'web bridge must seed projectsList with the desktop snapshot shape');
  for (const key of ['projects', 'assignments']) {
    assert.ok(
      new RegExp(`${key}:`).test(projectsSeed[1]),
      `projectsList stub must carry "${key}" like the desktop snapshot`,
    );
  }
});
