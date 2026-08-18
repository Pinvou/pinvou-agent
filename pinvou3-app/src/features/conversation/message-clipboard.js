import TurndownService from 'turndown';
import { gfm } from 'turndown-plugin-gfm';
import {
  assistantMarkdownCopyText,
  normalizeAssistantMessageText,
} from './structured-assistant-content.js';
import { copyClipboardText, fallbackCopyText } from '../../shared/clipboard.js';

export { copyClipboardText, fallbackCopyText };

let legacyHtmlConverter = null;

function legacyFencedCodeLanguage(node) {
  const code = node && node.firstChild;
  const className = String((code && code.getAttribute && code.getAttribute('class')) || '');
  const classLanguage = (className.match(/language-(\S+)/) || [null, ''])[1];
  const dataLanguageId = String((node && node.getAttribute && node.getAttribute('data-language-id')) || '')
    .trim()
    .toLowerCase();
  const dataLanguage = String((node && node.getAttribute && node.getAttribute('data-language')) || '')
    .trim();
  // renderMarkdown 把无法被 hljs 识别的围栏语言（persona-card / card-question /
  // scheduled-task-draft 等协议标签）记录在 pre 的 data-language 上，而 code 的
  // class 只会是 language-plaintext。这里把这些协议标签还原回围栏信息，让旧 HTML
  // 会话的复制与 UI 卡片分类保持一致；已知语言仍优先用 code class 的 language-*。
  if ((!classLanguage || classLanguage === 'plaintext') && dataLanguageId === 'plaintext' && dataLanguage && dataLanguage.toLowerCase() !== 'text') {
    return dataLanguage;
  }
  return classLanguage;
}

function legacyAssistantHtmlToMarkdown(html) {
  if (!html) return '';
  if (!legacyHtmlConverter) {
    legacyHtmlConverter = new TurndownService({
      headingStyle: 'atx',
      bulletListMarker: '-',
      codeBlockStyle: 'fenced',
    });
    legacyHtmlConverter.use(gfm);
    legacyHtmlConverter.addRule('pinvouFencedCodeLanguage', {
      filter: (node, options) => (
        options.codeBlockStyle === 'fenced'
        && node.nodeName === 'PRE'
        && node.firstChild
        && node.firstChild.nodeName === 'CODE'
        && node.getAttribute('data-language')
      ),
      replacement: (content, node, options) => {
        const code = node.firstChild;
        const language = legacyFencedCodeLanguage(node);
        const fenceChar = options.fence.charAt(0);
        let fenceSize = 3;
        const fenceInCodeRegex = new RegExp(`^${fenceChar}{3,}`, 'gm');
        let match;
        while ((match = fenceInCodeRegex.exec(code.textContent))) {
          if (match[0].length >= fenceSize) fenceSize = match[0].length + 1;
        }
        const fence = fenceChar.repeat(fenceSize);
        return `\n\n${fence}${language}\n${String(code.textContent).replace(/\n$/, '')}\n${fence}\n\n`;
      },
    });
    legacyHtmlConverter.keep(['kbd']);
    legacyHtmlConverter.remove(['script', 'style']);
  }
  return legacyHtmlConverter.turndown(String(html)).replace(/\u00a0/g, ' ');
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
    .map(item => {
      if (item.copyText != null) return normalizeAssistantMessageText(item.copyText);
      const source = normalizeAssistantMessageText(item.text);
      if (source && item.copyOptions !== undefined) {
        return assistantMarkdownCopyText(source, item.copyOptions);
      }
      return source || assistantItemCopyText(item.legacyItem, item.copyOptions);
    })
    .filter(Boolean);
  if (agentMessages.length) return normalizeAssistantMessageText(messages.join('\n\n'));
  return normalizeAssistantMessageText(turn.assistantText);
}

export function assistantResponseAvailable(turn) {
  if (!turn) return false;
  const items = Array.isArray(turn.items) && turn.items.length
    ? turn.items
    : Array.isArray(turn.presentation)
      ? turn.presentation
      : [];
  const agentMessages = items.filter(item => item?.type === 'agent_message');
  if (agentMessages.length) {
    return agentMessages.some(item => (
      item.phase !== 'commentary'
      && [item.copyText, item.text, item.legacyItem?.text, item.legacyItem?.html]
        .some(value => String(value || '').trim())
    ));
  }
  return Boolean(String(turn.assistantText || '').trim());
}
