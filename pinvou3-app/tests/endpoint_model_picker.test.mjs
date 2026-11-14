import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

// 端点模型选择面板的纯逻辑单测(规格 §2.10 JS 侧)。组件 JSX 无法在 node:vm
// 里加载,故徽章格式化/过滤/去重/批量截断等纯函数被抽到
// endpoint-model-picker-logic.js(无 JSX、无 import),本文件按
// tests/model_catalog_grouping.test.js 的 vm 剥 ESM 关键字模式加载它。
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const srcPath = path.join(root, 'src', 'features', 'settings', 'endpoint-model-picker-logic.js');
let code = fs.readFileSync(srcPath, 'utf8');
// 剥离 ESM export(与 model_catalog_grouping.test.js 同款):先剥 export {} 清单,
// 再剥剩余的 export 前缀。文件必须保持无 import、无 JSX,vm 直接求值。
code = code.replace(/\bexport\s+\{[^}]+\};?/g, '').replace(/\bexport\s+/g, '');
// JSX-free/import-free 检查在剥注释后做:JSDoc 的 {Array<object|string>} 之类
// 类型标注会误触 <> 正则。
const codeNoComments = code.replace(/\/\*[\s\S]*?\*\//g, '').split('\n').map(line => line.replace(/\/\/.*$/, '')).join('\n');
assert.doesNotMatch(codeNoComments, /\bimport\s/, 'logic module must stay import-free for vm loading');
assert.doesNotMatch(codeNoComments, /<[A-Za-z][^>]*>/, 'logic module must stay JSX-free');

const ctx = { console };
vm.createContext(ctx);
vm.runInContext(
  `${code}\n` +
  `this.formatTokensCount = formatTokensCount;\n` +
  `this.factSourceLabelKey = factSourceLabelKey;\n` +
  `this.modelDuplicationKey = modelDuplicationKey;\n` +
  `this.savedModelKeys = savedModelKeys;\n` +
  `this.normalizeDiscoveredRows = normalizeDiscoveredRows;\n` +
  `this.filterModelRows = filterModelRows;\n` +
  `this.splitBatchAdd = splitBatchAdd;\n` +
  `this.FILTER_VISIBLE_ROW_LIMIT = FILTER_VISIBLE_ROW_LIMIT;\n` +
  `this.ADD_ALL_BATCH_LIMIT = ADD_ALL_BATCH_LIMIT;\n`,
  ctx,
  { filename: srcPath },
);
const {
  formatTokensCount,
  factSourceLabelKey,
  modelDuplicationKey,
  savedModelKeys,
  normalizeDiscoveredRows,
  filterModelRows,
  splitBatchAdd,
  FILTER_VISIBLE_ROW_LIMIT,
  ADD_ALL_BATCH_LIMIT,
} = ctx;

// vm 里创建的数组/对象/Set 属于另一个 realm,与宿主字面量 deepStrictEqual 会因
// 原型不等而误报;统一过一遍 JSON 边界(被测值全部 JSON 安全)再断言。
const toHost = value => JSON.parse(JSON.stringify(value));

// --- formatTokensCount(徽章 K/M 格式化,1000 基,与 MonitorView 上下文档一致) ---
test('formatTokensCount: K/M thresholds', () => {
  assert.equal(formatTokensCount(999), '999');
  assert.equal(formatTokensCount(1000), '1K');
  assert.equal(formatTokensCount(4096), '4K');
  assert.equal(formatTokensCount(262144), '262K');
  assert.equal(formatTokensCount(1048576), '1M');
  assert.equal(formatTokensCount(2000000), '2M');
  assert.equal(formatTokensCount(1500000), '1.5M');
  assert.equal(formatTokensCount(12000000), '12M');
});
test('formatTokensCount: absent or invalid values render no badge', () => {
  assert.equal(formatTokensCount(null), '');
  assert.equal(formatTokensCount(), '');
  assert.equal(formatTokensCount(0), '');
  assert.equal(formatTokensCount(-5), '');
  assert.equal(formatTokensCount('not-a-number'), '');
  assert.equal(formatTokensCount(Number.NaN), '');
});
test('formatTokensCount: tolerates string digits from loose backends', () => {
  assert.equal(formatTokensCount('8192'), '8K');
  assert.equal(formatTokensCount('32768'), '33K'); // 1000 基四舍五入,与 monitor 同口径
});

// --- factSourceLabelKey(来源 tag → i18n 键) ---
test('factSourceLabelKey maps endpoint/catalog, rejects unknown', () => {
  assert.equal(factSourceLabelKey('endpoint'), 'factSourceEndpoint');
  assert.equal(factSourceLabelKey('catalog'), 'factSourceCatalog');
  assert.equal(factSourceLabelKey(''), null);
  assert.equal(factSourceLabelKey(), null);
  assert.equal(factSourceLabelKey('server'), null);
});

// --- modelDuplicationKey / savedModelKeys(model id + base_url 去重键) ---
test('modelDuplicationKey is id-case-insensitive and trailing-slash-insensitive', () => {
  assert.equal(modelDuplicationKey('GLM-5', 'https://api.example.com/v1'), modelDuplicationKey('glm-5', 'https://api.example.com/v1/'));
  assert.notEqual(modelDuplicationKey('glm-5', 'https://a.com/v1'), modelDuplicationKey('glm-5', 'https://b.com/v1'));
  assert.notEqual(modelDuplicationKey('glm-5', 'https://a.com/v1'), modelDuplicationKey('glm-6', 'https://a.com/v1'));
});
test('savedModelKeys reads SavedModel wire ids and tolerates junk', () => {
  const keys = [...savedModelKeys([
    { id: 'm1', model: 'DeepSeek-V4', base_url: 'https://api.deepseek.com' },
    { id: 'm2', model: 'qwen', base_url: null },
    { id: 'm3', model: '', base_url: 'https://x' }, // 空 wire id → 回退记录 id
    { id: '', model: '', base_url: 'https://y' }, // 全空 → 跳过
    null,
  ])].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  assert.deepEqual(keys, [
    modelDuplicationKey('deepseek-v4', 'https://api.deepseek.com'),
    modelDuplicationKey('qwen', ''),
    modelDuplicationKey('m3', 'https://x'),
  ].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0)));
  assert.deepEqual([...savedModelKeys(null)], []);
  assert.deepEqual([...savedModelKeys('nope')], []);
});

// --- normalizeDiscoveredRows(EndpointModelsListing → 渲染行) ---
test('normalizeDiscoveredRows: maps snake_case facts and clamps invalid values', () => {
  const rows = toHost(normalizeDiscoveredRows({
    status: 'listed',
    models: [
      { id: 'glm-5', title: ' GLM-5 Flagship ', context_window_tokens: 1048576, context_from: 'endpoint', max_output_tokens: 32768, output_from: 'catalog', loaded: false },
      { id: 'm2', context_window_tokens: -3, max_output_tokens: 'oops', context_from: 'server', loaded: true },
    ],
  }));
  assert.equal(rows.length, 2);
  assert.deepEqual(
    rows[0],
    { key: 'glm-5:0', id: 'glm-5', title: 'GLM-5 Flagship', contextWindowTokens: 1048576, contextFrom: 'endpoint', maxOutputTokens: 32768, outputFrom: 'catalog', loaded: false },
  );
  assert.equal(rows[1].contextWindowTokens, null);
  assert.equal(rows[1].maxOutputTokens, null);
  assert.equal(rows[1].contextFrom, '');
  assert.equal(rows[1].outputFrom, '');
  assert.equal(rows[1].loaded, true);
});
test('normalizeDiscoveredRows: tolerates missing listing and legacy string entries', () => {
  assert.deepEqual(toHost(normalizeDiscoveredRows(null)), []);
  assert.deepEqual(toHost(normalizeDiscoveredRows({ status: 'listed' })), []);
  const rows = toHost(normalizeDiscoveredRows({ models: ['bare-id', { id: '  ' }, { id: 'kept' }] }));
  assert.deepEqual(rows.map(row => row.id), ['bare-id', 'kept']);
});

// --- filterModelRows(>15 条过滤框逻辑) ---
test('filterModelRows: empty query returns all rows, blank spaces ignored', () => {
  const rows = [{ id: 'glm-5', title: '' }, { id: 'k3', title: 'K3 Long Context' }];
  assert.deepEqual(toHost(filterModelRows(rows, '')), rows);
  assert.deepEqual(toHost(filterModelRows(rows, '   ')), rows);
  assert.deepEqual(toHost(filterModelRows(null, '')), []);
});
test('filterModelRows: case-insensitive substring over id and title', () => {
  const rows = [{ id: 'glm-5', title: '' }, { id: 'k3', title: 'K3 Long Context' }];
  assert.deepEqual(toHost(filterModelRows(rows, 'GLM')).map(r => r.id), ['glm-5']);
  assert.deepEqual(toHost(filterModelRows(rows, 'long context')).map(r => r.id), ['k3']);
  assert.deepEqual(toHost(filterModelRows(rows, 'zzz')), []);
});

// --- splitBatchAdd(批量「全部添加」:去重 + 截断到 100) ---
test('splitBatchAdd: dedupes against saved models and within the batch', () => {
  const baseUrl = 'https://api.example.com/v1';
  const saved = savedModelKeys([{ model: 'dup-a', base_url: baseUrl }]);
  const rows = [
    { id: 'dup-a' },
    { id: 'fresh-1' },
    { id: 'fresh-2' },
    { id: 'fresh-1' }, // 批内重复
  ];
  const result = splitBatchAdd(rows, saved, baseUrl);
  assert.deepEqual(toHost(result.addable).map(row => row.id), ['fresh-1', 'fresh-2']);
  assert.equal(result.truncated, false);
  assert.equal(result.duplicates, 2);
});
test('splitBatchAdd: caps the batch at the limit and reports truncation', () => {
  const rows = Array.from({ length: ADD_ALL_BATCH_LIMIT + 37 }, (_, i) => ({ id: `m-${i}` }));
  const result = splitBatchAdd(rows, new Set(), 'https://x/v1');
  assert.equal(toHost(result.addable).length, ADD_ALL_BATCH_LIMIT);
  assert.equal(toHost(result.addable)[0].id, 'm-0');
  assert.equal(result.truncated, true);
});
test('splitBatchAdd: everything already saved yields an empty batch', () => {
  const baseUrl = 'https://x/v1';
  const saved = savedModelKeys([{ model: 'a', base_url: baseUrl }]);
  const result = splitBatchAdd([{ id: 'a' }], saved, baseUrl);
  assert.deepEqual(toHost(result.addable), []);
  assert.equal(result.truncated, false);
});
test('splitBatchAdd: tolerates non-set saved keys and junk rows', () => {
  const result = splitBatchAdd([null, { id: 'a' }], undefined, 'https://x/v1');
  assert.deepEqual(toHost(result.addable).map(row => row.id), ['a']);
});

// --- 常量冻结(规格 §2.6:>15 条过滤、>100 截断) ---
test('picker thresholds stay pinned', () => {
  assert.equal(FILTER_VISIBLE_ROW_LIMIT, 15);
  assert.equal(ADD_ALL_BATCH_LIMIT, 100);
});

// --- ACP 线复用(规格 §2.7):组件缺省 props 的「无已保存」语义 ---
// 组件 JSX 无法在 vm 里求值,这里按源码钉死缺省分支:savedKeys 非 Set(ACP 线
// 不传)→ savedSet=null → 行恒可添加、无「已保存」标记;onAddAll 缺省(ACP 线
// 不传)→ 头部不渲染批量按钮。聊天线(传 savedKeys/onAddAll)行为不受影响。
test('component treats non-Set savedKeys as "nothing saved" and onAddAll as optional', () => {
  const componentSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'settings', 'endpoint-model-picker.jsx'),
    'utf8',
  );
  assert.match(
    componentSource,
    /savedKeys instanceof Set \? savedKeys : null/,
    'non-Set savedKeys (ACP line omits the prop) must degrade to "no saved models", not crash',
  );
  assert.match(
    componentSource,
    /const addableCount = savedSet\s*\?\s*rows\.filter\(\s*row => !savedSet\.has\(dupKeyOf\(row\)\)\)\.length\s*:\s*rows\.length/,
    'with no saved set every row must stay addable (ACP line has no saved-model dedup)',
  );
  assert.match(
    componentSource,
    /\{onAddAll && \(\s*<button type="button" data-testid="endpoint-model-add-all"/,
    'the batch add-all button must render only when onAddAll is provided (ACP line omits it)',
  );
});
