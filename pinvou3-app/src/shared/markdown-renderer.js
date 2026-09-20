import createDOMPurify from 'dompurify';
/* eslint-disable import-x/namespace -- marked's ESM internals are resolved by the runtime */
import { Marked } from 'marked';
import { escapeCodeHtml, highlightCode } from './syntax-highlighter.js';
import { MARKDOWN_OPTIONS, scanMarkdownFences } from './markdown-fences.js';

const DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/giu;
// 危险标签/属性禁列表单一来源:pet 窗口等其它 DOMPurify 消费方直接复用,
// 不要各自再抄一份(此前 pet 副本漏了 u 标志,就是双写的实际事故面)。
export const MARKDOWN_FORBID_TAGS = ['style', 'iframe', 'object', 'embed', 'link', 'meta'];
export const MARKDOWN_FORBID_ATTR = ['onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'onblur'];
const SANITIZE_OPTIONS = {
  USE_PROFILES: { html: true },
  FORBID_TAGS: MARKDOWN_FORBID_TAGS,
  FORBID_ATTR: MARKDOWN_FORBID_ATTR,
};

let purifier;
function getPurifier() {
  if (purifier) return purifier;
  if (typeof createDOMPurify.sanitize === 'function') {
    purifier = createDOMPurify;
  } else if (typeof window !== 'undefined') {
    purifier = createDOMPurify(window);
  }
  return purifier;
}

/** Neutralize raw dangerous tags after marked.parse, before DOMPurify. */
export function neutralizeRawDangerousTags(html) {
  return html.replaceAll(DANGEROUS_TAGS_RE, (_, inner) => `&lt;${inner}&gt;`);
}

function fencedCodeIsClosed(token) {
  const fences = scanMarkdownFences(token.raw);
  return fences.length ? fences[0].closed : true;
}

const markdown = new Marked(MARKDOWN_OPTIONS);

markdown.use({
  renderer: {
    code(token) {
      const fenceClosed = fencedCodeIsClosed(token);
      const result = highlightCode(token.text, token.lang, {
        allowHighlight: fenceClosed,
        allowAutoDetect: fenceClosed,
      });
      const language = escapeCodeHtml(result.language);
      const languageId = escapeCodeHtml(result.languageId);
      const label = escapeCodeHtml(result.label);
      return `<pre class="pinvou-code-block" data-language="${label}" data-language-id="${languageId}"><code class="hljs language-${language}">${result.html}</code></pre>\n`;
    },
  },
});

export function renderMarkdownMarkup(text) {
  return neutralizeRawDangerousTags(markdown.parse(String(text || '')));
}

export function renderMarkdown(text) {
  const html = renderMarkdownMarkup(text);
  const domPurify = getPurifier();
  if (!domPurify || typeof domPurify.sanitize !== 'function') {
    return escapeCodeHtml(String(text || ''));
  }
  return domPurify.sanitize(html, SANITIZE_OPTIONS);
}

export function installGlobalMarkdownRenderer(target = window) {
  target.PinvouMarkdownRenderer = Object.freeze({ renderMarkdown });
  return target.PinvouMarkdownRenderer;
}
