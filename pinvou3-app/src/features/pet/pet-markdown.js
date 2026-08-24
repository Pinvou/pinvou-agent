import DOMPurify from 'dompurify';
// eslint-disable-next-line import-x/namespace -- marked v14 的 ESM 源码使用 ES2022 类字段,无法按本配置的 ES2021 地板解析(与 src/shared/markdown-renderer.js 同况);运行时经打包器处理无影响
import { marked } from 'marked';

const DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/gi;

marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });

export function renderPetMarkdown(text) {
  const html = marked.parse(String(text || '')).replaceAll(
    DANGEROUS_TAGS_RE,
    (_, inner) => `&lt;${inner}&gt;`,
  );
  return DOMPurify.sanitize(html, {
    FORBID_TAGS: ['style', 'iframe', 'object', 'embed', 'link', 'meta'],
    FORBID_ATTR: ['onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'onblur'],
  });
}
