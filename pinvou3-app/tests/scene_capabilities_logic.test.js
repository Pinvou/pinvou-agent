#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const companionPath = path.join(__dirname, '..', 'src', 'shared', 'companion-packages.js');
const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'scene-capabilities.js');
// scene-capabilities 经 import 引用 shared/companion-packages：vm script 语义
// 下剥掉 import/export 声明，共享模块先入上下文，两个源共用同一作用域。
const stripModuleSyntax = (source) => source
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '')
  .replace(/^\s*import\s[^\n]*\n/gm, '');
const code = [fs.readFileSync(companionPath, 'utf8'), fs.readFileSync(logicPath, 'utf8')]
  .map(stripModuleSyntax)
  .join('\n');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.canPrepareSceneCapabilities = canPrepareSceneCapabilities;
this.requiredCapabilitiesForMeta = requiredCapabilitiesForMeta;
this.prepareSceneCapabilities = prepareSceneCapabilities;`, ctx, { filename: logicPath });

const { canPrepareSceneCapabilities, requiredCapabilitiesForMeta, prepareSceneCapabilities } = ctx;

assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: true, dependencyInstallAvailable: true }),
  false,
  'Web host must not call desktop marketplace install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: false }),
  false,
  'Desktop host without dependency install capability must not call install APIs',
);
assert.strictEqual(
  canPrepareSceneCapabilities({ isWebHost: false, dependencyInstallAvailable: true }),
  true,
  'Desktop host with dependency install capability may prepare scene capabilities',
);

const dataVisualizationRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'design:data-visualization' });
assert.strictEqual(dataVisualizationRequirements.key, 'dataVisualization');
assert.deepStrictEqual([...dataVisualizationRequirements.tools], []);
assert.deepStrictEqual([...dataVisualizationRequirements.skills], ['visualizer']);

const documentWritingRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'work:document-writing' });
assert.strictEqual(documentWritingRequirements.key, 'documentWriting');
assert.deepStrictEqual([...documentWritingRequirements.tools], ['gongwen']);
assert.deepStrictEqual([...documentWritingRequirements.skills], ['government-writing']);

const pptDesignRequirements = requiredCapabilitiesForMeta({ pinvouScene: 'design:ppt' });
assert.strictEqual(pptDesignRequirements.key, 'pptDesign');
assert.deepStrictEqual([...pptDesignRequirements.tools], ['pptx']);
assert.deepStrictEqual([...pptDesignRequirements.skills], ['pptx']);

assert.strictEqual(requiredCapabilitiesForMeta(null), null);
assert.strictEqual(requiredCapabilitiesForMeta({ pinvouScene: 'design:poster' }), null);
// 用户可见文案由 UI 层从 t.uiChatScenes[requirements.key] 取值，模块不得再携带文案字段。
assert.strictEqual('label' in dataVisualizationRequirements, false);
assert.strictEqual('preparingText' in dataVisualizationRequirements, false);

// ---------------------------------------------------------------------------
// prepareSceneCapabilities 的可用性闭环：装上 ≠ 会话可见。开关禁用集与可见性
// 隐藏集任一命中场景要求包（PPT 实测回归：pptx 残留在 plain 隐藏集，安装成功
// 但 load_skill / mcp_pptx_make_pptx 均不可用），必须在发送前显式开启。
// ---------------------------------------------------------------------------
(async () => {

function createAvailabilityHarness({ tools = [], skills = [], disabled = [], hidden = [] } = {}) {
  const calls = [];
  const state = {
    tools: tools.map((t) => ({ ...t })),
    skills: skills.map((s) => ({ ...s })),
    disabled: [...disabled],
    hidden: [...hidden],
  };
  const invoke = async (command, args = {}) => {
    calls.push([command, args]);
    switch (command) {
      case 'list_marketplace_tools':
        return state.tools;
      case 'list_marketplace_skills':
        return state.skills;
      case 'install_marketplace_tool':
      case 'install_marketplace_skill': {
        // 与后端一致：安装把对应条目置为已安装。
        const list = command === 'install_marketplace_tool' ? state.tools : state.skills;
        const id = args.toolId || args.skillId;
        const entry = list.find((item) => item.id === id);
        if (entry) entry.installed = true;
        else list.push({ id, installed: true, companion_skills: [] });
        return;
      }
      case 'get_disabled_connectors':
        return state.disabled;
      case 'get_bundle_visibility':
        return state.hidden;
      case 'set_disabled_connectors':
        state.disabled = [...args.connectorIds];
        return;
      case 'set_bundle_visibility':
        state.hidden = [...args.bundleIds];
        return;
      default:
        throw new Error(`unexpected command: ${command}`);
    }
  };
  return { invoke, calls, state };
}

const pptMeta = { pinvouScene: 'design:ppt' };

// ① 已安装但残留在 plain 隐藏集 → 发送前自动移出隐藏集，其余条目原样保留。
{
  const harness = createAvailabilityHarness({
    tools: [{ id: 'pptx', installed: true, companion_skills: [] }],
    skills: [{ id: 'pptx', installed: true }],
    hidden: ['weather', 'pptx'],
    disabled: ['weather'],
  });
  const prepared = await prepareSceneCapabilities(pptMeta, harness.invoke);
  assert.strictEqual(prepared.ok, true, '隐藏集残留必须被就地开启而不是让强制场景落空');
  assert.strictEqual(prepared.reEnabled, true, '自动开启是对治理状态的变更，必须告知调用方');
  assert.deepStrictEqual(harness.state.hidden, ['weather'], '只移除场景点名的包，其余可见性保留');
  assert.deepStrictEqual(harness.state.disabled, ['weather'], '开关集未被误写');
  const writes = harness.calls.filter(([cmd]) => cmd.startsWith('set_'));
  assert.deepStrictEqual(writes.map(([cmd]) => cmd), ['set_bundle_visibility']);
}

// ② 已安装但被开关禁用 → 发送前自动开启，其余禁用项保留。
{
  const harness = createAvailabilityHarness({
    tools: [{ id: 'pptx', installed: true, companion_skills: [] }],
    skills: [{ id: 'pptx', installed: true }],
    disabled: ['weather', 'pptx'],
  });
  const prepared = await prepareSceneCapabilities(pptMeta, harness.invoke);
  assert.strictEqual(prepared.ok, true);
  assert.strictEqual(prepared.reEnabled, true, '自动开启是对治理状态的变更，必须告知调用方');
  assert.deepStrictEqual(harness.state.disabled, ['weather']);
  assert.deepStrictEqual(harness.state.hidden, []);
  const writes = harness.calls.filter(([cmd]) => cmd.startsWith('set_'));
  assert.deepStrictEqual(writes.map(([cmd]) => cmd), ['set_disabled_connectors']);
}

// ③ 已安装且可见可用 → 零写操作（不重写用户的整集配置）。
{
  const harness = createAvailabilityHarness({
    tools: [{ id: 'pptx', installed: true, companion_skills: [] }],
    skills: [{ id: 'pptx', installed: true }],
    disabled: ['weather'],
    hidden: ['weather'],
  });
  const prepared = await prepareSceneCapabilities(pptMeta, harness.invoke);
  assert.strictEqual(prepared.ok, true);
  assert.strictEqual(prepared.reEnabled, false, '未触碰治理状态时不得标记 reEnabled');
  assert.strictEqual(
    harness.calls.filter(([cmd]) => cmd.startsWith('set_')).length,
    0,
    '能力可用时不得触发任何开关/可见性写盘',
  );
}

// ④ companion 技能按所属包 id 比对（gongwen ↔ government-writing）：场景点名
//    技能 id，禁用集里是包 id，同样要被识别并开启。
{
  const harness = createAvailabilityHarness({
    tools: [{ id: 'gongwen', installed: true, companion_skills: ['government-writing'] }],
    skills: [{ id: 'government-writing', installed: true }],
    disabled: ['gongwen'],
  });
  const prepared = await prepareSceneCapabilities(
    { pinvouScene: 'work:document-writing' },
    harness.invoke,
  );
  assert.strictEqual(prepared.ok, true);
  assert.strictEqual(prepared.reEnabled, true);
  assert.deepStrictEqual(harness.state.disabled, [], 'companion 技能经包 id 命中禁用集并开启');
}

// ⑤ 未安装路径：安装后照样执行可用性检查（安装不会自动清隐藏集）。
{
  const harness = createAvailabilityHarness({
    skills: [],
    tools: [],
    hidden: ['pptx'],
  });
  const prepared = await prepareSceneCapabilities(pptMeta, harness.invoke);
  assert.strictEqual(prepared.ok, true);
  assert.strictEqual(prepared.installed, true);
  assert.strictEqual(prepared.reEnabled, true, '安装后残留的隐藏集仍要清掉且必须提示');
  assert.deepStrictEqual(harness.state.hidden, [], '安装后残留的隐藏集仍要清掉');
}
// eslint-disable-next-line unicorn/prefer-top-level-await -- smoke script keeps its existing async main() structure
})().catch((error) => {
  console.error(error);
  process.exit(1);
});

console.log('scene_capabilities_logic: ok');
