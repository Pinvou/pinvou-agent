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
  // 刻意不安装 PinvouMarkdownRenderer / PinvouMarkdownBridgeFallback（模拟共享脚本缺失）。
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
  assert.ok(out.indexOf('&lt;img') >= 0, `dangerous tags must be HTML-escaped, got: ${out}`);
  assert.ok(out.indexOf('&lt;script') >= 0, `script tags must be HTML-escaped, got: ${out}`);
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
  vm.runInContext(readApp('src', 'platform', 'web', 'bridge.js'), context, {
    filename: 'platform/web/bridge.js',
  });
  const out = windowObject.TauriBridge.renderMarkdown('before <script>x()</script> after');
  assert.ok(
    out.indexOf('&lt;script') >= 0 && out.indexOf('<script') < 0,
    `shared fallback must neutralize raw <script>, got: ${out}`,
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
  assert.ok(out.indexOf('&lt;iframe') >= 0, `iframe must be escaped, got: ${out}`);
  console.log('✓ shared fallback escapes HTML when vendor globals are missing');
}

// ── Case 4: 守护两处 bridge 源码——最末级 fallback 不得退化为原文返回 ──
// 防止未来重构再次把 escapeHtml 抽走、留下 fail-open 的 `return String(text || "")`。
for (const bridgeRel of [['src', 'platform', 'web', 'bridge.js'], ['src', 'platform', 'tauri', 'bridge.js']]) {
  const src = readApp(...bridgeRel);
  assert.match(
    src,
    /return escapeHtml\(text \|\| ""\)/u,
    `${bridgeRel.join('/')} final fallback must escape (not return raw text)`,
  );
  // 同时守护 escapeHtml 的定义就地产于此文件（安全原语不依赖外部脚本）。
  assert.match(
    src,
    /function escapeHtml\(s\)/u,
    `${bridgeRel.join('/')} must define its own escapeHtml for the final fallback`,
  );
  // 委托共享渲染器的主路径仍须保留（markdown_syntax_highlight 契约同样要求）。
  assert.match(
    src,
    /window\.PinvouMarkdownRenderer\.renderMarkdown\(text\)/u,
    `${bridgeRel.join('/')} must keep delegating to the shared renderer`,
  );
  console.log(`✓ ${bridgeRel.join('/')} keeps safe final fallback + renderer delegation`);
}

console.log('\nmarkdown bridge fallback safety: ok');
