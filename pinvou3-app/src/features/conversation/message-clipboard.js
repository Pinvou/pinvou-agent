import {
  assistantMarkdownCopyText,
  normalizeAssistantMessageText,
} from './structured-assistant-content.js';
import { copyClipboardText, fallbackCopyText } from '../../shared/clipboard.js';
import { createTurndownService } from '../../shared/turndown-factory.js';

export { copyClipboardText, fallbackCopyText };

// 旧 HTML 会话复制为 Markdown 的转换器懒加载缓存。
let legacyHtmlConverter = null;
let legacyConverterLoading = null;

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

async function ensureLegacyHtmlConverter() {
  if (legacyHtmlConverter) return legacyHtmlConverter;
  if (!legacyConverterLoading) {
    legacyConverterLoading = (async () => {
      const converter = await createTurndownService();
      converter.addRule('pinvouFencedCodeLanguage', {
        filter: (node, options) => (
          options.codeBlockStyle === 'fenced'
          && node.nodeName === 'PRE'
          && node.firstChild
          && node.firstChild.nodeName === 'CODE'
          && node.getAttribute('data-language')
        ),
        replacement: (_content, node, options) => {
          const code = node.firstChild;
          const language = legacyFencedCodeLanguage(node);
          const fenceChar = options.fence.charAt(0);
          let fenceSize = 3;
          const fenceInCodeRegex = new RegExp(`^${fenceChar}{3,}`, 'gm');
          let match;
          // biome-ignore lint/suspicious/noAssignInExpressions: 赋值即循环条件,重构损害可读性
          while ((match = fenceInCodeRegex.exec(code.textContent))) {
            if (match[0].length >= fenceSize) fenceSize = match[0].length + 1;
          }
          const fence = fenceChar.repeat(fenceSize);
          return `\n\n${fence}${language}\n${String(code.textContent).replace(/\n$/, '')}\n${fence}\n\n`;
        },
      });
      converter.remove(['script', 'style']);
      legacyHtmlConverter = converter;
      return converter;
    })().catch((error) => {
      // 加载失败(如动态 import 异常)时清除缓存,允许下次复制重试。
      legacyConverterLoading = null;
      throw error;
    });
  }
  return legacyConverterLoading;
}

async function legacyAssistantHtmlToMarkdown(html) {
  if (!html) return '';
  const converter = await ensureLegacyHtmlConverter();
  return converter.turndown(String(html)).replaceAll('\u00A0', ' ');
}

export function readClipboardText() {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.readText) {
    return navigator.clipboard.readText().catch(() => '');
  }
  return Promise.resolve('');
}

export { assistantMarkdownCopyText, normalizeAssistantMessageText };

// 旧 HTML 会话的复制走异步转换(turndown 懒加载后执行);text 直存的新会话
// 在同一函数内走同步路径,不触发懒加载。
export async function assistantItemCopyText(item, options) {
  if (!item) return '';
  const markdown = normalizeAssistantMessageText(item.text);
  if (markdown) return assistantMarkdownCopyText(markdown, options);
  return assistantMarkdownCopyText(await legacyAssistantHtmlToMarkdown(item.html), options);
}

// 旧 HTML 会话的 agent_message 可能只有 legacyItem.html 没有 text,整轮复制
// 必须经 turndown 转换兜底,因此本函数为 async;消费方统一 await。
export async function assistantResponseText(turn) {
  if (!turn) return '';
  const items = Array.isArray(turn.items) && turn.items.length
    ? turn.items
    : Array.isArray(turn.presentation)
      ? turn.presentation
      : [];
  const agentMessages = items.filter(item => item?.type === 'agent_message');
  const collected = await Promise.all(agentMessages
    .filter(item => item.phase !== 'commentary')
    .map(async item => {
      if (item.copyText != null) return normalizeAssistantMessageText(item.copyText);
      const source = normalizeAssistantMessageText(item.text);
      if (source && item.copyOptions !== undefined) {
        return assistantMarkdownCopyText(source, item.copyOptions);
      }
      if (source) return source;
      return assistantItemCopyText(item.legacyItem, item.copyOptions);
    }));
  const messages = collected.filter(Boolean);
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
