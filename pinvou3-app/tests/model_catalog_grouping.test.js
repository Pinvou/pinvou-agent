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

const ctx = { console, URL };
vm.createContext(ctx);
vm.runInContext(
  `${code}\n` +
  `this.isPresetModel = isPresetModel;\n` +
  `this.catalogItemMatchesModel = catalogItemMatchesModel;\n` +
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
  `this.normalizeStoredReasoningEffort = normalizeStoredReasoningEffort;\n` +
  `this.baseUrlUsesLoopback = baseUrlUsesLoopback;\n` +
  `this.baseUrlUsesLocalOrPrivate = baseUrlUsesLocalOrPrivate;\n` +
  `this.localProbeTiersForKind = localProbeTiersForKind;\n` +
  `this.alwaysThinkingSpecForModel = alwaysThinkingSpecForModel;\n` +
  `this.localReasoningTiers = localReasoningTiers;\n` +
  `this.reasoningEffortDisplayForTiers = reasoningEffortDisplayForTiers;\n` +
  `this.catalogImageCapableForModel = catalogImageCapableForModel;\n`,
  ctx,
  { filename: srcPath },
);

const { isPresetModel, catalogItemMatchesModel, MODEL_CATALOG, groupModelsForSelector, localUserNamed, selectorMainLabel, selectorSubLabel, providerLabelForModel, reasoningEffortTiersForModel, defaultReasoningEffortForModel, reasoningEffortForModelSwitch, normalizeStoredReasoningEffort, baseUrlUsesLoopback, baseUrlUsesLocalOrPrivate, localProbeTiersForKind, alwaysThinkingSpecForModel, localReasoningTiers, catalogImageCapableForModel, reasoningEffortDisplayForTiers } = ctx;

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
test('z.ai coding 目录行改为小写规范拼写,存量 GLM-5.2 大写配置经 legacyAliases 仍归预设', () => {
  // 2026-09 目录行改为 z.ai 官方小写 wire id（docs.z.ai API enum）；存量配置可能
  // 保存旧大写目录值（GLM-5.2），以 legacyAliases 兼容识别。GLM-5-Turbo 从未在
  // z.ai 发布，旧行已删，存量配置回落为自定义归类。
  const mkZai = model => mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://api.z.ai/api/coding/paas/v4', model });
  assert.strictEqual(isPresetModel(mkZai('glm-5.2')), true, '小写现行拼写精确命中');
  assert.strictEqual(isPresetModel(mkZai('GLM-5.2')), true, '旧大写存量配置经 legacyAliases 命中');
  assert.strictEqual(isPresetModel(mkZai('glm-5.3')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-5.3-flash')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-4.7')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-5-turbo')), false, 'GLM-5-Turbo 行已删,回落自定义归类');
});
test('Tencent Coding Plan catalog hits with legacy alias compatibility (official 2026-09-11 model table)', () => {
  // Both model rows hit; glm-5-0 is the official parallel second spelling on
  // the same page, registered in legacyAliases, so stored configs count as
  // preset with either spelling. kimi-k2.5 was retired platform-wide on
  // 2026-08-31 (announce 2414) and removed from the catalog.
  const base = 'https://api.lkeap.cloud.tencent.com/coding/v3';
  const mkPlan = model => mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'tencent', base_url: base, model });
  for (const model of ['tc-code-latest', 'glm-5', 'glm-5-0']) {
    assert.strictEqual(isPresetModel(mkPlan(model)), true, model);
  }
  assert.strictEqual(isPresetModel(mkPlan('kimi-k2.5')), false, 'kimi-k2.5 已全平台下线并从目录删除');
  assert.strictEqual(isPresetModel(mkPlan('kimi-k-2-5')), false, 'kimi-k-2-5 平行拼写同样不再收录');
  assert.strictEqual(isPresetModel(mkPlan('glm-5.2')), false, 'glm-5.2 is not in the official Coding Plan model table');
});
test('Tencent Token Plan is a separate catalog: distinguished from Coding Plan by base_url, no cross-group match', () => {
  // /plan/v3 general-tier rows hit their own group (official 2026-09-11 model
  // table, every row and every registered parallel spelling); on Coding Plan's
  // /coding/v3 the URL does not match, and with identical vendor+provider_kind
  // the exact comparison keeps the model custom. kimi-k2.5 was removed
  // platform-wide (announce 2414) and is no longer a catalog row in either group.
  const planBase = 'https://api.lkeap.cloud.tencent.com/plan/v3';
  const mkTokenPlan = model => mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'tencent', base_url: planBase, model });
  const planModels = [
    'tc-code-latest',
    'glm-5.3', 'glm-5-3', 'glm-5.3-flash', 'glm-5.2', 'glm-5-2', 'glm-5.1', 'glm-5-1', 'glm-5', 'glm-5-0',
    'kimi-k3', 'kimi-k2.7-code',
    'deepseek-v4-pro-202606', 'deepseek/deepseek-v4-pro-0813', 'deepseek/deepseek-v4-pro',
    'deepseek-v4-flash-202605', 'deepseek/deepseek-v4-flash-0731', 'deepseek/deepseek-v4-flash',
    'minimax-m3', 'minimax-m-3-0', 'minimax-m2.7', 'minimax-m-2-7',
    'hy3', 'hy3-preview', 'hy3-202608', 'hy4-preview',
  ];
  for (const model of planModels) {
    assert.strictEqual(isPresetModel(mkTokenPlan(model)), true, model);
  }
  assert.strictEqual(isPresetModel(mkTokenPlan('kimi-k2.5')), false, 'kimi-k2.5 已下线,任何腾讯云组都不再收录');
  assert.strictEqual(isPresetModel(mkTokenPlan('kimi-k-2-5')), false, 'kimi-k-2-5 平行拼写同样不再收录');
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'tencent', base_url: 'https://api.lkeap.cloud.tencent.com/coding/v3', model: 'glm-5.2' })), false, 'Token Plan-exclusive models must not match the Coding Plan catalog');
  assert.strictEqual(providerLabelForModel(mkTokenPlan('glm-5.2'), t), '腾讯云 Token Plan / Tencent Cloud Token Plan');
});
test('catalogItemMatchesModel 精确比较+legacyAliases 兼容迁移拼写', () => {
  // SettingsView 编辑弹窗的 initialCatalogMatch/known/active 均复用此比较:
  // z.ai 直连目录行已改为官方小写拼写 glm-5.2,存量大写 GLM-5.2 必须经
  // legacyAliases 命中,不得误判为自定义;除显式登记的历史拼写外一律精确比较。
  const zaiGroup = MODEL_CATALOG.cloud.find(group => group.key === 'glm_coding_plan_global');
  const glm52 = zaiGroup.items.find(item => item.model === 'glm-5.2');
  assert.ok(glm52, 'z.ai 直连目录应含小写 glm-5.2 规范拼写行');
  assert.deepStrictEqual([...(glm52.legacyAliases || [])], ['GLM-5.2'], '旧大写目录值须登记为 legacyAlias');
  assert.strictEqual(catalogItemMatchesModel(glm52, 'glm-5.2'), true);
  assert.strictEqual(catalogItemMatchesModel(glm52, 'GLM-5.2'), true);
  assert.strictEqual(catalogItemMatchesModel(glm52, 'Glm-5.2'), false);
  assert.strictEqual(catalogItemMatchesModel(glm52, 'glm-5.3'), false);
  assert.strictEqual(catalogItemMatchesModel(glm52), false);
});
test('本地 case-only 模型 ID 仍为自定义(vLLM ID 大小写敏感)', () => {
  // 本地 OpenAI-compatible 服务的模型 ID 是不透明字符串、可能区分大小写:
  // 与目录默认项 qwen36_35b_256k 仅大小写不同的 ID 是另一个模型,必须保持自定义。
  assert.strictEqual(isPresetModel(mk({ preset: 'local_vllm', model: 'QWEN36_35B_256K' })), false);
  assert.strictEqual(isPresetModel(mk({ preset: 'local_vllm', model: 'qwen36_35b_256k' })), true);
});
test('云端 case-only 模型 ID 仍为自定义(无拼写迁移的目录精确比较)', () => {
  // minimax 目录行 MiniMax-M3 从未变更过拼写,case-only 变体不是存量迁移值,
  // 不得命中预设;同 provider 未收录 ID 也保持自定义。
  assert.strictEqual(isPresetModel(mk({ preset: 'minimax', provider_kind: 'official_api', vendor: 'minimax', base_url: 'https://api.minimaxi.com/v1', model: 'minimax-m3' })), false);
  assert.strictEqual(isPresetModel(mk({ preset: 'minimax', provider_kind: 'official_api', vendor: 'minimax', base_url: 'https://api.minimaxi.com/v1', model: 'MiniMax-M3' })), true);
});
test('Coding Plan 手填 ID -> 自定义', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4', model: 'my-custom-glm' })), false);
});
test('官方 API 命中目录(deepseek-v4-pro) -> 预设', () => {
  assert.strictEqual(isPresetModel(mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model: 'deepseek-v4-pro' })), true);
});
test('deepseek 目录行退役拼写经 legacyAliases 仍归预设,新主力 deepseek-flash 精确命中', () => {
  // deepseek-v4-flash / -vision-exp 行已删并登记为 deepseek-flash 的
  // legacyAliases:存量配置必须继续识别为预设,而不是回落自定义。
  const mkDeepseek = model => mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model });
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-flash')), true, '新主力拼写精确命中');
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-flash')), true, '已删行拼写经 legacyAliases 命中');
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-flash-vision-exp')), true);
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-pro')), true);
});
test('2026-09-11 目录新增行归入对应 provider 组(预设识别)', () => {
  const mkCloud = (preset, vendor, base, model) => mk({ preset, provider_kind: 'official_api', vendor, base_url: base, model });
  // bigmodel Coding Plan 新增 glm-5.3 / glm-5.3-flash(多模态)
  const bigmodel = 'https://open.bigmodel.cn/api/coding/paas/v4';
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: bigmodel, model: 'glm-5.3' })), true);
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: bigmodel, model: 'glm-5.3-flash' })), true);
  // GLM 开放平台与 z.ai 新增 glm-5.3 / glm-5.3-flash
  assert.strictEqual(isPresetModel(mkCloud('glm', 'glm', 'https://open.bigmodel.cn/api/paas/v4', 'glm-5.3')), true);
  assert.strictEqual(isPresetModel(mkCloud('glm', 'glm', 'https://api.z.ai/api/paas/v4', 'glm-5.3-flash')), true);
  // xAI 新增 grok-4.6;Gemini 新增 gemini-3.8-flash;OpenAI 新增 gpt-6-astra;
  // Anthropic 新增 claude-fable-5-1;豆包新增编程特化预览行
  assert.strictEqual(isPresetModel(mkCloud('xai', 'xai', 'https://api.x.ai/v1', 'grok-4.6')), true);
  assert.strictEqual(isPresetModel(mkCloud('gemini', 'gemini', 'https://generativelanguage.googleapis.com/v1beta/openai', 'gemini-3.8-flash')), true);
  assert.strictEqual(isPresetModel(mkCloud('openai', 'openai', 'https://api.openai.com/v1', 'gpt-6-astra')), true);
  assert.strictEqual(isPresetModel(mkCloud('anthropic', 'anthropic', 'https://api.anthropic.com/v1', 'claude-fable-5-1')), true);
  assert.strictEqual(isPresetModel(mkCloud('doubao', 'doubao', 'https://ark.cn-beijing.volces.com/api/v3', 'doubao-seed-2-0-code-preview-260215')), true);
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
test('cloud model alias takes precedence in the main label', () => {
  assert.strictEqual(selectorMainLabel(mk({ alias: 'Daily assistant', model: 'deepseek-v4-pro' }), t), 'Daily assistant');
});
test('blank model alias falls back to the existing label', () => {
  assert.strictEqual(selectorMainLabel(mk({ alias: '   ', model: 'meta-llama/llama-4-scout' }), t), 'meta-llama/llama-4-scout');
});
test('local model alias is ignored by selector labels', () => {
  const local = mk({ alias: 'Must not render', name: '我的模型', preset: 'local_vllm', model: 'qwen36_35b_256k' });
  assert.strictEqual(selectorMainLabel(local, t), '我的模型');
  assert.strictEqual(selectorSubLabel(local, t), 'qwen36_35b_256k');
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
test('aliased model subtitle preserves the wire model ID', () => {
  assert.strictEqual(selectorSubLabel(mk({ alias: 'Daily assistant', model: 'deepseek-v4-pro' }), t), 'deepseek-v4-pro');
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
  // vm context arrays live in a different realm than the host; deepStrictEqual would false-positive on prototypes, so spread into host arrays to normalize
  const tiers = model => [...reasoningEffortTiersForModel(model) || []];
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
  // GLM-5.3-Flash 同入 z.ai tiered effort 路由（off/high/max）
  const zai53Flash = { preset: 'glm', vendor: 'glm', model: 'glm-5.3-flash', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zai53Flash), ['off', 'high', 'max']);
  const kimiCodeK3 = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://api.kimi.com/coding/v1' };
  assert.deepStrictEqual(tiers(kimiCodeK3), ['low', 'high', 'max']);
  // 底座 is_exact_kimi_code_k3_route 同时收录 k3-256k：Kimi Code 端点上同为 tiered low/high/max
  const kimiCodeK3256k = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3-256k', base_url: 'https://api.kimi.com/coding/v1' };
  assert.deepStrictEqual(tiers(kimiCodeK3256k), ['low', 'high', 'max']);
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
  // OpenAI 非 reasoning 系（gpt-5.4-mini）与 qwen/gemini/自定义兼容不提供切换
  const openaiMini = { preset: 'openai', vendor: 'openai', model: 'gpt-5.4-mini' };
  assert.strictEqual(reasoningEffortTiersForModel(openaiMini), null);
  // xai：仅精确 https://api.x.ai/v1 的 grok-4.6（low/medium/high/max，max wire 发 xhigh）
  // 与 grok-4.5（low/medium/high，xhigh/max 降级为 high 故不暴露）提供档位；Grok 推理
  // 不可关，不暴露 off。其余型号、无端点、非官方端点（如 openrouter 网关）→ null。
  const xai46 = { preset: 'xai', vendor: 'xai', model: 'grok-4.6', base_url: 'https://api.x.ai/v1' };
  assert.deepStrictEqual(tiers(xai46), ['low', 'medium', 'high', 'max']);
  const xai45 = { preset: 'xai', vendor: 'xai', model: 'grok-4.5', base_url: 'https://api.x.ai/v1' };
  assert.deepStrictEqual(tiers(xai45), ['low', 'medium', 'high']);
  const xai43Official = { preset: 'xai', vendor: 'xai', model: 'grok-4.3', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(xai43Official), null);
  const xaiBuildOfficial = { preset: 'xai', vendor: 'grok', model: 'grok-build-0.1', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(xaiBuildOfficial), null);
  const xai46NoBase = { preset: 'xai', vendor: 'xai', model: 'grok-4.6' };
  assert.strictEqual(reasoningEffortTiersForModel(xai46NoBase), null);
  const xai46Gateway = { preset: 'xai', vendor: 'xai', model: 'grok-4.6', base_url: 'https://openrouter.ai/api/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(xai46Gateway), null);
  const qwen = { preset: 'qwen', vendor: 'qwen', model: 'qwen3.8-max' };
  assert.strictEqual(reasoningEffortTiersForModel(qwen), null);
  const gemini = { preset: 'gemini', vendor: 'gemini', model: 'gemini-3.6-flash' };
  assert.strictEqual(reasoningEffortTiersForModel(gemini), null);
  const custom = { preset: 'openai_compatible', model: 'my-model' };
  assert.strictEqual(reasoningEffortTiersForModel(custom), null);
  // tiered effort 只认精确 first-party 端点：兼容网关同型号回落通用档位（fail-closed）。
  // 直连平台端点（国际 api.moonshot.ai 与中国 api.moonshot.cn）的 kimi-k3 都走
  // 底座 always-thinking K3 tiered 路由（low/high/max，off 归一为 low）。
  const moonshotCn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.deepStrictEqual(tiers(moonshotCn), ['low', 'high', 'max']);
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
  // /beta 仍为官方端点（对齐 Rust is_official_deepseek_base_url）；
  // api.deepseeki.com 非官方域名（官方文档从未出现，社区按 typosquat 处理），
  // 不再视为官方端点 → 不提供 deepseek 四档。
  const deepseekBeta = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseek.com/beta' };
  assert.deepStrictEqual(tiers(deepseekBeta), ['off', 'low', 'high', 'max']);
  const deepseeki = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseeki.com' };
  assert.strictEqual(reasoningEffortTiersForModel(deepseeki), null, 'deepseeki.com 非官方域名,不得回落 deepseek 档位');
  // volcengine：底座把 low/medium 归一为 high，仅 off/high/max 有区别
  const volcengine = { preset: 'doubao', vendor: 'doubao', model: 'doubao-seed-evolving' };
  assert.deepStrictEqual(tiers(volcengine), ['off', 'high', 'max']);
  // xiaomi-mimo：只有 thinking 开关（off/enabled），off/high 两档
  const mimo = { preset: 'mimo', vendor: 'mimo', model: 'mimo-v2.5-pro' };
  assert.deepStrictEqual(tiers(mimo), ['off', 'high']);
  // 本地 loopback OpenAI 兼容端点：探测后走 Ollama think 开关 / vLLM 档位，提供四档
  const localOllama = { preset: 'openai_compatible', model: 'qwen3:8b', base_url: 'http://127.0.0.1:11434/v1' };
  assert.deepStrictEqual(tiers(localOllama), ['off', 'low', 'medium', 'high']);
  const localLocalhost = { preset: 'openai_compatible', model: 'local-model', base_url: 'http://localhost:8000/v1' };
  assert.deepStrictEqual(tiers(localLocalhost), ['off', 'low', 'medium', 'high']);
  // 远端自定义 OpenAI 兼容端点不提供切换（无本地思考控制 wire）
  const remoteCustom = { preset: 'openai_compatible', model: 'my-model', base_url: 'https://api.example.com/v1' };
  assert.strictEqual(reasoningEffortTiersForModel(remoteCustom), null);
});

test('OpenAI reasoning 家族判定对齐底座 model_is_openai_reasoning_family（含手输自定义模型）', () => {
  const tiers = model => [...reasoningEffortTiersForModel(model) || []];
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
  const tiers = model => [...reasoningEffortTiersForModel(model) || []];
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
  assert.deepStrictEqual([...reasoningEffortTiersForModel(k26)], ['off', 'high']);
  assert.ok(![...reasoningEffortTiersForModel(k3)].includes('off'));
  assert.strictEqual(reasoningEffortForModelSwitch(k3), 'high');
  // 无档位模型切换置 null（未显式设置）；vllm 切回 off
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'xai', vendor: 'xai', model: 'grok-4.3' }), null);
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'local_vllm', model: 'qwen36_35b_256k' }), 'off');
  // z.ai glm-5.2 切换默认 high；中国端点 glm-5.2 无档位 → null
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://api.z.ai/api/paas/v4' }), 'high');
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' }), null);
});

test('baseUrlUsesLoopback 与 Rust bridge.rs 判定对齐', () => {
  // 回环：localhost / 127.0.0.0/8 / ::1（含展开形式）
  assert.strictEqual(baseUrlUsesLoopback('http://127.0.0.1:11434/v1'), true);
  assert.strictEqual(baseUrlUsesLoopback('http://127.255.0.1:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLoopback('http://localhost:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLoopback('http://LOCALHOST:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLoopback('http://[::1]:11434/v1'), true);
  assert.strictEqual(baseUrlUsesLoopback('http://[0:0:0:0:0:0:0:1]:11434/v1'), true);
  // 去尾点：127.0.0.1. 与 localhost. 仍按回环（对齐 Rust 的 trim_end_matches('.')）
  assert.strictEqual(baseUrlUsesLoopback('http://127.0.0.1.:11434/v1'), true);
  // 非回环：0.0.0.0 不是 loopback（Rust IpAddr::is_loopback() 语义）、
  // IPv4-mapped ::ffff:127.x 不是 ::1/128、公网/局域网/域名均非本地
  assert.strictEqual(baseUrlUsesLoopback('http://0.0.0.0:11434/v1'), false);
  assert.strictEqual(baseUrlUsesLoopback('http://[::ffff:127.0.0.1]:11434/v1'), false);
  assert.strictEqual(baseUrlUsesLoopback('http://[2001:db8::1]:11434/v1'), false);
  assert.strictEqual(baseUrlUsesLoopback('http://192.168.1.10:11434/v1'), false);
  assert.strictEqual(baseUrlUsesLoopback('https://api.example.com/v1'), false);
  assert.strictEqual(baseUrlUsesLoopback(''), false);
  assert.strictEqual(baseUrlUsesLoopback('not-a-url'), false);
});

test('baseUrlUsesLocalOrPrivate 覆盖 loopback/RFC1918/Docker 宿主别名', () => {
  // loopback（与 baseUrlUsesLoopback 一致）
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://127.0.0.1:11434/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://localhost:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://[::1]:11434/v1'), true);
  // RFC1918 私网段
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://10.0.0.5:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://172.16.3.4:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://172.31.255.254:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://192.168.1.10:11434/v1'), true);
  // 172.32 不在 172.16/12 段内
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://172.32.1.1:8000/v1'), false);
  // Docker 宿主别名
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://host.docker.internal:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://host.lima.internal:8000/v1'), true);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('http://myapp.docker.internal:9000/v1'), true);
  // 公网/域名非本地
  assert.strictEqual(baseUrlUsesLocalOrPrivate('https://api.deepseek.com/v1'), false);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('https://gateway.example.com/v1'), false);
  assert.strictEqual(baseUrlUsesLocalOrPrivate('https://192.168.1.10.example.com/v1'), false);
  assert.strictEqual(baseUrlUsesLocalOrPrivate(''), false);
});

test('localProbeTiersForKind 按探测结果映射真实档位', () => {
  // vllm → 四档；ollama → think 开关两档（避免 low/medium/high 归一误导）
  assert.deepStrictEqual([...localProbeTiersForKind('vllm')], ['off', 'low', 'medium', 'high']);
  assert.deepStrictEqual([...localProbeTiersForKind('ollama')], ['off', 'high']);
  // Frameworks wire-isomorphic with vLLM thinking control → same four tiers as vllm
  for (const kind of ['sglang', 'llamacpp', 'koboldcpp', 'lmdeploy', 'dockermodelrunner']) {
    assert.deepStrictEqual([...localProbeTiersForKind(kind)], ['off', 'low', 'medium', 'high'], kind);
  }
  // lmstudio/generic 底座空操作 → null（前端显示不支持提示）
  assert.strictEqual(localProbeTiersForKind('lmstudio'), null);
  assert.strictEqual(localProbeTiersForKind('generic'), null);
  // 未探测/未知 → 默认四档（前端探测完成前不误报不支持）
  assert.deepStrictEqual([...localProbeTiersForKind(null)], ['off', 'low', 'medium', 'high']);
  assert.deepStrictEqual([...localProbeTiersForKind('unknown')], ['off', 'low', 'medium', 'high']);
});

test('alwaysThinkingSpecForModel: always-thinking model knowledge table matching', () => {
  // vm realm objects have different prototypes from the test realm; compare via JSON structure
  const specJson = (modelId) => JSON.stringify(alwaysThinkingSpecForModel(modelId));
  // case-insensitive + underscores/spaces normalized to '-'
  assert.strictEqual(specJson('kimi-k3'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('Kimi-K3'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('kimi_k3'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('Kimi K3 Instruct'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('glm-5.3'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('GLM-4.7'), '{"tiers":["low","high"]}');
  assert.strictEqual(specJson('gpt-oss-120b'), '{"tiers":["low","medium","high"]}');
  // noControl: thinking cannot be disabled and no effort tier is controllable
  assert.strictEqual(specJson('kimi-k2-thinking'), '{"noControl":true}');
  assert.strictEqual(specJson('Kimi-K2.5-Thinking'), '{"noControl":true}');
  assert.strictEqual(specJson('kimi-k2.7'), '{"noControl":true}');
  assert.strictEqual(specJson('deepseek-r1-0528'), '{"noControl":true}');
  assert.strictEqual(specJson('MiniMax-M2'), '{"noControl":true}');
  assert.strictEqual(specJson('Qwen3-235B-A22B-Thinking'), '{"noControl":true}');
  // no match: plain qwen3 (no thinking), other models, empty values
  assert.strictEqual(alwaysThinkingSpecForModel('qwen3-32b'), null);
  assert.strictEqual(alwaysThinkingSpecForModel('qwen36_35b_256k'), null);
  assert.strictEqual(alwaysThinkingSpecForModel('glm-5.2'), null);
  assert.strictEqual(alwaysThinkingSpecForModel(''), null);
  assert.strictEqual(alwaysThinkingSpecForModel(null), null);
});

test('local routes hitting the knowledge table: tiers/defaults/stored-value normalization', () => {
  // spec.tiers model (local vllm preset): tier table replaced by the knowledge table, no off tier
  const k3Local = { preset: 'local_vllm', model: 'kimi-k3' };
  assert.deepStrictEqual([...reasoningEffortTiersForModel(k3Local)], ['low', 'high']);
  assert.strictEqual(defaultReasoningEffortForModel(k3Local), 'low');
  // stored off is not in spec.tiers → normalized to the lowest tier low; valid high is kept as-is
  assert.strictEqual(normalizeStoredReasoningEffort(k3Local, 'off'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Local, 'high'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Local, null), 'low');
  // local loopback openai_compatible endpoints also go through the knowledge table
  const gptOssLocal = { preset: 'openai_compatible', model: 'gpt-oss-20b', base_url: 'http://127.0.0.1:8000/v1' };
  assert.deepStrictEqual([...reasoningEffortTiersForModel(gptOssLocal)], ['low', 'medium', 'high']);
  assert.strictEqual(defaultReasoningEffortForModel(gptOssLocal), 'low');
  // noControl model → null (no switch offered, reusing the "not adjustable" semantic exit)
  const r1Local = { preset: 'local_vllm', model: 'deepseek-r1:14b' };
  assert.strictEqual(reasoningEffortTiersForModel(r1Local), null);
  assert.strictEqual(normalizeStoredReasoningEffort(r1Local, 'high'), null);
  // plain local models are unaffected: still default off, four tiers
  const qwenLocal = { preset: 'local_vllm', model: 'qwen3-32b' };
  assert.deepStrictEqual([...reasoningEffortTiersForModel(qwenLocal)], ['off', 'low', 'medium', 'high']);
  assert.strictEqual(defaultReasoningEffortForModel(qwenLocal), 'off');
  // exact cloud routes are unaffected: z.ai first-party glm-5.3 is still off/high/max
  const glmCloud = { preset: 'glm', vendor: 'glm', model: 'glm-5.3', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual([...reasoningEffortTiersForModel(glmCloud)], ['off', 'high', 'max']);
  // direct moonshot cloud route for K3 unchanged: low/high/max
  const k3Direct = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.deepStrictEqual([...reasoningEffortTiersForModel(k3Direct)], ['low', 'high', 'max']);
});

test('localReasoningTiers: probed tiers overlaid with the model knowledge table', () => {
  // spec.tiers overrides the probed tiers (even when the probe reports a four-tier framework)
  assert.deepStrictEqual([...localReasoningTiers('kimi-k3', 'vllm')], ['low', 'high']);
  assert.deepStrictEqual([...localReasoningTiers('gpt-oss-120b', 'sglang')], ['low', 'medium', 'high']);
  // under the ollama route the engine wire only has boolean think, so the only meaningful exposure for an always-thinking model is high
  assert.deepStrictEqual([...localReasoningTiers('gpt-oss-120b', 'ollama')], ['high']);
  // noControl → null (frontend shows a "thinking is always on" notice)
  assert.strictEqual(localReasoningTiers('deepseek-r1:14b', 'vllm'), null);
  assert.strictEqual(localReasoningTiers('Qwen3-235B-A22B-Thinking', 'ollama'), null);
  // lmstudio/generic: the engine's openai wire route is a no-op for reasoning_effort,
  // and knowledge-table tiers are likewise not offered — fall back to the probe result (null), restoring the "endpoint unsupported" notice
  assert.strictEqual(localReasoningTiers('kimi-k3', 'lmstudio'), null);
  assert.strictEqual(localReasoningTiers('gpt-oss-120b', 'generic'), null);
  // noControl on lmstudio/generic is likewise null (notice logic unchanged)
  assert.strictEqual(localReasoningTiers('deepseek-r1:14b', 'generic'), null);
  // no knowledge-table match → apply the probed tiers as-is
  assert.deepStrictEqual([...localReasoningTiers('qwen3-32b', 'ollama')], ['off', 'high']);
  assert.deepStrictEqual([...localReasoningTiers('qwen3-32b', 'llamacpp')], ['off', 'low', 'medium', 'high']);
  assert.strictEqual(localReasoningTiers('qwen3-32b', 'generic'), null);
  assert.deepStrictEqual([...localReasoningTiers('qwen3-32b', null)], ['off', 'low', 'medium', 'high']);
});

test('reasoningEffortDisplayForTiers: display fallback of stored tiers against probed tiers', () => {
  // ollama two-tier table: stored low/medium are wire-equivalent to high
  // (think:true); the highlight maps to the nearest tier, high
  assert.strictEqual(reasoningEffortDisplayForTiers('low', ['off', 'high']), 'high');
  assert.strictEqual(reasoningEffortDisplayForTiers('medium', ['off', 'high']), 'high');
  // in-table values return unchanged
  assert.strictEqual(reasoningEffortDisplayForTiers('off', ['off', 'high']), 'off');
  assert.strictEqual(reasoningEffortDisplayForTiers('high', ['off', 'high']), 'high');
  // four-tier table: no in-table tier is remapped
  assert.strictEqual(reasoningEffortDisplayForTiers('low', ['off', 'low', 'medium', 'high']), 'low');
  // max is not in the four-tier table: the core normalizes max to high; the highlight lands on high
  assert.strictEqual(reasoningEffortDisplayForTiers('max', ['off', 'low', 'medium', 'high']), 'high');
  // no high to land on (off-only table / empty table / non-array) → null (no highlight)
  assert.strictEqual(reasoningEffortDisplayForTiers('low', ['off']), null);
  assert.strictEqual(reasoningEffortDisplayForTiers('low', []), null);
  assert.strictEqual(reasoningEffortDisplayForTiers('low', 42), null);
  // no tier ever picked → null
  assert.strictEqual(reasoningEffortDisplayForTiers(null, ['off', 'high']), null);
});

test('defaultReasoningEffortForModel：vllm→off，其余支持档位的模型→high，不支持→null', () => {
  const deepseek = { preset: 'deepseek', vendor: 'deepseek', model: 'deepseek-v4-pro' };
  assert.strictEqual(defaultReasoningEffortForModel(deepseek), 'high');
  const vllm = { preset: 'local_vllm', model: 'qwen36_35b_256k' };
  assert.strictEqual(defaultReasoningEffortForModel(vllm), 'off');
  const xai = { preset: 'xai', vendor: 'xai', model: 'grok-4.3' };
  assert.strictEqual(defaultReasoningEffortForModel(xai), null);
  // xai 官方端点的 grok-4.6 提供档位，默认 high（与 deepseek 等一致）
  const xai46 = { preset: 'xai', vendor: 'xai', model: 'grok-4.6', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(defaultReasoningEffortForModel(xai46), 'high');
  assert.strictEqual(reasoningEffortForModelSwitch(xai46), 'high');
  // 本地 loopback OpenAI 兼容端点默认关闭思考（与 vllm 一致）
  const localOllama = { preset: 'openai_compatible', model: 'qwen3:8b', base_url: 'http://127.0.0.1:11434/v1' };
  assert.strictEqual(defaultReasoningEffortForModel(localOllama), 'off');
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
  assert.strictEqual(normalizeStoredReasoningEffort(deepseek), 'high');
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
  // 中国直连平台端点 api.moonshot.cn/v1 同入底座 K3 tiered 路由：off 归一为 low
  const k3Cn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(k3Cn, 'off'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Cn, 'medium'), 'high');
  // xai：grok-4.5 档位表为 low/medium/high，存量 max/xhigh 被底座降级 → 归一为 high；
  // grok-4.6 档位表含 max（wire 发 xhigh），存量 xhigh 原样归一为 max
  const xai45 = { preset: 'xai', vendor: 'xai', model: 'grok-4.5', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'max'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'xhigh'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'low'), 'low');
  const xai46 = { preset: 'xai', vendor: 'xai', model: 'grok-4.6', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(xai46, 'xhigh'), 'max');
  assert.strictEqual(normalizeStoredReasoningEffort(xai46, null), 'high');
  // 非 xai 官方端点/其余 Grok 型号无档位 → null
  assert.strictEqual(normalizeStoredReasoningEffort({ preset: 'xai', vendor: 'xai', model: 'grok-4.3', base_url: 'https://api.x.ai/v1' }, 'high'), null);
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

test('目录视觉能力标注(imageCapable):形状合法且查询只命中已标注条目', () => {
  const annotatedKeys = [];
  const annotatedIds = new Set();
  for (const scope of ['local', 'cloud']) {
    for (const group of MODEL_CATALOG[scope] || []) {
      for (const item of group.items || []) {
        if (item.imageCapable === undefined) continue;
        assert.ok(item.imageCapable === true || item.imageCapable === false,
          `imageCapable 只能是 true/false:${group.key}/${item.model}`);
        assert.ok(!item.custom, `custom 条目不应标注视觉能力:${group.key}`);
        assert.ok(item.model, `标注条目必须有模型 ID:${group.key}`);
        // 同一模型可出现在多个 provider 组(如 MiniMax-M3 中国/国际版),按组内唯一校验。
        annotatedKeys.push(`${group.key}/${item.model}`);
        annotatedIds.add(item.model);
      }
    }
  }
  assert.ok(annotatedKeys.length > 0, '目录至少保留一条视觉能力标注');
  assert.strictEqual(new Set(annotatedKeys).size, annotatedKeys.length, '同组内标注模型 ID 不得重复');
  // 跨组同 ID(含 legacyAliases)的显式标注必须一致:查询取第一个有标注的命中项,
  // 各组标注冲突时会静默依赖遍历序,在此提前拦截。
  const annotatedById = new Map();
  for (const scope of ['local', 'cloud']) {
    for (const group of MODEL_CATALOG[scope] || []) {
      for (const item of group.items || []) {
        if (item.imageCapable === undefined || item.custom) continue;
        for (const id of [item.model, ...(item.legacyAliases || [])]) {
          const known = annotatedById.get(id);
          assert.ok(known === undefined || known === item.imageCapable,
            `模型 ${id} 在多个组的 imageCapable 标注冲突`);
          if (known === undefined) annotatedById.set(id, item.imageCapable);
        }
      }
    }
  }
  // 查询:已标注命中其显式值(true 或 false);未标注/未命中/空值落 null(由「自动
  // 处理」链兜底)。false 为官方明示纯文本的显式标注,与「未标注」语义不同。
  for (const id of annotatedIds) {
    const flagged = catalogImageCapableForModel(id);
    assert.ok(flagged === true || flagged === false, `${id} 应命中标注`);
  }
  // 显式 true:deepseek 新主力(V4.1-Flash),已删行拼写经 legacyAliases 同样命中
  assert.strictEqual(catalogImageCapableForModel('deepseek-flash'), true);
  assert.strictEqual(catalogImageCapableForModel('deepseek-v4-flash'), true);
  // 显式 false:官方明示纯文本的旗舰行
  assert.strictEqual(catalogImageCapableForModel('deepseek-v4-pro'), false);
  assert.strictEqual(catalogImageCapableForModel('glm-5.2'), false, 'glm 组 glm-5.2 显式标注纯文本');
  assert.strictEqual(catalogImageCapableForModel('qwen3.7-max'), false);
  // bigmodel/z.ai coding 组的 glm-5.2 行未标注,查询不应被未标注命中项短路
  assert.strictEqual(catalogImageCapableForModel('glm-5.3'), false);
  assert.strictEqual(catalogImageCapableForModel('完全不存在的模型'), null);
  assert.strictEqual(catalogImageCapableForModel(''), null);
  assert.strictEqual(catalogImageCapableForModel(null), null);
});

console.log(`\nmodel_catalog_grouping: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
