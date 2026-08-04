import TurndownService from 'turndown';
import { gfm } from 'turndown-plugin-gfm';
import {
  assistantMarkdownCopyText,
  normalizeAssistantMessageText,
} from './structured-assistant-content.js';

let legacyHtmlConverter = null;

function legacyAssistantHtmlToMarkdown(html) {
  if (!html) return '';
  if (!legacyHtmlConverter) {
    legacyHtmlConverter = new TurndownService({
      headingStyle: 'atx',
      bulletListMarker: '-',
      codeBlockStyle: 'fenced',
    });
    legacyHtmlConverter.use(gfm);
    legacyHtmlConverter.keep(['kbd']);
    legacyHtmlConverter.remove(['script', 'style']);
  }
  return legacyHtmlConverter.turndown(String(html));
}

export function fallbackCopyText(text) {
  return new Promise((resolve) => {
    if (typeof document === 'undefined' || !document.body) {
      resolve(false);
      return;
    }
    let textarea = null;
    try {
      textarea = document.createElement('textarea');
      textarea.value = String(text || '');
      textarea.setAttribute('readonly', '');
      textarea.style.position = 'fixed';
      textarea.style.left = '-9999px';
      textarea.style.top = '-9999px';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.focus();
      textarea.select();
      textarea.setSelectionRange(0, textarea.value.length);
      resolve(Boolean(document.execCommand('copy')));
    } catch {
      resolve(false);
    } finally {
      if (textarea?.parentNode) textarea.parentNode.removeChild(textarea);
    }
  });
}

export function copyClipboardText(text) {
  const value = String(text || '');
  if (!value) return Promise.resolve(false);
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(value)
      .then(() => true)
      .catch(() => fallbackCopyText(value));
  }
  return fallbackCopyText(value);
}

export function readClipboardText() {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.readText) {
    return navigator.clipboard.readText().catch(() => '');
  }
  return Promise.resolve('');
}

export { assistantMarkdownCopyText, normalizeAssistantMessageText };

export function assistantItemCopyText(item, options) {
  if (!item) return '';
  const markdown = normalizeAssistantMessageText(item.text);
  if (markdown) return assistantMarkdownCopyText(markdown, options);
  return assistantMarkdownCopyText(legacyAssistantHtmlToMarkdown(item.html), options);
}

export function assistantResponseText(turn) {
  if (!turn) return '';
  const items = Array.isArray(turn.items) && turn.items.length
    ? turn.items
    : Array.isArray(turn.presentation)
      ? turn.presentation
      : [];
  const agentMessages = items.filter(item => item?.type === 'agent_message');
  const messages = agentMessages
    .filter(item => item.phase !== 'commentary')
    .map(item => (
      normalizeAssistantMessageText(item.copyText ?? item.text)
      || assistantItemCopyText(item.legacyItem, item.copyOptions)
    ))
    .filter(Boolean);
  if (agentMessages.length) return normalizeAssistantMessageText(messages.join('\n\n'));
  return normalizeAssistantMessageText(turn.assistantText);
}
