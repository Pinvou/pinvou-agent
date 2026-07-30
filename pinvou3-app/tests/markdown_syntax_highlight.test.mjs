import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { renderMarkdownMarkup } from '../src/shared/markdown-renderer.js';
import {
  highlightCode,
  MAX_HIGHLIGHT_SOURCE_BYTES,
  normalizeSyntaxLanguage,
  supportedSyntaxLanguages,
} from '../src/shared/syntax-highlighter.js';

const testRoot = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.resolve(testRoot, '..');
const readApp = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

assert.ok(
  supportedSyntaxLanguages.length >= 70,
  `expected broad syntax coverage, found ${supportedSyntaxLanguages.length} languages`,
);
for (const language of [
  'javascript', 'typescript', 'python', 'java', 'c', 'cpp', 'csharp', 'go', 'rust',
  'kotlin', 'swift', 'dart', 'php', 'ruby', 'bash', 'powershell', 'sql', 'json',
  'yaml', 'ini', 'dockerfile', 'cmake', 'nginx', 'haskell', 'elixir', 'r', 'julia',
  'x86asm', 'wasm', 'xml', 'css', 'markdown', 'diff',
]) {
  assert.ok(supportedSyntaxLanguages.includes(language), `missing syntax language: ${language}`);
}

for (const [alias, canonical] of [
  ['js', 'javascript'], ['jsx', 'javascript'], ['tsx', 'typescript'], ['py', 'python'],
  ['cs', 'csharp'], ['c++', 'cpp'], ['ps1', 'powershell'], ['yml', 'yaml'],
  ['toml', 'ini'], ['vue', 'xml'], ['svelte', 'xml'], ['html', 'xml'],
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

const unsupported = renderMarkdownMarkup('```unknown-lang\n<script>alert("x")</script>\n```');
assert.match(unsupported, /class="hljs language-plaintext"/u);
assert.match(unsupported, /&lt;script&gt;/u);
assert.doesNotMatch(unsupported, /<script>/u);

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

const dangerousRawHtml = renderMarkdownMarkup('before <script>alert("x")</script> after');
assert.match(dangerousRawHtml, /&lt;script&gt;/u);
assert.doesNotMatch(dangerousRawHtml, /<script>/u);

const css = readApp('src', 'styles', 'base.css');
assert.match(css, /\.dark-code \.msg-md :not\(pre\) > code/u);
assert.match(css, /\.light-code \.msg-md :not\(pre\) > code/u);
assert.match(css, /\.dark-code \.msg-md pre > code \{ background:transparent; color:inherit; \}/u);
assert.match(css, /\.msg-md pre\.pinvou-code-block::before/u);
assert.match(css, /\.hljs-keyword/u);
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
  'code.language-accesslog .hljs-number',
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
console.log('Markdown syntax highlighting contract: ok');
