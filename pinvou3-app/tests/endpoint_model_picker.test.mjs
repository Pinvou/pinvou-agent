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
test('formatTokensCount: the K→M rounding boundary folds to 1M, never 1000K (round-3 review minor)', () => {
  assert.equal(formatTokensCount(999499), '999K', 'below the rounding midpoint stays K');
  assert.equal(formatTokensCount(999500), '1M', '999500/1000 rounds to 1000 kilos → 1M');
  assert.equal(formatTokensCount(999999), '1M', 'the top of the K band must fold to 1M');
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
test('modelDuplicationKey folds base URL case too (round-1 review minor)', () => {
  assert.equal(modelDuplicationKey('glm-5', 'HTTPS://API.Example.COM/v1'), modelDuplicationKey('glm-5', 'https://api.example.com/v1'));
  assert.equal(modelDuplicationKey('glm-5', 'https://a.com/V1/'), modelDuplicationKey('glm-5', 'https://a.com/v1'));
  // 大小写折叠不得吞掉真正不同的端点。
  assert.notEqual(modelDuplicationKey('glm-5', 'https://a.com/v1'), modelDuplicationKey('glm-5', 'https://a.com/v2'));
});
test('modelDuplicationKey folds the MiniMax legacy origin exactly like the Rust migration (r13 review P2)', () => {
  // migrated_minimax_base_url: api.minimax.chat → api.minimaxi.com(精确源,
  // 或 legacy 前缀 + '/')。表单里的旧域名必须与迁移后的落库记录共享一个
  // 去重键——否则第二次旧域名保存通过守卫、落成重复对(记录 id 不同,
  // 原生按 id 原地插入)。
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.minimax.chat/v1'),
    modelDuplicationKey('glm-5', 'https://api.minimaxi.com/v1'),
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.minimax.chat'),
    modelDuplicationKey('glm-5', 'https://api.minimaxi.com'),
  );
  // 大小写无关地命中迁移(Rust 是 ASCII 小写比较)。
  assert.equal(
    modelDuplicationKey('glm-5', 'HTTPS://API.MINIMAX.CHAT/v1'),
    modelDuplicationKey('glm-5', 'https://api.minimaxi.com/v1'),
  );
  // 前缀非边界不得迁移:同源更长主机名是另一个端点。
  assert.notEqual(
    modelDuplicationKey('glm-5', 'https://api.minimax.chat.example.com/v1'),
    modelDuplicationKey('glm-5', 'https://api.minimaxi.com/v1'),
  );
  // 其他域名不受影响。
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
  );
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
test('splitBatchAdd: the truncation flag flips exactly past the limit (round-5 m1: the >= mutant survives without this boundary)', () => {
  const atLimit = splitBatchAdd(Array.from({ length: ADD_ALL_BATCH_LIMIT }, (_, i) => ({ id: `m-${i}` })), new Set(), 'https://x/v1');
  assert.equal(toHost(atLimit.addable).length, ADD_ALL_BATCH_LIMIT);
  assert.equal(atLimit.truncated, false, 'exactly at the limit the list is complete, not truncated');
  const onePast = splitBatchAdd(Array.from({ length: ADD_ALL_BATCH_LIMIT + 1 }, (_, i) => ({ id: `m-${i}` })), new Set(), 'https://x/v1');
  assert.equal(toHost(onePast.addable).length, ADD_ALL_BATCH_LIMIT);
  assert.equal(onePast.truncated === true, true, 'one valid entry past the limit must set truncated');
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
    /if \(!savedSet\) return uniqueKeys\.size;/,
    'with no saved set every (unique) row must stay addable (ACP line has no saved-model dedup)',
  );
  assert.match(
    componentSource,
    /\{onAddAll && \(\s*<button type="button" data-testid="endpoint-model-add-all"/,
    'the batch add-all button must render only when onAddAll is provided (ACP line omits it)',
  );
});

// --- 复审 M7:行管线 memo 化 + 渲染上限(200 行)---
test('rows pipeline is memoized and the render is capped with an "N more" footer', () => {
  const componentSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'settings', 'endpoint-model-picker.jsx'),
    'utf8',
  );
  assert.match(
    componentSource,
    /const PICKER_RENDER_CAP = 200;/,
    'the render cap constant must stay pinned at 200 rows',
  );
  assert.match(
    componentSource,
    /useMemo\(\(\) => normalizeDiscoveredRows\(listing\), \[listing\]\)/,
    'normalizeDiscoveredRows must run in a useMemo keyed on the listing (not per keystroke render)',
  );
  assert.match(
    componentSource,
    /useMemo\(\s*\(\) => \(showFilter \? filterModelRows\(rows, filterDraft\) : rows\),\s*\[rows, showFilter, filterDraft\],\s*\)/,
    'filterModelRows must run in a useMemo keyed on [rows, showFilter, filterDraft]',
  );
  assert.match(
    componentSource,
    /visibleRows\.slice\(0, PICKER_RENDER_CAP\)/,
    'the DOM render must cap at PICKER_RENDER_CAP rows',
  );
  assert.match(
    componentSource,
    /const addableCount = useMemo\(\(\) => \{[\s\S]*?const uniqueKeys = new Set\(\);[\s\S]*?for \(const row of rows\) uniqueKeys\.add\(modelDuplicationKey\(row\.id, baseUrl\)\);[\s\S]*?if \(!savedSet\) return uniqueKeys\.size;[\s\S]*?\}, \[rows, savedSet, baseUrl\]\);/,
    'the addable count must be memoized on [rows, savedSet, baseUrl] and counted over UNIQUE duplication keys — a per-row count advertised in-listing duplicates the confirmed batch collapses (round-3 review minor + round-4 review M4 follow-up)',
  );
  assert.match(
    componentSource,
    /settingsCopy\.modelListMoreCount\(hiddenRowCount\)/,
    'a filtered footer must surface as the localized "N more — refine the filter" line',
  );
  assert.match(
    componentSource,
    /settingsCopy\.modelListMoreCountPlain\(hiddenRowCount\)/,
    'an untouched filter must get the plain "N more" footer (the refine advice presupposes a filter in use, round-3 review minor)',
  );
  assert.match(
    componentSource,
    /settingsCopy\.modelListFilterEmpty/,
    'the filtered-empty branch must use the dedicated "no models match this filter" copy (not noModelsFound)',
  );
  assert.match(
    componentSource,
    /settingsCopy\.modelListTruncated/,
    'the listing truncated flag must surface as a localized notice line',
  );
  // round-4 FE polish: the dialog labels itself with its visible title so
  // screen readers announce the heading instead of an anonymous dialog.
  assert.match(
    componentSource,
    /data-testid="endpoint-model-picker-dialog" role="dialog" aria-modal="true" aria-labelledby="endpoint-model-picker-title"/,
    'the picker dialog must reference its title heading via aria-labelledby',
  );
  assert.match(
    componentSource,
    /<h2 id="endpoint-model-picker-title"/,
    'the title heading must carry the id the dialog labels itself with',
  );
  assert.match(
    componentSource,
    /disabled=\{loading \|\| pickBusy \|\| addableCount === 0\}/,
    'the batch Add All button must also disable while a single-row save is in flight (a batch snapshot taken then would double-save that row, round-4 review M4)',
  );
});

// --- 复审 minor-8:错误状态嗓音映射(3xx/tls/network 复用连接测试文案) ---
test('error status maps 3xx redirects and tls/network transports onto the connectionMessages voices', () => {
  const componentSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'settings', 'endpoint-model-picker.jsx'),
    'utf8',
  );
  assert.match(
    componentSource,
    /httpStatus >= 300 && httpStatus < 400\s*\?\s*connectionCopy\.redirect/,
    'a 3xx error listing must reuse the connection-test redirect voice (服务地址发生跳转), not the http_error fallback',
  );
  assert.match(
    componentSource,
    /network: connectionCopy\.unknown/,
    'generic/mid-stream network failures must stop reading as the HTTP-error voice (http_status is null there)',
  );
  assert.match(
    componentSource,
    /tls: connectionCopy\.tls_error/,
    'the Rust core tls transport must map onto the tls_error voice',
  );
  assert.match(
    componentSource,
    /connect: connectionCopy\.connection_refused/,
    'the existing timeout/dns/connect arms stay pinned alongside the new ones',
  );
});

// --- 复审 minor-15/M4:徽章 key 带 role 前缀 + addNotice 横幅 ---
test('fact badges key with a role prefix and the picker renders the addNotice banner', () => {
  const componentSource = fs.readFileSync(
    path.join(root, 'src', 'features', 'settings', 'endpoint-model-picker.jsx'),
    'utf8',
  );
  assert.match(
    componentSource,
    /renderFactBadge\(row\.contextWindowTokens, row\.contextFrom, 'ctx'\)/,
    'the context badge must key with the ctx role prefix',
  );
  assert.match(
    componentSource,
    /renderFactBadge\(row\.maxOutputTokens, row\.outputFrom, 'out'\)/,
    'the output badge must key with the out role prefix (identical source+value pairs must not collide)',
  );
  assert.match(
    componentSource,
    /key=\{`\$\{role\}-\$\{source\}-\$\{text\}`\}/,
    'the badge React key must combine role + source + text',
  );
  assert.match(
    componentSource,
    /data-testid="endpoint-model-add-notice"/,
    'a passed addNotice must render as an in-panel banner between the filter and the rows',
  );
  assert.match(
    componentSource,
    /\{addNotice && \(\s*(?:\/\/[^\n]*\n\s*)*<div data-testid="endpoint-model-add-notice"/,
    'the banner renders only when the parent passes a non-null notice (optional prop, ACP line omits it)',
  );
  assert.match(
    componentSource,
    /data-testid="endpoint-model-add-notice" role="status" aria-live="polite"/,
    'the async add result must be announced politely by screen readers without stealing focus (round-4 FE polish)',
  );
});

// --- 复审 minor-17:Ollama 裸名 :latest 折叠(与 Rust ollama_canonical_name 同口径) ---
test('modelDuplicationKey folds exactly one trailing :latest (saved llama3 vs discovery llama3:latest)', () => {
  assert.equal(
    modelDuplicationKey('llama3', 'https://x/v1'),
    modelDuplicationKey('llama3:latest', 'https://x/v1'),
    'a saved bare name and its Ollama-canonical :latest form must dedup as one model',
  );
  assert.equal(
    modelDuplicationKey('LLAMA3', 'https://x/v1'),
    modelDuplicationKey('llama3:latest', 'https://x/v1'),
    'the fold composes with the existing case-insensitive id fold',
  );
  // 只折一层 :latest:其余 tag 是不同模型,绝不折叠。
  assert.notEqual(
    modelDuplicationKey('llama3:latest:latest', 'https://x/v1'),
    modelDuplicationKey('llama3:latest', 'https://x/v1'),
  );
  assert.notEqual(
    modelDuplicationKey('llama3:v1', 'https://x/v1'),
    modelDuplicationKey('llama3', 'https://x/v1'),
    'a different tag is a different model; only :latest folds',
  );
  assert.notEqual(
    modelDuplicationKey('foo:latest', 'https://x/v1'),
    modelDuplicationKey('bar:latest', 'https://x/v1'),
    'the fold must not collapse distinct ids that both carry :latest',
  );
});
// URL canonical fold (review P2: full-URL → canonical list → delete → re-add):
// the Rust persistence arm (strip_chat_completions_suffix, prefs/model.rs)
// rewrites a /chat/completions-suffixed URL to its base before the record is
// ever listed, so the dedup key must fold the same suffix — a booking made
// from the raw form URL has to hash to the same identity as its own canonical
// listed record, or the settled-book retirement and the saved-keys guard
// never match and the identity stays blocked for the whole session.
test('modelDuplicationKey strips the /chat/completions suffix exactly like the Rust persistence arm', () => {
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
    'the raw form URL and its canonical persisted record must share one key',
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions/'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
    'trailing slash before the suffix folds first, mirroring trim_url_tail',
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions///'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
    'all trailing slashes fold before the suffix check (trim_end_matches)',
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/Chat/Completions'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1'),
    'the suffix matches case-insensitively (to_ascii_lowercase probe)',
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://host/chat/completions'),
    modelDuplicationKey('glm-5', 'https://host'),
    'a suffix that is the whole path collapses to the origin',
  );
  assert.equal(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions'),
    modelDuplicationKey('GLM-5', 'https://API.example.COM/v1/'),
    'the suffix fold composes with the existing case/slash folds',
  );
  assert.notEqual(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v2'),
    'the strip must not collapse genuinely different endpoints',
  );
  assert.notEqual(
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions/x'),
    modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions'),
    'only a trailing suffix strips; a deeper path is a different endpoint',
  );
  assert.equal(
    savedModelKeys([{ model: 'glm-5', base_url: 'https://api.example.com/v1' }]).has(modelDuplicationKey('glm-5', 'https://api.example.com/v1/chat/completions')),
    true,
    'savedModelKeys hashes canonical records into the same key space as raw form input',
  );
});
test('splitBatchAdd treats saved llama3 vs discovery llama3:latest as already saved', () => {
  const baseUrl = 'https://x/v1';
  const saved = savedModelKeys([{ model: 'llama3', base_url: baseUrl }]);
  const result = splitBatchAdd([{ id: 'llama3:latest' }, { id: 'llama3' }], saved, baseUrl);
  assert.deepEqual(toHost(result.addable), [], 'both spellings must dedup against the saved bare name');
  assert.equal(result.duplicates, 2);
});
