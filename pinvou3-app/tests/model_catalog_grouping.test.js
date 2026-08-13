#!/usr/bin/env node
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vm = require('vm');

const srcPath = path.join(__dirname, '..', 'src', 'features', 'settings', 'model-catalog.js');
let code = fs.readFileSync(srcPath, 'utf8');
// 剥离 ESM 关键字(与 composer_tool_menu_logic.test.js 同款)
code = code.replace(/\bexport\s+\{[^}]+\};?/g, '').replace(/\bexport\s+/g, '');
// 剥离 asset 导入(SVG/PNG)与副作用导入(Node 无法解析,函数体不依赖它们)
code = code.replace(/import\s+[^;]*from\s+['"][^'"]*\/brand-icons\/[^'"]+['"];?/g, '');
code = code.replace(/import\s+['"]\.\/settings-i18n\.js['"];?/g, '');
// 剥离模块级图标映射(BRAND_ICON_BY_PRESET/VENDOR):其 import 已剥离,但对象字面量仍在模块顶层
// 引用这些标识符,会在 vm 求值时抛 "deepseekIcon is not defined"。被测函数不依赖图标映射。
code = code.replace(/const\s+BRAND_ICON_BY_(?:PRESET|VENDOR)\s*=\s*\{[\s\S]*?\};?/g, '');

const ctx = { console };
vm.createContext(ctx);
vm.runInContext(
  `${code}\n` +
  `this.isPresetModel = isPresetModel;\n` +
  `this.groupModelsForSelector = groupModelsForSelector;\n` +
  `this.localUserNamed = localUserNamed;\n` +
  `this.selectorMainLabel = selectorMainLabel;\n` +
  `this.selectorSubLabel = selectorSubLabel;\n` +
  `this.MODEL_CATALOG = MODEL_CATALOG;\n` +
  `this.findCloudProviderForModel = findCloudProviderForModel;\n` +
  `this.providerLabelForModel = providerLabelForModel;\n` +
  `this.reasoningEffortTiersForModel = reasoningEffortTiersForModel;\n` +
  `this.defaultReasoningEffortForModel = defaultReasoningEffortForModel;\n` +
  `this.reasoningEffortForModelSwitch = reasoningEffortForModelSwitch;\n` +
  `this.normalizeStoredReasoningEffort = normalizeStoredReasoningEffort;\n`,
  ctx,
  { filename: srcPath },
);

const { isPresetModel, groupModelsForSelector, localUserNamed, selectorMainLabel, selectorSubLabel, providerLabelForModel, reasoningEffortTiersForModel, defaultReasoningEffortForModel, reasoningEffortForModelSwitch, normalizeStoredReasoningEffort } = ctx;

// i18n 测试替身:复刻实际字典里会用到的字段
const t = {
  modelPresetOpenaiCompatible: 'OpenAI 兼容',
  uiSettingsDetail: {
    localModelName: name => (name ? `本地 ${name}` : '本地模型'),
  },
};
const tEn = {
  modelPresetOpenaiCompatible: 'OpenAI Compatible',
  uiSettingsDetail: {
    localModelName: name => (name ? `Local ${name}` : 'Local model'),
  },
};
const localModelNameFn = t.uiSettingsDetail.localModelName;
// providerLabelForModel 内部读 t.uiSettingsDetail.providerCatalog,测试中无覆盖则回退 presetProviderLabel
// selectorSubLabel 的「目录命中」分支依赖 findCloudProviderForModel + providerLabelForModel;后者无覆盖时回退 provider.title/presetProviderLabel。

function mk(partial) { return Object.assign({ id: 'm1', name: '', preset: 'openai_compatible', model: '', base_url: '', provider_kind: null, vendor: null }, partial); }

let pass = 0, fail = 0;
function test(name, fn) { try { fn(); pass++; console.log('  ok - ' + name); } catch (e) { fail++; console.log('  FAIL - ' + name + '\n    ' + e.message); } }

// --- isPresetModel ---
test('OpenAI Compatible 未知 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'custom', model: 'meta-llama/llama-4-scout' })), false);
});
test('OpenAI Compatible 命中目录 ID 仍为自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'custom', base_url: 'https://openrouter.ai/api/v1', model: 'deepseek-v4-pro' })), false);
});
test('Coding Plan 命中目录(glm-5.2) -> 预设', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4', model: 'glm-5.2' })), true);
});
test('Coding Plan 手填 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4', model: 'my-custom-glm' })), false);
});
test('官方 API 命中目录(deepseek-v4-pro) -> 预设', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'deepseek-v4-pro' })), true);
});
test('官方 API 手填 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'deepseek-v9-fake' })), false);
});
test('官方 API 仅命中其他 provider 的目录 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'glm-5.2' })), false);
});
test('本地命中目录(qwen36_35b_256k) -> 预设', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'local_vllm', model: 'qwen36_35b_256k' })), true);
});
test('本地手填 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'local_vllm', model: 'ollama/phi4' })), false);
});

// --- groupModelsForSelector ---
test('分组保留原顺序', () => {
  const a = mk({ id: 'a', preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'deepseek-v4-pro' });
  const b = mk({ id: 'b', preset: 'openai_compatible', provider_kind: 'custom', model: 'x/y' });
  const c = mk({ id: 'c', preset: 'openai_compatible', provider_kind: 'custom', model: 'x/z' });
  const g = groupModelsForSelector([a, b, c]);
  // 用 join 比较:vm 沙箱内 .map() 返回的数组与外层 realm 数组原型不同,
  // deepStrictEqual 会以 "not reference-equal" 误判;join 为原始字符串后跨 realm 稳定。
  assert.strictEqual(g.preset.map(m => m.id).join('|'), 'a');
  assert.strictEqual(g.custom.map(m => m.id).join('|'), 'b|c');
});

// --- localUserNamed ---
test('本地默认名 -> 非用户命名', () => {
  assert.strictEqual(localUserNamed(mk({ preset: 'local_vllm', name: '本地 qwen36_35b_256k', model: 'qwen36_35b_256k' }), localModelNameFn), false);
});
test('本地改名 -> 用户命名', () => {
  assert.strictEqual(localUserNamed(mk({ preset: 'local_vllm', name: '我的模型', model: 'qwen36_35b_256k' }), localModelNameFn), true);
});
test('中文界面保存的本地默认名在英文界面仍非用户命名', () => {
  assert.strictEqual(localUserNamed(mk({ preset: 'local_vllm', name: '本地 qwen36_35b_256k', model: 'qwen36_35b_256k' }), tEn.uiSettingsDetail.localModelName), false);
});
test('非本地 -> 恒 false', () => {
  assert.strictEqual(localUserNamed(mk({ preset: 'deepseek', name: '任意', model: 'deepseek-v4-pro' }), localModelNameFn), false);
});

// --- selectorMainLabel ---
test('预设行主标签 = name(item.title)', () => {
  assert.strictEqual(selectorMainLabel(mk({ name: 'GLM-5.2', preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4', model: 'glm-5.2' }), t), 'GLM-5.2');
});
test('自定义行主标签 = 模型 ID', () => {
  assert.strictEqual(selectorMainLabel(mk({ name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', model: 'meta-llama/llama-4-scout' }), t), 'meta-llama/llama-4-scout');
});
test('本地已命名 -> 用 name', () => {
  assert.strictEqual(selectorMainLabel(mk({ name: '我的模型', preset: 'local_vllm', model: 'qwen36_35b_256k' }), t), '我的模型');
});
test('本地预设默认名随当前界面语言显示', () => {
  assert.strictEqual(selectorMainLabel(mk({ name: '本地 qwen36_35b_256k', preset: 'local_vllm', model: 'qwen36_35b_256k' }), tEn), 'Local qwen36_35b_256k');
});
test('本地自定义模型跨语言仍以模型 ID 为主标签', () => {
  assert.strictEqual(selectorMainLabel(mk({ name: '本地 ollama/phi4', preset: 'local_vllm', model: 'ollama/phi4' }), tEn), 'ollama/phi4');
});

// --- selectorSubLabel ---
test('预设行副标题 = provider 归属(非 model)', () => {
  // Finding #2: 预设行副标题改为 providerLabel,主=Title-Case title,副=provider,消除 model 重复。
  assert.strictEqual(selectorSubLabel(mk({ name: 'GLM-5.2', preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4', model: 'glm-5.2' }), t), '智谱 Coding Plan / GLM Coding Plan');
});
test('OpenAI Compatible 自定义行副标题 = modelPresetOpenaiCompatible', () => {
  assert.strictEqual(selectorSubLabel(mk({ name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', base_url: 'https://api.openrouter.ai/v1', model: 'meta-llama/llama-4-scout' }), t), 'OpenAI 兼容');
});
test('本地已命名副标题 = model', () => {
  assert.strictEqual(selectorSubLabel(mk({ name: '我的模型', preset: 'local_vllm', model: 'qwen36_35b_256k' }), t), 'qwen36_35b_256k');
});

// --- 回归:Finding #2 预设行 title===model 时主副不可重复 ---
test('预设 deepseek(title===model) 主副标签不重复', () => {
  // name 保存为 item.title,目录里 deepseek 的 title === model === 'deepseek-v4-pro'。
  // 修复前主副均为模型 id('deepseek-v4-pro'),显示重复;修复后副标题为 provider 归属。
  const presetModel = mk({ name: 'deepseek-v4-pro', preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'deepseek-v4-pro' });
  const main = selectorMainLabel(presetModel, t);
  const sub = selectorSubLabel(presetModel, t);
  assert.strictEqual(main, 'deepseek-v4-pro');
  // 副标题改为 provider 归属(providerLabelForModel),与主标签(模型 id)不同 -> 消除重复。
  assert.notStrictEqual(sub, main);
  assert.strictEqual(sub, providerLabelForModel(presetModel, t));
});

// --- 空值/边界 guard ---
test('selectorMainLabel(null) = ""', () => {
  assert.strictEqual(selectorMainLabel(null, t), '');
});
test('groupModelsForSelector([]) = {preset:[], custom:[]}', () => {
  const g = groupModelsForSelector([]);
  assert.strictEqual(g.preset.length, 0);
  assert.strictEqual(g.custom.length, 0);
});

// --- 回归:本次 bug 场景 ---
test('同 provider 多自定义模型主标签各不相同', () => {
  const m1 = mk({ id: 'm1', name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', model: 'meta-llama/llama-4-scout' });
  const m2 = mk({ id: 'm2', name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', model: 'openai/gpt-oss-120b' });
  assert.notStrictEqual(selectorMainLabel(m1, t), selectorMainLabel(m2, t));
});
test('同 provider 多个目录内自定义模型主标签仍各不相同', () => {
  const m1 = mk({ id: 'm1', name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', base_url: 'https://openrouter.ai/api/v1', model: 'deepseek-v4-pro' });
  const m2 = mk({ id: 'm2', name: 'OpenAI 兼容', preset: 'openai_compatible', provider_kind: 'custom', base_url: 'https://openrouter.ai/api/v1', model: 'glm-5.2' });
  assert.strictEqual(selectorMainLabel(m1, t), 'deepseek-v4-pro');
  assert.strictEqual(selectorMainLabel(m2, t), 'glm-5.2');
});

// ── 思考深度档位（reasoning effort）──
test('reasoningEffortTiersForModel 按 provider 暴露有实际区别的档位', () => {
  // vm 上下文数组与宿主 realm 不同，deepStrictEqual 会因原型不同误报，用 Array.from 归一
  const tiers = model => Array.from(reasoningEffortTiersForModel(model) || []);
  const deepseek = { preset: 'deepseek', vendor: 'deepseek', model: 'deepseek-v4-pro' };
  assert.deepStrictEqual(tiers(deepseek), ['off', 'low', 'high', 'max']);
  const moonshot = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.deepStrictEqual(tiers(moonshot), ['low', 'high', 'max']);
  const moonshotNonK3 = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k2.6' };
  assert.deepStrictEqual(tiers(moonshotNonK3), ['off', 'high']);
  const zai52 = { preset: 'glm', vendor: 'glm', model: 'GLM-5.2', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zai52), ['off', 'high', 'max']);
  const zaiTurbo = { preset: 'glm', vendor: 'glm', model: 'glm-5-turbo', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zaiTurbo), ['off', 'high']);
  const zai51 = { preset: 'glm', vendor: 'glm', model: 'glm-5.1', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zai51), ['off', 'high']);
  // GLM-5.3 继承 GLM-5.2 的 reasoning_options，同为 tiered effort（底座 is_exact_zai_tiered_effort_route）
  const zai53 = { preset: 'glm', vendor: 'glm', model: 'glm-5.3', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zai53), ['off', 'high', 'max']);
  const kimiCodeK3 = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://api.kimi.com/coding/v1' };
  assert.deepStrictEqual(tiers(kimiCodeK3), ['low', 'high', 'max']);
  const vllm = { preset: 'local_vllm', model: 'qwen36_35b_256k' };
  assert.deepStrictEqual(tiers(vllm), ['off', 'low', 'medium', 'high']);
  const anthropic = { preset: 'anthropic', vendor: 'anthropic', model: 'claude-sonnet-5' };
  assert.deepStrictEqual(tiers(anthropic), ['low', 'medium', 'high', 'max']);
  const openai56 = { preset: 'openai', vendor: 'openai', model: 'gpt-5.6-terra' };
  assert.deepStrictEqual(tiers(openai56), ['off', 'low', 'medium', 'high', 'max']);
  // 品悟目录收录的 reasoning 家族模型（gpt-5.5 / gpt-5.6-sol/terra/luna）提供切换
  const openai55 = { preset: 'openai', vendor: 'openai', model: 'gpt-5.5' };
  assert.deepStrictEqual(tiers(openai55), ['off', 'low', 'medium', 'high', 'max']);
  const openai56Sol = { preset: 'openai', vendor: 'openai', model: 'gpt-5.6-sol' };
  assert.deepStrictEqual(tiers(openai56Sol), ['off', 'low', 'medium', 'high', 'max']);
  // OpenAI 非 reasoning 系（gpt-5.4-mini）与 xai/qwen/gemini/自定义兼容不提供切换
  const openaiMini = { preset: 'openai', vendor: 'openai', model: 'gpt-5.4-mini' };
  assert.strictEqual(reasoningEffortTiersForModel(openaiMini), null);
  const xai = { preset: 'xai', vendor: 'xai', model: 'grok-4.3' };
  assert.strictEqual(reasoningEffortTiersForModel(xai), null);
  const qwen = { preset: 'qwen', vendor: 'qwen', model: 'qwen3.8-max' };
  assert.strictEqual(reasoningEffortTiersForModel(qwen), null);
  const gemini = { preset: 'gemini', vendor: 'gemini', model: 'gemini-3.6-flash' };
  assert.strictEqual(reasoningEffortTiersForModel(gemini), null);
  const custom = { preset: 'openai_compatible', model: 'my-model' };
  assert.strictEqual(reasoningEffortTiersForModel(custom), null);
  // tiered effort 只认精确 first-party 端点：中国端点 / 兼容网关同型号回落通用档位（fail-closed）
  const moonshotCn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.deepStrictEqual(tiers(moonshotCn), ['off', 'high']);
  const k3OnDirectPlatform = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.deepStrictEqual(tiers(k3OnDirectPlatform), ['off', 'high']);
  const k3OnGateway = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://gateway.example.com/v1' };
  assert.deepStrictEqual(tiers(k3OnGateway), ['off', 'high']);
  const zaiCodingPlanGlobal = { preset: 'openai_compatible', vendor: 'glm', model: 'glm-5.2', base_url: 'https://api.z.ai/api/coding/paas/v4' };
  assert.deepStrictEqual(tiers(zaiCodingPlanGlobal), ['off', 'high', 'max']);
  // zai：中国端点 / 兼容网关 / 未验证模型底座删除 thinking/reasoning_effort，off 与 high 等效 → 不提供切换
  const zaiCn = { preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' };
  assert.strictEqual(reasoningEffortTiersForModel(zaiCn), null);
  const zaiGateway = { preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://gateway.example.com/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(zaiGateway), null);
  const zaiUnknownModel = { preset: 'glm', vendor: 'glm', model: 'glm-4.7', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.strictEqual(reasoningEffortTiersForModel(zaiUnknownModel), null);
  // minimax：仅 first-party MiniMax-M3 提供 off/high，M2.7/M2.5 与兼容网关不提供切换
  const minimaxM3 = { preset: 'minimax', vendor: 'minimax', model: 'MiniMax-M3', base_url: 'https://api.minimax.io/v1' };
  assert.deepStrictEqual(tiers(minimaxM3), ['off', 'high']);
  const minimaxM3Cn = { preset: 'minimax', vendor: 'minimax', model: 'MiniMax-M3', base_url: 'https://api.minimaxi.com/v1' };
  assert.deepStrictEqual(tiers(minimaxM3Cn), ['off', 'high']);
  const minimaxM27 = { preset: 'minimax', vendor: 'minimax', model: 'MiniMax-M2.7', base_url: 'https://api.minimax.io/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(minimaxM27), null);
  const minimaxGateway = { preset: 'minimax', vendor: 'minimax', model: 'MiniMax-M3', base_url: 'https://gateway.example.com/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(minimaxGateway), null);
  // 官方 deepseek base_url 推断：openai_compatible 且无 vendor，但 base_url 指向官方端点 → deepseek 档位
  const deepseekByUrl = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseek.com/v1' };
  assert.deepStrictEqual(tiers(deepseekByUrl), ['off', 'low', 'high', 'max']);
  // /beta 与 api.deepseeki.com 同为官方端点（对齐 Rust is_official_deepseek_base_url）
  const deepseekBeta = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseek.com/beta' };
  assert.deepStrictEqual(tiers(deepseekBeta), ['off', 'low', 'high', 'max']);
  const deepseeki = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseeki.com' };
  assert.deepStrictEqual(tiers(deepseeki), ['off', 'low', 'high', 'max']);
  // volcengine：底座把 low/medium 归一为 high，仅 off/high/max 有区别
  const volcengine = { preset: 'doubao', vendor: 'doubao', model: 'doubao-seed-evolving' };
  assert.deepStrictEqual(tiers(volcengine), ['off', 'high', 'max']);
  // xiaomi-mimo：只有 thinking 开关（off/enabled），off/high 两档
  const mimo = { preset: 'mimo', vendor: 'mimo', model: 'mimo-v2.5-pro' };
  assert.deepStrictEqual(tiers(mimo), ['off', 'high']);
});

test('OpenAI reasoning 家族判定对齐底座 model_is_openai_reasoning_family（含手输自定义模型）', () => {
  const tiers = model => Array.from(reasoningEffortTiersForModel(model) || []);
  const openai = model => ({ preset: 'openai', vendor: 'openai', model });
  // 官方 OpenAI 支持手输自定义模型 ID：这些模型底座会注入多档 reasoning_effort，
  // 前端必须提供切换，不能因「不在目录内」返回 null（否则后端注入、前端不可控）。
  const reasoningFamily = [
    'gpt-5.6', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna',
    'gpt-5.5', 'gpt-5.5-pro',
    'gpt-5.5-2026-01-01', 'gpt-5.5-pro-2026-01-01',
    'gpt-5-codex', 'gpt-5.1-codex', 'gpt-5.1-codex-mini', 'gpt-5.1-codex-max',
    'gpt-5.2-codex', 'gpt-5.3-codex', 'codex-gpt-5.5', 'chatgpt-gpt-5.5',
    'gpt-5.5-codex', 'gpt-5.5-codex-preview', 'codex-gpt-5.5-preview', 'chatgpt-gpt-5.5-preview',
  ];
  reasoningFamily.forEach(id => {
    assert.deepStrictEqual(tiers(openai(id)), ['off', 'low', 'medium', 'high', 'max'], `reasoning 家族正例应提供切换: ${id}`);
  });
  // 非 reasoning 家族（含名称近似但底座 predicate 不命中的）不提供切换
  const nonReasoning = [
    'gpt-5.4-mini', 'gpt-4o', 'gpt-4.1', 'o3', 'o4-mini',
    'gpt-5.5-2026-1-1', 'gpt-5.5-pro-20260101', 'gpt-5.5-codex-preview-extra',
  ];
  nonReasoning.forEach(id => {
    assert.strictEqual(reasoningEffortTiersForModel(openai(id)), null, `非 reasoning 模型应为 null: ${id}`);
  });
});

test('reasoningEffortTiersForModel：精确路由语义对齐底座 is_exact_https_route', () => {
  const tiers = model => Array.from(reasoningEffortTiersForModel(model) || []);
  const mkZai = baseUrl => ({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: baseUrl });
  // 一个尾斜杠无意义（底座 strip_suffix('/')），仍为精确端点
  assert.deepStrictEqual(tiers(mkZai('https://api.z.ai/api/paas/v4/')), ['off', 'high', 'max']);
  // 两个尾斜杠：底座只删一个，path 变成 api/paas/v4/，与官方 path 不等 → fail-closed
  assert.strictEqual(reasoningEffortTiersForModel(mkZai('https://api.z.ai/api/paas/v4//')), null);
  // path 大小写敏感：API/paas/v4 是相邻路由，不是官方端点
  assert.strictEqual(reasoningEffortTiersForModel(mkZai('https://api.z.ai/API/paas/v4')), null);
  // host/scheme 大小写不敏感
  assert.deepStrictEqual(tiers(mkZai('https://API.Z.AI/api/paas/v4')), ['off', 'high', 'max']);
  assert.deepStrictEqual(tiers(mkZai('HTTPS://api.z.ai/api/paas/v4')), ['off', 'high', 'max']);
});

test('reasoningEffortForModelSwitch：K2.6(off) → K3 重置为 high', () => {
  const k26 = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k2.6' };
  const k3 = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.ai/v1' };
  // K2.6 上用户可存 off；切到 K3 后 off 不在其档位表（low/high/max）内，必须重置为 high
  assert.deepStrictEqual(Array.from(reasoningEffortTiersForModel(k26)), ['off', 'high']);
  assert.ok(!Array.from(reasoningEffortTiersForModel(k3)).includes('off'));
  assert.strictEqual(reasoningEffortForModelSwitch(k3), 'high');
  // 无档位模型切换置 null（未显式设置）；vllm 切回 off
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'xai', vendor: 'xai', model: 'grok-4.3' }), null);
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'local_vllm', model: 'qwen36_35b_256k' }), 'off');
  // z.ai glm-5.2 切换默认 high；中国端点 glm-5.2 无档位 → null
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://api.z.ai/api/paas/v4' }), 'high');
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' }), null);
});

test('defaultReasoningEffortForModel：vllm→off，其余支持档位的模型→high，不支持→null', () => {
  const deepseek = { preset: 'deepseek', vendor: 'deepseek', model: 'deepseek-v4-pro' };
  assert.strictEqual(defaultReasoningEffortForModel(deepseek), 'high');
  const vllm = { preset: 'local_vllm', model: 'qwen36_35b_256k' };
  assert.strictEqual(defaultReasoningEffortForModel(vllm), 'off');
  const xai = { preset: 'xai', vendor: 'xai', model: 'grok-4.3' };
  assert.strictEqual(defaultReasoningEffortForModel(xai), null);
});

test('normalizeStoredReasoningEffort：存量旧值归一，无档位模型为 null', () => {
  const deepseek = { preset: 'deepseek', vendor: 'deepseek', model: 'deepseek-v4-pro' };
  // medium 不在 deepseek 档位表内（底座把 medium 归一为 high）→ 归一到 high；
  // low 是底座保留的真实档位，应在档位表内原样保留。
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'medium'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'low'), 'low');
  // 底座别名 → 规范档位后再与档位表匹配
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'light'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'minimum'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'ultra'), 'max');
  // 存量值已在档位表内 → 原样保留
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'off'), 'off');
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, 'max'), 'max');
  // 无存量 → 回退默认档位
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, null), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek, undefined), 'high');
  // vllm 默认 off，存量为空时同样回退 off
  const vllm = { preset: 'local_vllm', model: 'qwen36_35b_256k' };
  assert.strictEqual(normalizeStoredReasoningEffort(vllm, null), 'off');
  // 无档位模型（xai 底座空操作）→ null
  const xai = { preset: 'xai', vendor: 'xai', model: 'grok-4.3' };
  assert.strictEqual(normalizeStoredReasoningEffort(xai, 'high'), null);
  assert.strictEqual(normalizeStoredReasoningEffort(xai, null), null);
  // anthropic 档位表含 low/medium/high/max：存量 medium 原样保留
  const anthropic = { preset: 'anthropic', vendor: 'anthropic', model: 'claude-sonnet-5' };
  assert.strictEqual(normalizeStoredReasoningEffort(anthropic, 'medium'), 'medium');
  // always-thinking K3（国际直连平台）：off 在底座 K3 路由里等价于 low，medium 等价于 high
  const k3Direct = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'off'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'none'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'medium'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'low'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'high'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Direct, 'max'), 'max');
  // 中国端点 kimi-k3 走通用 moonshot 档位（off/high），off 保持 off
  const k3Cn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(k3Cn, 'off'), 'off');
});

test('手输改字段（model ID / base_url）归一只修正失效值、保留有效值', () => {
  // Kimi 非 K3 上已存 off：改自定义 ID（仍非 K3）后 off 依然合法，保留而非重置为 high
  const moonshotCustom = { preset: 'kimi', vendor: 'kimi', model: 'custom-kimi-a' };
  assert.strictEqual(normalizeStoredReasoningEffort(moonshotCustom, 'off'), 'off');
  // 改 ID 为 kimi-k3（always-thinking）后 off 失效，按底座真实等价值归一为 low
  const k3 = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(k3, 'off'), 'low');
  // vLLM 上已存 high：改本地模型 ID / base_url 后 high 依然合法，保留（不误清用户选择）
  const vllmHigh = { preset: 'local_vllm', model: 'qwen36_35b_256k' };
  assert.strictEqual(normalizeStoredReasoningEffort(vllmHigh, 'high'), 'high');
  // openai_compatible 改 base_url 到官方 deepseek 端点：档位从无到有，存量 null 回落默认 high
  const deepseekByUrl = { preset: 'openai_compatible', model: 'my-model', base_url: 'https://api.deepseek.com' };
  assert.strictEqual(normalizeStoredReasoningEffort(deepseekByUrl, null), 'high');
});

console.log(`\nmodel_catalog_grouping: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
