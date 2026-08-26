import DOMPurify from 'dompurify';
// eslint-disable-next-line import-x/namespace -- marked v14's ESM source uses ES2022 class fields, unparseable at this config's ES2021 floor (same as src/shared/markdown-renderer.js); the runtime bundler handles it, no impact
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
