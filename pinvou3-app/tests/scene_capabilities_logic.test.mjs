#!/usr/bin/env node
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const logicPath = path.join(__dirname, '..', 'src', 'features', 'chat', 'scene-capabilities.js');
const code = fs.readFileSync(logicPath, 'utf8')
  .replace(/\bexport\s+\{[^}]+\};?/g, '')
  .replace(/\bexport\s+/g, '');

const ctx = {};
vm.createContext(ctx);
vm.runInContext(`${code}
this.canPrepareSceneCapabilities = canPrepareSceneCapabilities;
this.requiredCapabilitiesForMeta = requiredCapabilitiesForMeta;`, ctx, { filename: logicPath });

const { canPrepareSceneCapabilities, requiredCapabilitiesForMeta } = ctx;

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

console.log('scene_capabilities_logic: ok');

// --- DenyAll 适配（评审 #455 R5-B3）：安装 ≠ 可用，场景动作必须完成显式 opt-in ---
vm.runInContext('this.prepareSceneCapabilities = prepareSceneCapabilities;', ctx, { filename: logicPath });
const { prepareSceneCapabilities } = ctx;

function makeInvoke({ tools = [], skills = [], disabled = [] } = {}) {
  const state = {
    tools: new Set(tools),
    skills: new Set(skills),
    disabled: new Set(disabled),
    setCalls: [],
  };
  const toolList = () => [...state.tools].map((id) => ({ id, installed: true }));
  const skillList = () => [...state.skills].map((id) => ({ id, installed: true }));
  const invoke = async (command, args) => {
    if (command === 'list_marketplace_tools') return toolList();
    if (command === 'list_marketplace_skills') return skillList();
    if (command === 'install_marketplace_tool') { state.tools.add(args.toolId); return null; }
    if (command === 'install_marketplace_skill') { state.skills.add(args.skillId); return null; }
    if (command === 'get_disabled_connectors') return [...state.disabled];
    if (command === 'set_disabled_connectors') {
      state.setCalls.push([...args.connectorIds]);
      state.disabled = new Set(args.connectorIds);
      return null;
    }
    throw new Error(`unexpected command ${command}`);
  };
  return { invoke, state };
}

async function runDenyAllOptInScenarios() {
  // 全装全关：场景包坐在 DenyAll 扩集里（有效禁用集含包 id），场景动作
  // 必须把它移出禁用集——否则模型收不到工具、ready 文案在说谎。
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['gongwen', 'government-writing', 'feishu'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, false, 'nothing was installed');
    assert.strictEqual(prepared.enabled, true, 'scene packages must be opted in');
    assert.strictEqual(state.disabled.has('gongwen'), false);
    assert.strictEqual(state.disabled.has('government-writing'), false);
    assert.strictEqual(state.disabled.has('feishu'), true, 'unrelated packs stay disabled');
    assert.strictEqual(state.setCalls.length > 0, true, 'opt-in must persist via set_disabled_connectors');
  }

  // 未安装 + 未初始化（DenyAll 默认关）：安装后仍需显式 opt-in。
  {
    const { invoke, state } = makeInvoke({ disabled: ['pptx'] });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'design:ppt' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, true);
    assert.strictEqual(prepared.enabled, true);
    assert.strictEqual(state.disabled.has('pptx'), false);
  }

  // 已装且不在禁用集：零开关写，enabled=false（UI 不再弹 ready）。
  {
    const { invoke, state } = makeInvoke({
      tools: ['gongwen'],
      skills: ['government-writing'],
      disabled: ['feishu'],
    });
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, invoke);
    assert.strictEqual(prepared.ok, true);
    assert.strictEqual(prepared.installed, false);
    assert.strictEqual(prepared.enabled, false);
    assert.strictEqual(state.setCalls.length, 0, 'no switch write when already enabled');
  }

  // 开关读取失败：fail 必须可见（ok=false + error），不得按「已就绪」放行。
  {
    const { invoke } = makeInvoke({ tools: ['gongwen'], skills: ['government-writing'] });
    const failing = async (command, args) => {
      if (command === 'get_disabled_connectors') throw new Error('scope file unreadable');
      return invoke(command, args);
    };
    const prepared = await prepareSceneCapabilities({ pinvouScene: 'work:document-writing' }, failing);
    assert.strictEqual(prepared.ok, false, 'gate-read failure must not pass as ready');
    assert.strictEqual(prepared.enableFailed, true);
    assert.match(prepared.error, /scope file unreadable/);
  }

  console.log('scene_capabilities_logic deny-all opt-in: ok');
}

try {
  await runDenyAllOptInScenarios();
} catch (error) {
  console.error(error);
  process.exit(1);
}
