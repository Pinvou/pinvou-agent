import createDOMPurify from 'dompurify';
import { Marked } from 'marked';
import { escapeCodeHtml, highlightCode } from './syntax-highlighter.js';

const DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/giu;
const SANITIZE_OPTIONS = {
  USE_PROFILES: { html: true },
  FORBID_TAGS: ['style', 'iframe', 'object', 'embed', 'link', 'meta'],
  FORBID_ATTR: ['onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'onblur'],
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

function neutralizeRawDangerousTags(html) {
  return html.replace(DANGEROUS_TAGS_RE, (_, inner) => `&lt;${inner}&gt;`);
}

function fencedCodeIsClosed(token) {
  const opening = String(token.raw || '').match(/^\s*(`{3,}|~{3,})/u);
  if (!opening) return true;
  const fence = opening[1];
  const closing = new RegExp(`(?:^|\\n)\\s*${fence[0]}{${fence.length},}\\s*$`, 'u');
  return closing.test(String(token.raw || '').trimEnd());
}

const markdown = new Marked({
  gfm: true,
  breaks: true,
  headerIds: false,
  mangle: false,
});

markdown.use({
  useNewRenderer: true,
  renderer: {
    code(token) {
      const result = highlightCode(token.text, token.lang, {
        allowAutoDetect: fencedCodeIsClosed(token),
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
