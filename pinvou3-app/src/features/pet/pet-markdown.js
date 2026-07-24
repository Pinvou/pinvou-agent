import DOMPurify from 'dompurify';
import { marked } from 'marked';

const DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/gi;

marked.setOptions({ gfm: true, breaks: true, headerIds: false, mangle: false });

export function renderPetMarkdown(text) {
  const html = marked.parse(String(text || '')).replace(
    DANGEROUS_TAGS_RE,
    (_, inner) => `&lt;${inner}&gt;`,
  );
  return DOMPurify.sanitize(html, {
    FORBID_TAGS: ['style', 'iframe', 'object', 'embed', 'link', 'meta'],
    FORBID_ATTR: ['onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'onblur'],
  });
}
