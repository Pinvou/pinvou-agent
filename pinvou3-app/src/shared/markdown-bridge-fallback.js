/**
 * markdown-bridge-fallback.js — 供 plain-script bridge（web/tauri）共用的 Markdown 渲染兜底。
 *
 * 背景：platform/{web,tauri}/bridge.js 是以 <script src> 加载的普通脚本（非 ES module），
 * 无法 import shared/markdown-renderer.js（后者依赖 marked/dompurify 等 npm 包、由 Vite 打包）。
 * 因此 bridge 层保留一份基于 vendor 全局（window.marked / window.DOMPurify）的兜底实现，
 * 供 React 主包尚未安装 window.PinvouMarkdownRenderer 的短暂窗口使用。
 *
 * 此前该兜底在 platform/web/bridge.js 与 platform/tauri/bridge.js 中逐字重复两份，
 * 现收敛到本文件，由两处 bridge 统一引用 window.PinvouMarkdownBridgeFallback。
 * 注意：bridge 层负责「优先委托 window.PinvouMarkdownRenderer（npm 版，含语法高亮）」，
 * 仅在共享渲染器尚未安装时才调用本文件的 vendor 全局兜底；本文件不再重复该委托。
 */
(function (root) {
  "use strict";

  // 抹平裸 <script>/<style>/<iframe> 等危险标签：在 marked.parse 【之后】做替换而非之前，
  // 只命中正文裸写的 HTML，不会双重转义代码块里已被转义的 `<script>` 字面量。
  var DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/gi;

  function neutralizeRawDangerousTags(html) {
    return html.replace(DANGEROUS_TAGS_RE, function (_, inner) { return "&lt;" + inner + "&gt;"; });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }

  function renderMarkdown(text) {
    if (!root.marked || !root.DOMPurify) return escapeHtml(text);
    var html = neutralizeRawDangerousTags(root.marked.parse(text || ""));
    return root.DOMPurify.sanitize(html, {
      // 兜底：即使 neutralize 有漏网（罕见 HTML 注释/CDATA 等），DOMPurify 仍剥掉这些
      FORBID_TAGS: ["style", "iframe", "object", "embed", "link", "meta"],
      FORBID_ATTR: ["onerror", "onload", "onclick", "onmouseover", "onfocus", "onblur"],
    });
  }

  if (root.marked) {
    root.marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });
  }

  root.PinvouMarkdownBridgeFallback = Object.freeze({ renderMarkdown: renderMarkdown });
})(typeof window !== "undefined" ? window : this);
