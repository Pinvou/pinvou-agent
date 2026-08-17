import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { renderMarkdownMarkup } from '../src/shared/markdown-renderer.js';
import {
  highlightCode,
  highlightDiffCode,
  MAX_HIGHLIGHT_SOURCE_BYTES,
  normalizeSyntaxLanguage,
  supportedSyntaxLanguages,
} from '../src/shared/syntax-highlighter.js';

const testRoot = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(testRoot, '..');
const readApp = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

assert.ok(
  supportedSyntaxLanguages.length >= 75,
  `expected broad syntax coverage, found ${supportedSyntaxLanguages.length} languages`,
);
for (const language of [
  'javascript', 'typescript', 'python', 'java', 'c', 'cpp', 'csharp', 'go', 'rust',
  'kotlin', 'swift', 'dart', 'php', 'ruby', 'bash', 'powershell', 'sql', 'json',
  'yaml', 'ini', 'dockerfile', 'cmake', 'nginx', 'haskell', 'elixir', 'r', 'julia',
  'x86asm', 'wasm', 'xml', 'css', 'markdown', 'diff',
  'log',
]) {
  assert.ok(supportedSyntaxLanguages.includes(language), `missing syntax language: ${language}`);
}

for (const [alias, canonical] of [
  ['js', 'javascript'], ['jsx', 'javascript'], ['tsx', 'typescript'], ['py', 'python'],
  ['cs', 'csharp'], ['c++', 'cpp'], ['ps1', 'powershell'], ['yml', 'yaml'],
  ['toml', 'ini'], ['vue', 'xml'], ['svelte', 'xml'], ['html', 'xml'], ['logs', 'log'],
]) {
  assert.equal(normalizeSyntaxLanguage(alias), canonical, `${alias} alias must resolve to ${canonical}`);
}

const python = renderMarkdownMarkup([
  '```python',
  'def heap_sort(arr):',
  '    return sorted(arr)',
  '```',
].join('\n'));
assert.match(python, /class="pinvou-code-block" data-language="Python"/u);
assert.match(python, /data-language-id="python"/u);
assert.match(python, /class="hljs language-python"/u);
assert.match(python, /hljs-keyword">def</u);
assert.match(python, /hljs-title function_">heap_sort</u);

const commonMarkFence = renderMarkdownMarkup('  ```python\r\n  def greet():\r\n    return True\r\n  ````   ');
assert.match(commonMarkFence, /data-language-id="python"/u);
assert.match(commonMarkFence, /hljs-keyword">def</u);

const tsx = renderMarkdownMarkup('```tsx\nconst App = () => <main>Hello</main>;\n```');
assert.match(tsx, /data-language="TSX"/u);
assert.match(tsx, /data-language-id="tsx"/u);
assert.match(tsx, /class="hljs language-typescript"/u);
assert.match(tsx, /hljs-keyword">const</u);

const html = renderMarkdownMarkup('```html\n<main><script>const ready = true;</script><style>.app { color: red; }</style></main>\n```');
assert.match(html, /data-language-id="html"/u);
assert.match(html, /class="language-javascript"/u);
assert.match(html, /class="language-css"/u);

const vue = renderMarkdownMarkup('```vue\n<UserCard v-if="user" :name="user.name">{{ user.name }}</UserCard>\n```');
assert.match(vue, /data-language-id="vue"/u);
assert.match(vue, /class="hljs language-xml"/u);
assert.match(vue, /hljs-name">UserCard</u);
assert.match(vue, /hljs-attr">v-if</u);

const diff = renderMarkdownMarkup('```diff\n-oldValue\n+newValue\n```');
assert.match(diff, /hljs-deletion">-oldValue</u);
assert.match(diff, /hljs-addition">\+newValue</u);

const json = renderMarkdownMarkup('```json\n{"enabled": true}\n```');
assert.match(json, /hljs-attr">&quot;enabled&quot;</u);
assert.match(json, /hljs-punctuation">\{/u);

const sql = renderMarkdownMarkup('```sql\nSELECT COUNT\(\*\) FROM users WHERE id = :id;\n```');
assert.match(sql, /hljs-keyword">SELECT</u);
assert.match(sql, /hljs-built_in">COUNT</u);

const autoDetected = renderMarkdownMarkup('```\ndef greet(name):\n    return f"Hello {name}"\n```');
assert.match(autoDetected, /data-language="Python"/u);
assert.match(autoDetected, /hljs-keyword">def</u);

const incompleteStream = renderMarkdownMarkup('```\ndef greet(name):\n    return name');
assert.match(incompleteStream, /data-language="Text"/u);
assert.doesNotMatch(incompleteStream, /hljs-keyword/u);

const incompleteExplicitStream = renderMarkdownMarkup('```python\ndef greet(name):\n    return name');
assert.match(incompleteExplicitStream, /data-language="Python"/u);
assert.match(incompleteExplicitStream, /data-language-id="plaintext"/u);
assert.match(incompleteExplicitStream, /class="hljs language-plaintext"/u);
assert.doesNotMatch(incompleteExplicitStream, /hljs-keyword/u);

const genericLog = renderMarkdownMarkup([
  '```log',
  '2026-07-30T10:00:00+08:00 INFO GET /api/users status=200',
  '2026-07-30T10:00:01+08:00 WARN retry status=429',
  '2026-07-30T10:00:02+08:00 ERROR request failed status=500',
  '```',
].join('\n'));
assert.match(genericLog, /data-language="Log"/u);
assert.match(genericLog, /class="hljs language-log"/u);
assert.match(genericLog, /hljs-built_in">INFO</u);
assert.match(genericLog, /hljs-warning">WARN</u);
assert.match(genericLog, /hljs-deletion">ERROR</u);
assert.match(genericLog, /hljs-keyword">GET</u);
assert.match(genericLog, /hljs-string">\/api\/users</u);
assert.match(genericLog, /hljs-attr">status</u);

const unsupported = renderMarkdownMarkup('```unknown-lang\n<script>alert("x")</script>\n```');
assert.match(unsupported, /class="hljs language-plaintext"/u);
assert.match(unsupported, /&lt;script&gt;/u);
assert.doesNotMatch(unsupported, /<script>/iu);

const oversizedJavaScript = highlightCode(
  `const value = 1;\n${'x'.repeat(MAX_HIGHLIGHT_SOURCE_BYTES)}`,
  'javascript',
);
assert.equal(oversizedJavaScript.language, 'plaintext');
assert.equal(oversizedJavaScript.languageId, 'plaintext');
assert.equal(oversizedJavaScript.label, 'JavaScript');
assert.equal(oversizedJavaScript.highlighted, false);
assert.equal(oversizedJavaScript.oversized, true);
assert.doesNotMatch(oversizedJavaScript.html, /hljs-keyword/u);

const oversizedUtf8 = highlightCode(
  '中'.repeat(Math.ceil(MAX_HIGHLIGHT_SOURCE_BYTES / 3) + 1),
  'python',
);
assert.equal(oversizedUtf8.language, 'plaintext');
assert.equal(oversizedUtf8.oversized, true);

// diff 视图走逐行前缀着色（highlightDiffCode），刻意绕过 MAX_HIGHLIGHT_SOURCE_BYTES：
// 超过普通回落阈值的 diff 仍必须保留行级高亮（+/-/@@/文件头），否则该优化路径会静默
// 退化为无高亮纯文本。此处对照上面的 oversizedJavaScript（回退 plaintext）验证差异。
const oversizedDiff = highlightDiffCode(
  `+${'a'.repeat(MAX_HIGHLIGHT_SOURCE_BYTES)}\n-${'b'.repeat(MAX_HIGHLIGHT_SOURCE_BYTES)}\n`,
);
assert.equal(oversizedDiff.highlighted, true);
assert.equal(oversizedDiff.language, 'diff');
assert.match(oversizedDiff.html, /hljs-addition">/u);
assert.match(oversizedDiff.html, /hljs-deletion">/u);
assert.equal(oversizedDiff.oversized, undefined);

const cachedSource = 'const cacheProbe = () => 42;';
const cachedFirst = highlightCode(cachedSource, 'javascript');
const cachedSecond = highlightCode(cachedSource, 'javascript');
assert.strictEqual(cachedSecond, cachedFirst, 'completed code highlight should use the bounded cache');
for (let index = 1; index <= 24; index += 1) {
  highlightCode(`const boundedCacheProbe${index} = ${index};`, 'javascript');
}
const cachedAfterEviction = highlightCode(cachedSource, 'javascript');
assert.notStrictEqual(cachedAfterEviction, cachedFirst, 'highlight cache must evict its least-recently-used entry');

// ---- 危险标签抹平边界(原 render_markdown.test.js 的独有价值,迁移自该测试) ----
// 修法见 markdown-renderer.js 的 neutralizeRawDangerousTags:marked.parse 之后把
// script/style/iframe/object/embed/link/meta escape 成 &lt;...&gt;,防止裸标签吃掉
// 表格后续列或被浏览器当真 HTML 执行。

// 表格 cell 含裸 <script> 不能吃掉后续列(原始回归截图场景)。
const tableScriptCell = renderMarkdownMarkup([
  '| Finding | Severity | Status | User Decision |',
  '|---|---|---|---|',
  '| 两步写入的 JS 代码在同一个 <script> 标签内 | CRITICAL | RAISED | 待用户拍 |',
].join('\n'));
assert.match(tableScriptCell, /CRITICAL/u);
assert.match(tableScriptCell, /RAISED/u);
assert.match(tableScriptCell, /&lt;script/u);
assert.doesNotMatch(tableScriptCell, /<script>/iu);

// 反引号包的 `<script>` 不被双重转义(代码块内是字面量)。
const inlineCodeScript = renderMarkdownMarkup('`<script>` 标签');
assert.doesNotMatch(inlineCodeScript, /&amp;lt;/u);
assert.match(inlineCodeScript, /&lt;script&gt;/u);

// 裸 <iframe> 同样抹平。
const rawIframe = renderMarkdownMarkup('before <iframe src="evil"></iframe> after');
assert.match(rawIframe, /&lt;iframe/u);
assert.match(rawIframe, /&lt;\/iframe/u);
assert.doesNotMatch(rawIframe, /<iframe/iu);

// 合法的 <br> 与 marked 自己产出的结构标签(<h1>/<table>/<td>)不能被误伤。
assert.match(renderMarkdownMarkup('line1<br>line2'), /<br>/u);
const structured = renderMarkdownMarkup('# Title\n\n- bullet\n\n| a | b |\n|---|---|\n| 1 | 2 |');
assert.match(structured, /<h1[^>]*>/u);
assert.match(structured, /<table/u);
assert.match(structured, /<td>/u);

const dangerousRawHtml = renderMarkdownMarkup('before <script>alert("x")</script> after');
assert.match(dangerousRawHtml, /&lt;script&gt;/u);
assert.doesNotMatch(dangerousRawHtml, /<script>/iu);
// 大写变体同样必须抹平(生产 DANGEROUS_TAGS_RE 带 giu 标志)。
const uppercaseScript = renderMarkdownMarkup('before <SCRIPT>alert("x")</SCRIPT> after');
assert.match(uppercaseScript, /&lt;SCRIPT&gt;/u);
assert.doesNotMatch(uppercaseScript, /<SCRIPT>/iu);

const css = readApp('src', 'styles', 'base.css');
assert.match(css, /\.dark-code \.msg-md :not\(pre\) > code/u);
assert.match(css, /\.light-code \.msg-md :not\(pre\) > code/u);
assert.match(css, /\.msg-md\.dark-code pre > code/u);
assert.match(css, /\.msg-md\.light-code pre > code/u);
assert.match(css, /\.persona-body\.dark-code/u);
assert.match(css, /\.persona-body\.light-code/u);
assert.match(css, /\.msg-md pre\.pinvou-code-block::before/u);
assert.match(css, /\.pinvou-code-block \.hljs-keyword/u);
for (const selector of [
  'code.language-diff .hljs-addition',
  'code.language-json .hljs-punctuation',
  'code:is(.language-yaml,.language-ini) .hljs-attr',
  'code:is(.language-bash,.language-shell,.language-powershell) .hljs-built_in',
  'code:is(.language-sql,.language-pgsql) .hljs-keyword',
  'code.language-xml .hljs-name',
  'pre:is([data-language-id="vue"],[data-language-id="svelte"])',
  'code:is(.language-javascript,.language-typescript) .hljs-title.class_',
  'code.language-python .hljs-meta',
  'code:is(.language-java,.language-csharp,.language-kotlin) .hljs-meta',
  'code:is(.language-c,.language-cpp) .hljs-meta',
  'code:is(.language-go,.language-rust) .hljs-type',
  'code.language-markdown .hljs-quote',
  'code.language-dockerfile > .hljs-keyword',
  'code:is(.language-accesslog,.language-log) .hljs-number',
  'code.language-log .hljs-warning',
]) {
  assert.ok(css.includes(selector), `missing language-specific style: ${selector}`);
}

for (const bridgePath of [
  ['src', 'platform', 'tauri', 'bridge.js'],
  ['src', 'platform', 'web', 'bridge.js'],
]) {
  assert.match(
    readApp(...bridgePath),
    /window\.PinvouMarkdownRenderer\.renderMarkdown\(text\)/u,
    `${bridgePath.join('/')} must delegate to the shared renderer`,
  );
}
assert.match(
  readApp('src', 'features', 'conversation', 'ConversationTimeline.jsx'),
  /import \{ renderMarkdown \} from '\.\.\/\.\.\/shared\/markdown-renderer\.js'/u,
);
// Personas.jsx 代码块明暗切换采用静态共存写法 light-code dark-code
// （配合 base.css 的 .dark 祖先限定；P1 重构后 isDark 三元式已消除）。
assert.match(
  readApp('src', 'features', 'personas', 'Personas.jsx'),
  /light-code dark-code/u,
);
console.log('Markdown syntax highlighting contract: ok');
