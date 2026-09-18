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
  `this.MODEL_PRESET_DEFS = MODEL_PRESET_DEFS;\n` +
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

const { isPresetModel, catalogItemMatchesModel, MODEL_CATALOG, MODEL_PRESET_DEFS, groupModelsForSelector, localUserNamed, selectorMainLabel, selectorSubLabel, providerLabelForModel, reasoningEffortTiersForModel, defaultReasoningEffortForModel, reasoningEffortForModelSwitch, normalizeStoredReasoningEffort, baseUrlUsesLoopback, baseUrlUsesLocalOrPrivate, localProbeTiersForKind, alwaysThinkingSpecForModel, localReasoningTiers, catalogImageCapableForModel, reasoningEffortDisplayForTiers } = ctx;

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
test('z.ai coding catalog rows move to the official lowercase spelling; legacy uppercase GLM-5.2 configs still classify as preset via legacyAliases', () => {
  // The 2026-09 catalog rows use z.ai's official lowercase wire ids (docs.z.ai
  // API enum); existing configs may hold the old uppercase catalog value
  // (GLM-5.2), recognized via legacyAliases. GLM-5-Turbo was never released on
  // z.ai; its old row is deleted and existing configs fall back to custom.
  const mkZai = model => mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: 'https://api.z.ai/api/coding/paas/v4', model });
  assert.strictEqual(isPresetModel(mkZai('glm-5.2')), true, 'lowercase current spelling hits exactly');
  assert.strictEqual(isPresetModel(mkZai('GLM-5.2')), true, 'legacy uppercase config hits via legacyAliases');
  assert.strictEqual(isPresetModel(mkZai('glm-5.3')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-5.3-flash')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-4.7')), true);
  assert.strictEqual(isPresetModel(mkZai('glm-5-turbo')), false, 'GLM-5-Turbo row is deleted; falls back to custom');
  // The GLM-5-Turbo row on z.ai's official paas endpoint is deleted too
  // (absent from docs.z.ai's current overview/
  // pricing/API enum), so existing configs likewise fall back to custom.
  const mkZaiPaas = model => mk({ preset: 'glm', provider_kind: 'official_api', vendor: 'glm', base_url: 'https://api.z.ai/api/paas/v4', model });
  assert.strictEqual(isPresetModel(mkZaiPaas('GLM-5-Turbo')), false, 'the z.ai paas GLM-5-Turbo row is deleted; falls back to custom');
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
  assert.strictEqual(isPresetModel(mkPlan('kimi-k2.5')), false, 'kimi-k2.5 is retired platform-wide and removed from the catalog');
  assert.strictEqual(isPresetModel(mkPlan('kimi-k-2-5')), false, 'kimi-k-2-5 parallel spelling is no longer listed either');
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
  assert.strictEqual(isPresetModel(mkTokenPlan('kimi-k2.5')), false, 'kimi-k2.5 is retired; no Tencent Cloud group lists it anymore');
  assert.strictEqual(isPresetModel(mkTokenPlan('kimi-k-2-5')), false, 'kimi-k-2-5 parallel spelling is no longer listed either');
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'tencent', base_url: 'https://api.lkeap.cloud.tencent.com/coding/v3', model: 'glm-5.2' })), false, 'Token Plan-exclusive models must not match the Coding Plan catalog');
  assert.strictEqual(providerLabelForModel(mkTokenPlan('glm-5.2'), t), '腾讯云 Token Plan / Tencent Cloud Token Plan');
});
test('catalogItemMatchesModel 精确比较+legacyAliases 兼容迁移拼写', () => {
  // SettingsView 编辑弹窗的 initialCatalogMatch/known/active 均复用此比较:
  // The z.ai direct catalog row now uses the official lowercase glm-5.2
  // spelling; existing uppercase GLM-5.2 configs must hit via
  // legacyAliases and must not be misclassified as custom; everything except
  // explicitly registered historical spellings stays exact-compare.
  const zaiGroup = MODEL_CATALOG.cloud.find(group => group.key === 'glm_coding_plan_global');
  const glm52 = zaiGroup.items.find(item => item.model === 'glm-5.2');
  assert.ok(glm52, 'the z.ai direct catalog should contain the canonical lowercase glm-5.2 row');
  assert.deepStrictEqual([...(glm52.legacyAliases || [])], ['GLM-5.2'], 'the old uppercase catalog value must be registered as a legacyAlias');
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
test('deepseek catalog retired spellings still classify as preset via legacyAliases; the new mainline deepseek-flash hits exactly', () => {
  // The deepseek-v4-flash / -vision-exp rows are deleted and registered as
  // legacyAliases of deepseek-flash: existing configs must keep being
  // recognized as preset instead of falling back to custom.
  const mkDeepseek = model => mk({ preset: 'deepseek', provider_kind: 'official_api', vendor: 'deepseek', base_url: 'https://api.deepseek.com', model });
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-flash')), true, 'the new mainline spelling hits exactly');
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-flash')), true, 'the deleted-row spelling hits via legacyAliases');
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-flash-vision-exp')), true);
  assert.strictEqual(isPresetModel(mkDeepseek('deepseek-v4-pro')), true);
});
test('2026-09-11 catalog additions land in their provider groups (preset recognition)', () => {
  const mkCloud = (preset, vendor, base, model) => mk({ preset, provider_kind: 'official_api', vendor, base_url: base, model });
  // bigmodel Coding Plan adds glm-5.3 / glm-5.3-flash (multimodal)
  const bigmodel = 'https://open.bigmodel.cn/api/coding/paas/v4';
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: bigmodel, model: 'glm-5.3' })), true);
  assert.strictEqual(isPresetModel(mk({ preset: 'openai_compatible', provider_kind: 'coding_plan', vendor: 'glm', base_url: bigmodel, model: 'glm-5.3-flash' })), true);
  // GLM open platform and z.ai add glm-5.3 / glm-5.3-flash
  assert.strictEqual(isPresetModel(mkCloud('glm', 'glm', 'https://open.bigmodel.cn/api/paas/v4', 'glm-5.3')), true);
  assert.strictEqual(isPresetModel(mkCloud('glm', 'glm', 'https://api.z.ai/api/paas/v4', 'glm-5.3-flash')), true);
  // xAI adds grok-4.6; Gemini adds gemini-3.8-flash; OpenAI adds gpt-6-astra;
  // Anthropic adds claude-fable-5-1; Doubao adds the coding-specialized preview row
  assert.strictEqual(isPresetModel(mkCloud('xai', 'xai', 'https://api.x.ai/v1', 'grok-4.6')), true);
  assert.strictEqual(isPresetModel(mkCloud('gemini', 'gemini', 'https://generativelanguage.googleapis.com/v1beta/openai', 'gemini-3.8-flash')), true);
  assert.strictEqual(isPresetModel(mkCloud('openai', 'openai', 'https://api.openai.com/v1', 'gpt-6-astra')), true);
  assert.strictEqual(isPresetModel(mkCloud('anthropic', 'anthropic', 'https://api.anthropic.com/v1', 'claude-fable-5-1')), true);
  assert.strictEqual(isPresetModel(mkCloud('doubao', 'doubao', 'https://ark.cn-beijing.volces.com/api/v3', 'doubao-seed-2-0-code-preview-260215')), true);
  // the qwen international group lists qwen3.7-flash (in the international
  // full catalog, restored on the 2026-09-11 re-check)
  assert.strictEqual(isPresetModel(mkCloud('qwen', 'qwen', 'https://dashscope-intl.aliyuncs.com/compatible-mode/v1', 'qwen3.7-flash')), true);
  // qwen3.8-flash enters all three qwen groups (cn / Token Plan / international)
  const qwenGroups = (MODEL_CATALOG.cloud || []).filter(g => (g.key || '').startsWith('qwen'));
  assert.strictEqual(qwenGroups.length, 3, 'sync this assertion when the qwen group count changes');
  for (const g of qwenGroups) {
    assert.ok(
      (g.items || []).some(i => i.model === 'qwen3.8-flash'),
      `${g.key} group should list qwen3.8-flash`,
    );
  }
});

test('MODEL_PRESET_DEFS default models match the locked Rust prefs figures (default_model_matches_vendor_docs_2026_09)', () => {
  // The identically named test in prefs/model.rs locks the Rust side; this
  // test locks the frontend side, so drift on either side surfaces explicitly
  // in the corresponding test (the qwen default once drifted for a long time
  // unlocked).
  const expected = {
    local_vllm: 'qwen36_35b_256k',
    deepseek: 'deepseek-flash',
    kimi: 'kimi-k3',
    openai_compatible: '',
    qwen: 'qwen3.8-max',
    doubao: 'doubao-seed-evolving',
    minimax: 'MiniMax-M3',
    glm: 'glm-5.3',
    mimo: 'mimo-v2.5-pro',
    openai: 'gpt-5.6-terra',
    anthropic: 'claude-sonnet-5',
    gemini: 'gemini-3.8-flash',
    xai: 'grok-4.6',
  };
  for (const [key, model] of Object.entries(expected)) {
    assert.strictEqual(MODEL_PRESET_DEFS[key] && MODEL_PRESET_DEFS[key].model, model, `${key} default model drift`);
  }
});

test('MODEL_PRESET_DEFS and the Rust default_model table cross-check their source (cross-language guard against one side being updated without the other)', () => {
  // Each side's own lock test only guards "changing the implementation but
  // forgetting the test"; an intentional update (implementation + this side's
  // test) that misses the other side drifts silently. This parses the
  // default_model match block of prefs/model.rs for a real cross-check,
  // eliminating the last cross-language mirror.
  const variantToKey = {
    LocalVllm: 'local_vllm', Deepseek: 'deepseek', Kimi: 'kimi',
    OpenaiCompatible: 'openai_compatible', Qwen: 'qwen', Doubao: 'doubao',
    Minimax: 'minimax', Glm: 'glm', Mimo: 'mimo', Openai: 'openai',
    Anthropic: 'anthropic', Gemini: 'gemini', Xai: 'xai',
  };
  const rustSrc = fs.readFileSync(
    path.join(__dirname, '..', 'src-tauri', 'src', 'platform', 'prefs', 'model.rs'), 'utf8',
  );
  const fnStart = rustSrc.indexOf('pub fn default_model(&self)');
  const fnEnd = rustSrc.indexOf('pub fn context_window_fallback', fnStart);
  assert.ok(fnStart > 0 && fnEnd > fnStart, 'prefs/model.rs should contain default_model and context_window_fallback');
  const rustTable = {};
  // Strip // line comments before matching (otherwise examples like
  // ModelPreset::X => "id" in doc comments would be taken as table entries):
  // the (?<!:) guard excludes the slashes of `https://` — naively stripping
  // `//` in the URL table would cut the match arm off at the protocol header
  // (same shape as the image_capability extractor, whose tables have no URLs
  // and can use the bare rule).
  for (const m of rustSrc.slice(fnStart, fnEnd).replace(/(?<!:)\/\/[^\n]*/g, '').matchAll(/ModelPreset::(\w+)\s*=>\s*"([^"]*)"/g)) {
    rustTable[m[1]] = m[2];
  }
  assert.deepStrictEqual(
    Object.keys(rustTable).sort((a, b) => a.localeCompare(b)),
    Object.keys(variantToKey).sort((a, b) => a.localeCompare(b)),
    'sync variantToKey and both lock tests when the Rust default_model variant set changes',
  );
  // One known, intentional difference: openai_compatible is deliberately
  // empty on the frontend (pure custom template), while the Rust side keeps
  // that preset's legacy migration fallback gpt-5.6-terra; the main.jsx
  // override pair is pinned by a separate test. The other 12 presets must be
  // equal item by item.
  assert.strictEqual(rustTable.OpenaiCompatible, 'gpt-5.6-terra');
  assert.strictEqual(MODEL_PRESET_DEFS.openai_compatible.model, '');
  for (const [variant, key] of Object.entries(variantToKey)) {
    if (variant === 'OpenaiCompatible') continue;
    assert.strictEqual(
      rustTable[variant], MODEL_PRESET_DEFS[key] && MODEL_PRESET_DEFS[key].model,
      `${variant} and frontend ${key} default model drift`,
    );
  }
});

test('MODEL_PRESET_DEFS and the Rust default_base_url table cross-check their source (cross-language guard against baseUrl drift)', () => {
  // Same shape as the default_model cross-check: default_model already has
  // triple guarding, but the 13
  // default_base_url literals previously had no cross-language check at all —
  // changes like the minimax domain migration are the most likely to touch
  // only one side and drift silently.
  const variantToKey = {
    LocalVllm: 'local_vllm', Deepseek: 'deepseek', Kimi: 'kimi',
    OpenaiCompatible: 'openai_compatible', Qwen: 'qwen', Doubao: 'doubao',
    Minimax: 'minimax', Glm: 'glm', Mimo: 'mimo', Openai: 'openai',
    Anthropic: 'anthropic', Gemini: 'gemini', Xai: 'xai',
  };
  const rustSrc = fs.readFileSync(
    path.join(__dirname, '..', 'src-tauri', 'src', 'platform', 'prefs', 'model.rs'), 'utf8',
  );
  const fnStart = rustSrc.indexOf('pub fn default_base_url(&self)');
  const fnEnd = rustSrc.indexOf('pub fn default_model(&self)', fnStart);
  assert.ok(fnStart > 0 && fnEnd > fnStart, 'prefs/model.rs should contain default_base_url and default_model');
  const rustTable = {};
  // Strip // line comments before matching (otherwise examples like
  // ModelPreset::X => "id" in doc comments would be taken as table entries):
  // the (?<!:) guard excludes the slashes of `https://` — naively stripping
  // `//` in the URL table would cut the match arm off at the protocol header
  // (same shape as the image_capability extractor, whose tables have no URLs
  // and can use the bare rule).
  for (const m of rustSrc.slice(fnStart, fnEnd).replace(/(?<!:)\/\/[^\n]*/g, '').matchAll(/ModelPreset::(\w+)\s*=>\s*"([^"]*)"/g)) {
    rustTable[m[1]] = m[2];
  }
  assert.deepStrictEqual(
    Object.keys(rustTable).sort((a, b) => a.localeCompare(b)),
    Object.keys(variantToKey).sort((a, b) => a.localeCompare(b)),
    'sync variantToKey when the Rust default_base_url variant set changes',
  );
  // One known, intentional difference: openai_compatible is deliberately
  // empty on the frontend (pure custom template), while the Rust side keeps
  // that preset's legacy migration fallback https://api.openai.com/v1; the
  // main.jsx override pair is pinned by a separate test. The other 12 presets
  // must be equal item by item.
  assert.strictEqual(rustTable.OpenaiCompatible, 'https://api.openai.com/v1');
  assert.strictEqual(MODEL_PRESET_DEFS.openai_compatible.baseUrl, '');
  for (const [variant, key] of Object.entries(variantToKey)) {
    if (variant === 'OpenaiCompatible') continue;
    assert.strictEqual(
      rustTable[variant], MODEL_PRESET_DEFS[key] && MODEL_PRESET_DEFS[key].baseUrl,
      `${variant} and frontend ${key} default baseUrl drift`,
    );
  }
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
  // GLM-5.3-Flash joins the z.ai tiered effort route too (off/high/max)
  const zai53Flash = { preset: 'glm', vendor: 'glm', model: 'glm-5.3-flash', base_url: 'https://api.z.ai/api/paas/v4' };
  assert.deepStrictEqual(tiers(zai53Flash), ['off', 'high', 'max']);
  // open.bigmodel.cn/api/paas/v4 is also a first-party tiered route since base
  // #53
  // (the third host of is_exact_zai_chat_route, and the glm preset's default
  // endpoint): its tiers must match
  // api.z.ai, otherwise the engine consumes tiered effort while the UI has no
  // switch.
  const bigmodel53 = { preset: 'glm', vendor: 'glm', model: 'glm-5.3', base_url: 'https://open.bigmodel.cn/api/paas/v4' };
  assert.deepStrictEqual(tiers(bigmodel53), ['off', 'high', 'max']);
  const bigmodel53Flash = { preset: 'glm', vendor: 'glm', model: 'glm-5.3-flash', base_url: 'https://open.bigmodel.cn/api/paas/v4' };
  assert.deepStrictEqual(tiers(bigmodel53Flash), ['off', 'high', 'max']);
  const bigmodel52 = { preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' };
  assert.deepStrictEqual(tiers(bigmodel52), ['off', 'high', 'max']);
  // Second-order effect of the third host: glm-5.1 / glm-5-turbo get off/high
  // on that host too (same source as api.z.ai; before base #53 they had no
  // tiers on that host).
  assert.deepStrictEqual(
    tiers({ preset: 'glm', vendor: 'glm', model: 'glm-5.1', base_url: 'https://open.bigmodel.cn/api/paas/v4' }),
    ['off', 'high'],
  );
  assert.deepStrictEqual(
    tiers({ preset: 'glm', vendor: 'glm', model: 'glm-5-turbo', base_url: 'https://open.bigmodel.cn/api/paas/v4' }),
    ['off', 'high'],
  );
  // the bigmodel coding host (auto-switch semantics) is explicitly excluded by
  // the base; neither side offers tiers
  const bigmodelCoding = { preset: 'glm', vendor: 'glm', model: 'glm-5.3', base_url: 'https://open.bigmodel.cn/api/coding/paas/v4' };
  assert.strictEqual(reasoningEffortTiersForModel(bigmodelCoding), null);
  const kimiCodeK3 = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://api.kimi.com/coding/v1' };
  assert.deepStrictEqual(tiers(kimiCodeK3), ['low', 'high', 'max']);
  // the base is_exact_kimi_code_k3_route also covers k3-256k: tiered
  // low/high/max on the Kimi Code endpoint too
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
  // OpenAI non-reasoning models (gpt-5.4-mini) and qwen/gemini/custom
  // compatible offer no switching
  const openaiMini = { preset: 'openai', vendor: 'openai', model: 'gpt-5.4-mini' };
  assert.strictEqual(reasoningEffortTiersForModel(openaiMini), null);
  // xai: only grok-4.6 on the exact https://api.x.ai/v1 (low/medium/high/max,
  // max sends xhigh on the wire)
  // and grok-4.5 (low/medium/high; xhigh/max are downgraded to high so not
  // exposed) offer tiers; Grok reasoning
  // cannot be turned off and off is not exposed. Other models, no endpoint, or
  // unofficial endpoints (e.g. an openrouter gateway) → null.
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
  // tiered effort only recognizes exact first-party endpoints: same models on
  // compatible gateways fall back to generic tiers (fail-closed).
  // kimi-k3 on both direct platform endpoints (international api.moonshot.ai
  // and China api.moonshot.cn) goes through the
  // base always-thinking K3 tiered route (low/high/max, off normalized to low).
  const moonshotCn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.deepStrictEqual(tiers(moonshotCn), ['low', 'high', 'max']);
  const k3OnDirectPlatform = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://api.moonshot.ai/v1' };
  assert.deepStrictEqual(tiers(k3OnDirectPlatform), ['off', 'high']);
  const k3OnGateway = { preset: 'openai_compatible', vendor: 'kimi', model: 'k3', base_url: 'https://gateway.example.com/v1' };
  assert.deepStrictEqual(tiers(k3OnGateway), ['off', 'high']);
  const zaiCodingPlanGlobal = { preset: 'openai_compatible', vendor: 'glm', model: 'glm-5.2', base_url: 'https://api.z.ai/api/coding/paas/v4' };
  assert.deepStrictEqual(tiers(zaiCodingPlanGlobal), ['off', 'high', 'max']);
  // zai: for compatible gateways / unverified models the base strips
  // thinking/reasoning_effort → no switching offered;
  // open.bigmodel.cn/api/paas/v4 is a first-party tiered route since #53 (see
  // the bigmodel53 case above), no longer part of the "China endpoint has no
  // tiers" exception.
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
  // /beta is still the official endpoint (aligned with Rust
  // is_official_deepseek_base_url, including the repeated-suffix
  // stripping semantics: trim_end_matches strips consecutive /beta, /v1);
  // api.deepseeki.com
  // is an unofficial domain (never listed in the official documentation; the
  // community reports it does not resolve,
  // awesome-deepseek-agent#311) and is no longer treated as official → no
  // deepseek four tiers.
  const deepseekBeta = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseek.com/beta' };
  assert.deepStrictEqual(tiers(deepseekBeta), ['off', 'low', 'high', 'max']);
  const deepseekRepeatedSuffix = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseek.com/v1/beta/beta' };
  assert.deepStrictEqual(tiers(deepseekRepeatedSuffix), ['off', 'low', 'high', 'max'], 'repeated /beta suffixes must count as the official endpoint exactly like Rust trim_end_matches');
  const deepseeki = { preset: 'openai_compatible', model: 'my-deepseek', base_url: 'https://api.deepseeki.com' };
  assert.strictEqual(reasoningEffortTiersForModel(deepseeki), null, 'deepseeki.com is an unofficial domain and must not fall back to the deepseek tiers');
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
  // z.ai glm-5.2 switch defaults to high; the bigmodel paas host (tiered route
  // since #53) is also
  // high; glm-5.2 on a compatible gateway has no tiers → null
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://api.z.ai/api/paas/v4' }), 'high');
  assert.strictEqual(reasoningEffortForModelSwitch({ preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' }), 'high');
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
  // grok-4.6 on the xai official endpoint offers tiers and defaults to high
  // (consistent with deepseek etc.)
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
  // The third host (open.bigmodel.cn/api/paas/v4) has a tier table since base
  // #53, so a stored medium value should normalize to high (same source as
  // api.z.ai).
  const bigmodelGlm = { preset: 'glm', vendor: 'glm', model: 'glm-5.2', base_url: 'https://open.bigmodel.cn/api/paas/v4' };
  assert.strictEqual(normalizeStoredReasoningEffort(bigmodelGlm, 'medium'), 'high');
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
  // the China direct platform endpoint api.moonshot.cn/v1 joins the base K3
  // tiered route too: off normalizes to low
  const k3Cn = { preset: 'kimi', vendor: 'kimi', model: 'kimi-k3', base_url: 'https://api.moonshot.cn/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(k3Cn, 'off'), 'low');
  assert.strictEqual(normalizeStoredReasoningEffort(k3Cn, 'medium'), 'high');
  // xai: the grok-4.5 tier table is low/medium/high and stored max/xhigh are
  // downgraded by the base → normalized to high;
  // the grok-4.6 tier table includes max (wire sends xhigh), so stored xhigh
  // normalizes to max as-is
  const xai45 = { preset: 'xai', vendor: 'xai', model: 'grok-4.5', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'max'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'xhigh'), 'high');
  assert.strictEqual(normalizeStoredReasoningEffort(xai45, 'low'), 'low');
  const xai46 = { preset: 'xai', vendor: 'xai', model: 'grok-4.6', base_url: 'https://api.x.ai/v1' };
  assert.strictEqual(normalizeStoredReasoningEffort(xai46, 'xhigh'), 'max');
  assert.strictEqual(normalizeStoredReasoningEffort(xai46, null), 'high');
  // off is not in the grok-4.6 tier table: normalized to high, matching the
  // base sending off as wire high
  assert.strictEqual(normalizeStoredReasoningEffort(xai46, 'off'), 'high');
  // non-xai official endpoints / other Grok models have no tiers → null
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
  // Query: annotated ids hit their explicit value (true or false);
  // unannotated / miss / empty fall to null (the "auto
  // processing" chain is the fallback). false is an explicit annotation for
  // officially text-only rows, semantically different from "unannotated".
  for (const id of annotatedIds) {
    const flagged = catalogImageCapableForModel(id);
    assert.ok(flagged === true || flagged === false, `${id} should hit an annotation`);
  }
  // explicit true: the new deepseek mainline (V4.1-Flash); deleted-row
  // spellings also hit via legacyAliases
  assert.strictEqual(catalogImageCapableForModel('deepseek-flash'), true);
  assert.strictEqual(catalogImageCapableForModel('deepseek-v4-flash'), true);
  // explicit false: flagship rows the official docs call text-only
  assert.strictEqual(catalogImageCapableForModel('deepseek-v4-pro'), false);
  assert.strictEqual(catalogImageCapableForModel('glm-5.2'), false, "the glm group's glm-5.2 is explicitly annotated text-only");
  assert.strictEqual(catalogImageCapableForModel('qwen3.7-max'), false);
  // the glm-5.3 rows in the coding/Token Plan groups are also explicitly
  // false, no longer relying on cross-group scan order
  assert.strictEqual(catalogImageCapableForModel('glm-5.3'), false);
  // Tencent lowercase wire spellings hit the same group's false annotation
  // (a case mismatch previously could only fall to null → auto)
  assert.strictEqual(catalogImageCapableForModel('minimax-m2.7'), false);
  assert.strictEqual(catalogImageCapableForModel('minimax-m-2-7'), false);
  assert.strictEqual(catalogImageCapableForModel('完全不存在的模型'), null);
  assert.strictEqual(catalogImageCapableForModel(''), null);
  assert.strictEqual(catalogImageCapableForModel(null), null);
});

test('imageCapable annotations cross-check the Rust builtin verified table source (frontend annotations must not diverge from backend resolution)', () => {
  // The frontend imageCapable and the backend image_capability.rs are the last
  // unguarded mirror pair: FE true
  // with BE Unknown would silently degrade official-route image input (the
  // form shows "supported" while sending resolves
  // Unknown); FE false with a BE hit would inline images for text-only rows on
  // send. This parses the two Rust
  // tables (substring VERIFIED + exact EXACT), recomputes
  // builtin_verified_supports_image on the JS side
  // and does bidirectional parity against the explicit frontend annotations;
  // unannotated (null) is unconstrained — both sides land on the "auto" chain.
  const rustSrc = fs.readFileSync(
    path.join(__dirname, '..', 'src-tauri', 'src', 'features', 'assistant', 'image_capability.rs'), 'utf8',
  );
  const extractTable = prefix => {
    const start = rustSrc.indexOf(prefix);
    assert.ok(start > 0, `image_capability.rs should contain ${prefix} (sync this cross-check if the table layout changes)`);
    const end = rustSrc.indexOf('];', start);
    assert.ok(end > start, 'the Rust table is not closed properly; sync this cross-check extractor');
    // strip comment lines inside the table before taking string literals, so
    // example spellings in comments do not pollute the entry set.
    return [...rustSrc.slice(start, end).replace(/\/\/[^\n]*/g, '').matchAll(/"([^"]+)"/g)].map(m => m[1]);
  };
  const substrings = extractTable('const VERIFIED_IMAGE_CAPABLE_MODELS');
  const exacts = extractTable('const EXACT_VERIFIED_IMAGE_CAPABLE_MODELS');
  assert.ok(substrings.length > 0, 'a VERIFIED table that parses to empty means format drift; fix this extractor');
  assert.ok(exacts.length > 0, 'an EXACT table that parses to empty means format drift; fix this extractor');
  const backendSupports = id => exacts.includes(id) || substrings.some(entry => id.includes(entry));
  let checked = 0;
  for (const scope of ['local', 'cloud']) {
    for (const group of MODEL_CATALOG[scope] || []) {
      for (const item of group.items || []) {
        if (item.custom || item.imageCapable === undefined) continue;
        for (const id of [item.model, ...(item.legacyAliases || [])]) {
          const fe = catalogImageCapableForModel(id);
          if (fe !== true && fe !== false) continue;
          checked += 1;
          const be = backendSupports(String(id).toLowerCase());
          if (fe === true) {
            assert.ok(be, `frontend annotates image-capable but backend resolves Unknown: ${group.key}/${id}`);
          } else {
            assert.ok(!be, `frontend annotates text-only but backend treats it as image-capable (images would be inlined on send): ${group.key}/${id}`);
          }
        }
      }
    }
  }
  assert.ok(checked >= 60, `parity check coverage is abnormally low (only ${checked} entries); the catalog annotations or alias structure appear to have drifted`);
});

// --- modelDescriptions i18n guardrails ---
// SettingsView only looks up modelDescriptions for non-custom rows (custom rows
// use the dedicated custom*Desc keys),
// and a missing entry would fall back to showing Chinese to en/ja users. The
// catalog and the dictionaries live in different files, and
// ui_language_coverage only does zh key parity, which does not cover this, so
// the sources are cross-checked directly.
const extractModelDescriptionKeys = file => {
  const src = fs.readFileSync(path.join(__dirname, '..', 'src', 'shared', 'i18n', file), 'utf8');
  const keys = new Set();
  for (const m of src.matchAll(/Object\.assign\(\w+\.uiSettingsDetail\.modelDescriptions,\s*\{([\s\S]*?)\n\}\);/g)) {
    for (const k of m[1].matchAll(/'([^']+)'\s*:/g)) keys.add(k[1]);
  }
  for (const m of src.matchAll(/modelDescriptions:\s*\{([^}]*)\}/g)) {
    for (const k of m[1].matchAll(/'([^']+)'\s*:/g)) keys.add(k[1]);
  }
  return keys;
};
const collectCatalogDescs = () => {
  const descs = new Set();
  for (const scope of ['local', 'cloud']) {
    for (const group of MODEL_CATALOG[scope] || []) {
      for (const item of group.items || []) {
        if (!item.custom && item.desc) descs.add(item.desc);
      }
    }
  }
  return descs;
};

test('modelDescriptions i18n completeness: every non-custom catalog desc has entries in en/ja', () => {
  const en = extractModelDescriptionKeys('en.js');
  const ja = extractModelDescriptionKeys('ja.js');
  const descs = collectCatalogDescs();
  assert.ok(descs.size > 0, 'should extract non-custom descs from the catalog');
  for (const desc of descs) {
    assert.ok(en.has(desc), `en is missing a catalog desc entry: ${desc}`);
    assert.ok(ja.has(desc), `ja is missing a catalog desc entry: ${desc}`);
  }
});

test('modelDescriptions has no stale keys left from the refresh (desc renames must clean up the en/ja/zh entries in sync)', () => {
  // The 2026-09 refresh renamed many descs and en/ja once accumulated 25 dead
  // keys. The former 6 baseline leftover keys are all cleared (the allowlist
  // is removed); any dead key from now on turns this test red.
  // zh is scanned too (zh's modelDescriptions is an optional override whose
  // key set is a subset of en's,
  // but it can accumulate dead keys the same way — the compatibility
  // highspeed/compatible-endpoint examples once slipped through).
  const descs = collectCatalogDescs();
  for (const file of ['en.js', 'ja.js', 'zh.js']) {
    const dead = [...extractModelDescriptionKeys(file)].filter(k => !descs.has(k));
    assert.deepStrictEqual(dead, [], `${file} has unreferenced modelDescriptions dead keys: ${dead.join(', ')}`);
  }
});

console.log(`\nmodel_catalog_grouping: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
