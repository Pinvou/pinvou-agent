import createDOMPurify from 'dompurify';
/* eslint-disable import-x/namespace -- marked's ESM internals are resolved by the runtime */
import { Marked, Parser, Renderer } from 'marked';
import { escapeCodeHtml, highlightCode } from './syntax-highlighter.js';
import { MARKDOWN_OPTIONS, scanMarkdownFences } from './markdown-fences.js';

const DANGEROUS_TAGS_RE = /<(\/?(?:script|style|iframe|object|embed|link|meta)\b[^>]*)>/giu;
// 危险标签/属性禁列表单一来源:pet 窗口等其它 DOMPurify 消费方直接复用,
// 不要各自再抄一份(此前 pet 副本漏了 u 标志,就是双写的实际事故面)。
export const MARKDOWN_FORBID_TAGS = ['style', 'iframe', 'object', 'embed', 'link', 'meta'];
export const MARKDOWN_FORBID_ATTR = ['onerror', 'onload', 'onclick', 'onmouseover', 'onfocus', 'onblur'];
// 消毒配置单一来源:pet 窗口等其它 DOMPurify 消费方直接复用,
// 不要各自再拼一份(否则 USE_PROFILES 之类的限制会静默漂移)。
export const SANITIZE_OPTIONS = {
  USE_PROFILES: { html: true },
  FORBID_TAGS: MARKDOWN_FORBID_TAGS,
  FORBID_ATTR: MARKDOWN_FORBID_ATTR,
};

// One recent render supplies source context to HTML-only legacy callers.
// Conversation components pass their source explicitly; no history is retained.
let recentMarkdown = null;

export function markdownSourceForHtml(html) {
  return recentMarkdown?.html === html ? recentMarkdown.source : undefined;
}

export function markdownHtmlTagIsLiteral(raw) {
  return raw.matchAll(DANGEROUS_TAGS_RE).next().value?.index === 0;
}

export function markdownSourceTokens(source) {
  return typeof source === 'string' ? markdown.lexer(source) : [];
}

export function markdownDefinitionTitles(source) {
  if (typeof source !== 'string') return [];
  const titles = [];
  markdown.walkTokens(markdown.lexer(source), token => {
    if (token.type === 'def' && token.title) {
      titles.push(String(token.title).slice(0, 4096));
      if (titles.length > 32) titles.shift();
    }
  });
  return titles;
}

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

export function markdownCodeIsClosed(token) {
  const fences = scanMarkdownFences(token.raw);
  return fences.length ? fences[0].closed : true;
}

function renderCode(token) {
  const fenceClosed = markdownCodeIsClosed(token);
  const result = highlightCode(token.text, token.lang, {
    allowHighlight: fenceClosed,
    allowAutoDetect: fenceClosed,
  });
  const language = escapeCodeHtml(result.language);
  const languageId = escapeCodeHtml(result.languageId);
  const label = escapeCodeHtml(result.label);
  return `<pre class="pinvou-code-block" data-language="${label}" data-language-id="${languageId}"><code class="hljs language-${language}">${result.html}</code></pre>\n`;
}

const markdown = new Marked(MARKDOWN_OPTIONS);
markdown.use({ renderer: { code: renderCode } });

export function renderMarkdownMarkup(text) {
  return neutralizeRawDangerousTags(markdown.parse(String(text || '')));
}

// Detached diagnostics use the same parser and sanitizer. Decorations never
// enter production HTML; callers validate the resulting visible text first.
export function renderMarkdownSourceProbe(tokens, decorate) {
  const renderer = new Renderer(MARKDOWN_OPTIONS);
  for (const method of ['text', 'codespan', 'code', 'html']) {
    const original = method === 'code' ? renderCode : renderer[method].bind(renderer);
    renderer[method] = token => {
      const html = original(token);
      return token.tokens?.length ? html : decorate(token, html);
    };
  }
  const html = neutralizeRawDangerousTags(new Parser({ ...MARKDOWN_OPTIONS, renderer }).parse(tokens));
  const domPurify = getPurifier();
  // A detached diagnostic must not start another resource load. These
  // attribute removals do not alter the DOM text used for validation.
  return domPurify?.sanitize ? domPurify.sanitize(html, { ...SANITIZE_OPTIONS,
    FORBID_ATTR: [...MARKDOWN_FORBID_ATTR, 'src', 'srcset', 'href', 'poster', 'background', 'style'],
  }) : null;
}

export function renderMarkdown(text) {
  const source = String(text || '');
  const html = renderMarkdownMarkup(source);
  const domPurify = getPurifier();
  const result = domPurify && typeof domPurify.sanitize === 'function'
    ? domPurify.sanitize(html, SANITIZE_OPTIONS) : escapeCodeHtml(source);
  recentMarkdown = { html: result, source };
  return result;
}

export function installGlobalMarkdownRenderer(target = window) {
  target.PinvouMarkdownRenderer = Object.freeze({ renderMarkdown });
  return target.PinvouMarkdownRenderer;
}
