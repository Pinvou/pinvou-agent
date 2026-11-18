import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { ChatBubble } from '../../src/features/chat/ChatView.jsx';
import { dict, ensureLanguage } from '../../src/shared/i18n.js';
import { renderMarkdown } from '../../src/shared/markdown-renderer.js';

// The smoke server exposes the private component only in this test build.
await ensureLanguage('en');
const root = createRoot(document.getElementById('root'));
window.legacySelectionFixture = {
  renderHtml: renderMarkdown,
  render(text, html, htmlSource, streaming = true) {
    flushSync(() => root.render(<ChatBubble
      item={{ id: 'throttle', type: 'assistant', streaming, text, html, htmlSource }}
      sessionId="fixture" theme="light" t={dict.en} showAssistantActions={false}
    />));
  },
  select(word, backward = false, selector = '#root .msg-md') {
    const host = document.querySelector(selector);
    const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
    const entries = [];
    let text = '';
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.parentElement.closest('button, .pinvou-code-copy-live')) continue;
      entries.push({ node, start: text.length, end: text.length + node.length });
      text += node.data;
    }
    const start = text.indexOf(word);
    if (start < 0) throw new Error(`Selection missing: ${word}`);
    const from = entries.find(entry => start < entry.end);
    const end = start + word.length;
    const to = entries.find(entry => end <= entry.end);
    const anchor = [from.node, start - from.start];
    const focus = [to.node, end - to.start];
    if (backward) getSelection().setBaseAndExtent(...focus, ...anchor);
    else getSelection().setBaseAndExtent(...anchor, ...focus);
  },
};
