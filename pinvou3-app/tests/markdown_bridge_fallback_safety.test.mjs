// 验证 Markdown 渲染「最末级 fallback」在共享脚本缺失时仍转义 HTML（防 fail-open）。
//
// 背景：bridge.js（web/tauri）的 renderMarkdown 委托链为
//   1. window.PinvouMarkdownRenderer（npm 主包）
//   2. window.PinvouMarkdownBridgeFallback（shared/markdown-bridge-fallback.js，普通脚本）
//   3. 内联 escapeHtml（最末级）
// 第 3 级必须自带转义：远程 Web 部署缓存错配/资源缺失导致第 2 级脚本未加载时，
// renderMarkdown 的返回仍由 ChatView.jsx 的 dangerouslySetInnerHTML 消费，
// 原文返回会让 <img onerror=...> 等危险输入原样注入（fail-open）。
//
// 现有 render_markdown.test.js 把正则实现复制了一份，不执行真实 bridge 代码，
// 覆盖不到「共享脚本缺失」这条路径。本测试改为在 vm 里加载真实 web/bridge.js，
// 在「同时移除两个渲染器」的条件下断言危险输入被转义。
//
// 注：tauri/bridge.js 的 renderMarkdown 与 web 逐字相同（同一修复），但完整加载
// tauri bridge 需要大量 __PINVOU_TAURI_BRIDGE_FEATURES__ 注册 mock，成本不匹配；
// 故以 web bridge（评审指出的「远程 Web 部署」fail-open 向量）作为真实执行代表，
// 并对 shared 兜底文件单独做无 vendor 全局时的转义断言。
//
// 跑法：`node tests/markdown_bridge_fallback_safety.test.mjs`

import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const readApp = (...parts) => fs.readFileSync(path.join(root, ...parts), 'utf8');

// 复用 web_bridge_domain_contract.test.mjs 的最小 window mock：只够让 web/bridge.js
// 以「web 平台」身份完成加载并暴露 flat transport（含 renderMarkdown）。
function buildWebBridgeContext() {
  const storage = new Map();
  const localStorage = {
    getItem: key => (storage.has(key) ? storage.get(key) : null),
    setItem: (key, value) => storage.set(key, String(value)),
    removeItem: key => storage.delete(key),
  };
  const windowObject = {
    PinvouPlatform: {
      kind: 'web',
      isWeb: true,
      capabilities: {},
      can: () => false,
      canInvoke: () => false,
    },
    __TAURI__: {
      core: { invoke: async () => null },
      event: { listen: async () => () => {} },
      dialog: { open: async () => null },
    },
    location: { search: '', href: 'https://example.test/pinvou3/remote/' },
    localStorage,
    crypto: { randomUUID: () => '00000000-0000-8000-0000-000000000000' },
    performance: { now: () => 0 },
    addEventListener() {},
    setTimeout,
    clearTimeout,
  };
  const context = vm.createContext({
    window: windowObject,
    document: {
      readyState: 'loading',
      addEventListener() {},
      createElement: () => ({ click() {}, remove() {}, style: {}, setAttribute() {} }),
      body: { appendChild() {} },
    },
    navigator: { mediaDevices: null },
    localStorage,
    console,
    setTimeout,
    clearTimeout,
    setInterval,
    clearInterval,
    structuredClone,
    URL,
    URLSearchParams,
    Blob,
    Uint8Array,
    ArrayBuffer,
    TextEncoder,
    TextDecoder,
  });
  return { context, windowObject };
}

// ── Case 1: 真实 web/bridge.js，两个渲染器都不存在 → 最末级 fallback 必须转义 ──
{
  const { context, windowObject } = buildWebBridgeContext();
  // 刻意不安装 PinvouMarkdownRenderer / PinvouMarkdownBridgeFallback（模拟 markdown
  // 共享脚本缺失）。bridge-shared-helpers.js 是 bridge 的启动依赖（随 index.html 先于
  // 两份 bridge 加载），因此这里先装 payload 再装 bridge——与真实加载顺序一致。
  vm.runInContext(readApp('src', 'shared', 'bridge-shared-helpers.js'), context, {
    filename: 'shared/bridge-shared-helpers.js',
  });
  vm.runInContext(readApp('src', 'platform', 'web', 'bridge.js'), context, {
    filename: 'platform/web/bridge.js',
  });
  const renderMarkdown = windowObject.TauriBridge.renderMarkdown;
  assert.equal(typeof renderMarkdown, 'function', 'web bridge must expose renderMarkdown');

  const dangerous = '<img src=x onerror="alert(1)"><script>steal()</script>';
  const out = renderMarkdown(dangerous);
  // escapeHtml 把标签定界符转义成实体，整段变成无活性的可见文本——攻击向量随之失效。
  // 注意：escapeHtml 只转义 &<>"'，不删除属性名字母，故 "onerror" 字面仍会出现，
  // 但它已不在任何真实标签内（前面是 &lt;img ...），不再被浏览器当作事件处理器。
  assert.equal(out.indexOf('<img'), -1, 'raw <img must not survive the final fallback');
  assert.equal(out.indexOf('<script'), -1, 'raw <script must not survive the final fallback');
  assert.ok(out.includes('&lt;img'), `dangerous tags must be HTML-escaped, got: ${out}`);
  assert.ok(out.includes('&lt;script'), `script tags must be HTML-escaped, got: ${out}`);
  console.log('✓ web bridge escapes dangerous HTML when both renderers are absent');
}

// ── Case 2: 真实 web/bridge.js，只装了共享兜底且 vendor 全在 → 危险标签被中和 ──
// 验证委托链第 2 级（shared/markdown-bridge-fallback.js）在正常加载时仍工作，
// 且 bridge 确实把控制权交给了它（而非错误地停在 escapeHtml）。
{
  const { context, windowObject } = buildWebBridgeContext();
  // 构造最小 vendor 全局：marked 只做透传，DOMPurify 只做基本 sanitize。
  // 目的不是复刻真实 vendor 行为，而是让兜底文件走 neutralize + sanitize 分支。
  windowObject.marked = {
    parse: src => String(src || ''),
    setOptions() {},
  };
  windowObject.DOMPurify = { sanitize: html => String(html || '') };
  vm.runInContext(readApp('src', 'shared', 'markdown-bridge-fallback.js'), context, {
    filename: 'shared/markdown-bridge-fallback.js',
  });
  vm.runInContext(readApp('src', 'shared', 'bridge-shared-helpers.js'), context, {
    filename: 'shared/bridge-shared-helpers.js',
  });
  vm.runInContext(readApp('src', 'platform', 'web', 'bridge.js'), context, {
    filename: 'platform/web/bridge.js',
  });
  const out = windowObject.TauriBridge.renderMarkdown('before <script>x()</script> after');
  assert.ok(
    out.includes('&lt;script') && !out.includes('<script'),
    `shared fallback must neutralize raw <script>, got: ${out}`,
  );
  // u 旗标回归：`i`+`u` 的 Unicode 简单折叠（U+017F ſ→s）必须命中。此前兜底副本
  // 漏了 u 旗标，<ſcript> 这类折叠形态在兜底路径原样穿过 neutralize。
  const outFolded = windowObject.TauriBridge.renderMarkdown('x <ſcript>evil()</ſcript> y');
  assert.ok(
    outFolded.includes('&lt;ſcript') && !outFolded.includes('<ſcript'),
    `shared fallback must neutralize unicode-folded <ſcript> (needs u flag), got: ${outFolded}`,
  );
  console.log('✓ web bridge delegates to shared fallback to neutralize dangerous tags');
}

// ── Case 3: shared/markdown-bridge-fallback.js 单独加载，vendor 全局缺失 → 转义 ──
{
  const { context, windowObject } = buildWebBridgeContext();
  // 不装 marked / DOMPurify：兜底文件自身必须走 escapeHtml 分支。
  vm.runInContext(readApp('src', 'shared', 'markdown-bridge-fallback.js'), context, {
    filename: 'shared/markdown-bridge-fallback.js',
  });
  const out = windowObject.PinvouMarkdownBridgeFallback.renderMarkdown(
    '<iframe src=evil></iframe>',
  );
  assert.equal(out.indexOf('<iframe'), -1, 'raw <iframe must not survive missing vendors');
  assert.ok(out.includes('&lt;iframe'), `iframe must be escaped, got: ${out}`);
  console.log('✓ shared fallback escapes HTML when vendor globals are missing');
}

// ── Case 4: 守护渲染入口——最末级 fallback 不得退化为原文返回 ──
// 防止未来重构再次把 escapeHtml 抽走、留下 fail-open 的 `return String(text || "")`。
// Wave 6 后 renderMarkdown/escapeHtml 只在 shared/bridge-shared-helpers.js 定义
// （安全原语单一实现），两条 lane 仅保留 cluster 转发——分别守护。
const sharedRenderSource = readApp('src', 'shared', 'bridge-shared-helpers.js');
assert.match(
  sharedRenderSource,
  /return escapeHtml\(text \|\| ""\)/u,
  'shared renderMarkdown final fallback must escape (not return raw text)',
);
assert.match(
  sharedRenderSource,
  /function escapeHtml\(s\)/u,
  'shared helpers must define escapeHtml for the final fallback',
);
assert.match(
  sharedRenderSource,
  /window\.PinvouMarkdownRenderer\.renderMarkdown\(text\)/u,
  'shared renderMarkdown must keep delegating to the vendor renderer',
);
for (const bridgeRel of [['src', 'platform', 'web', 'bridge.js'], ['src', 'platform', 'tauri', 'bridge.js']]) {
  const src = readApp(...bridgeRel);
  assert.match(
    src,
    /function renderMarkdown\(text\) \{ return pinvouShared/u,
    `${bridgeRel.join('/')} must keep forwarding renderMarkdown to the shared cluster`,
  );
  console.log(`✓ ${bridgeRel.join('/')} forwards renderMarkdown to the shared cluster`);
}

// ── Case 5: 两份 sanitize 危险标签词表（DANGEROUS_TAGS_RE）必须逐字节同构 ──
// markdown-renderer.js（npm 主车道，ESM）与 markdown-bridge-fallback.js（bridge 兜底
// 车道，普通 script 无法 import ESM）各持一份 DANGEROUS_TAGS_RE，本应逐字镜像。
// 此前兜底副本漏了 u 旗标：`i`+`u` 的 Unicode 简单折叠（U+017F ſ→s、U+212A K→k）
// 在兜底路径失效，<ſcript> 等折叠形态不被中和而主车道正常。这里读两文件源文本，
// 断言正则字面量（含旗标）逐字节一致，防止再次单边漂移。
{
  const rendererSource = readApp('src', 'shared', 'markdown-renderer.js');
  const fallbackSource = readApp('src', 'shared', 'markdown-bridge-fallback.js');
  const extractWordlist = source =>
    source.match(/^[ \t]*const DANGEROUS_TAGS_RE = (\/.+\/[a-z]+);$/m)?.[1];
  const rendererLiteral = extractWordlist(rendererSource);
  const fallbackLiteral = extractWordlist(fallbackSource);
  assert.ok(rendererLiteral, 'markdown-renderer.js must declare DANGEROUS_TAGS_RE');
  assert.ok(fallbackLiteral, 'markdown-bridge-fallback.js must declare DANGEROUS_TAGS_RE');
  assert.equal(
    fallbackLiteral,
    rendererLiteral,
    'DANGEROUS_TAGS_RE must stay byte-identical across renderer and bridge fallback (flags included)',
  );
  // 显式钉住 u 旗标：字节同构已隐含它，但让回归的失败原因一目了然。
  assert.match(
    rendererLiteral,
    /\/[a-z]*u$/,
    'DANGEROUS_TAGS_RE must keep the u flag (unicode case folds: <ſcript>, <linK(U+212A)>…)',
  );
  // 行为抽查：iu 折叠形态必须命中（这正是 gi 副本当初漏掉的向量）。
  // 用 match 而非 test：词表正则带 g 旗标，test 会跨断言携带 lastIndex。
  const [, body, flags] = rendererLiteral.match(/^\/(.+)\/([a-z]+)$/);
  const wordlistRe = new RegExp(body, flags);
  assert.ok('<ſcript>alert(1)</ſcript>'.match(wordlistRe), 'unicode-folded <ſcript> must match');
  assert.ok('<lin\u{212A} src=x>x</lin\u{212A}>'.match(wordlistRe), 'kelvin-sign-folded <linK> must match');
  assert.ok('<script>\u{1F977}x()</script>'.match(wordlistRe), 'astral-adjacent tags must match');

  // 同类漂移面：兜底不能 import ESM，只能持有 DOMPurify FORBID 词表副本
  // （renderer 侧单一来源为 MARKDOWN_FORBID_TAGS / MARKDOWN_FORBID_ATTR）。
  // 锁内容一致（引号风格允许不同），防禁列表静默漂移。
  const parseWordlist = (source, name) => {
    const match = source.match(new RegExp(`${name}[ \t]*[:=][ \t]*(\\[[^\\n]*\\])`));
    assert.ok(match, `${name} must be declared as an array literal`);
    return JSON.parse(match[1].replaceAll("'", '"'));
  };
  assert.deepEqual(
    parseWordlist(fallbackSource, 'FORBID_TAGS'),
    parseWordlist(rendererSource, 'MARKDOWN_FORBID_TAGS'),
    'FORBID_TAGS content must match MARKDOWN_FORBID_TAGS',
  );
  assert.deepEqual(
    parseWordlist(fallbackSource, 'FORBID_ATTR'),
    parseWordlist(rendererSource, 'MARKDOWN_FORBID_ATTR'),
    'FORBID_ATTR content must match MARKDOWN_FORBID_ATTR',
  );
  console.log('✓ sanitize wordlists are identical across renderer and bridge fallback');
}

console.log('\nmarkdown bridge fallback safety: ok');
