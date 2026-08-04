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
  `this.providerLabelForModel = providerLabelForModel;\n`,
  ctx,
  { filename: srcPath },
);

const { isPresetModel, groupModelsForSelector, localUserNamed, selectorMainLabel, selectorSubLabel, providerLabelForModel } = ctx;

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

console.log(`\nmodel_catalog_grouping: ${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
