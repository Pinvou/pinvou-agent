import React from 'react';
import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { ConversationMarkdown } from '../../src/features/conversation/ConversationTimeline.jsx';
import { renderMarkdown } from '../../src/shared/markdown-renderer.js';
import { updateRichText } from '../../src/features/conversation/rich-text-dom.js';
import { applyStreamFade, setStreamFadePreference } from '../../src/features/conversation/stream-fade.js';
import { ensureCodeCopyButtons } from '../../src/features/conversation/code-block-copy.js';

const root = createRoot(document.getElementById('root'));
const copy = { copyCode: 'Copy code', copyReplySuccess: 'Copied', copyReplyFailed: 'Copy failed' };
const writes = [];
let pendingWrite = null;
let writeMode = 'success';
Object.defineProperty(navigator, 'clipboard', {
  configurable: true,
  value: { writeText: text => {
    writes.push(text);
    if (writeMode === 'pending') return new Promise(resolve => { pendingWrite = resolve; });
    if (writeMode === 'failure') return Promise.reject(new Error('clipboard denied'));
    return Promise.resolve();
  } },
});
document.execCommand = () => false;

function render(text, streaming = true) {
  flushSync(() => root.render(<ConversationMarkdown text={text} streaming={streaming} copy={copy} />));
}

function pointAt(container, absolute, preferPrevious) {
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let node = walker.nextNode();
  while (node) {
    if (!node.parentElement.closest('button, .pinvou-code-copy-live')) {
      if (absolute < node.length || (absolute === node.length && preferPrevious)) return [node, absolute];
      absolute -= node.length;
    }
    node = walker.nextNode();
  }
  throw new Error('Selection offset outside fixture content');
}

function select(selector, text, backward = false, occurrence = 0) {
  const container = document.querySelector(selector);
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
  let value = '';
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!node.parentElement.closest('button, .pinvou-code-copy-live')) value += node.data;
  }
  let start = -1;
  for (let index = 0; index <= occurrence; index += 1) start = value.indexOf(text, start + 1);
  if (start < 0) throw new Error(`Selection text missing: ${text}`);
  const from = pointAt(container, start, false);
  const to = pointAt(container, start + text.length, true);
  const selection = window.getSelection();
  if (backward) selection.setBaseAndExtent(...to, ...from);
  else selection.setBaseAndExtent(...from, ...to);
  return selection.toString();
}

window.streamingFixture = {
  render,
  renderHtml: renderMarkdown,
  updateRichText,
  select,
  writes,
  mode(value) { writeMode = value; },
  finishWrite() { pendingWrite?.(); pendingWrite = null; },
  setAnimation: setStreamFadePreference,
  legacy(html, streaming = true) {
    const container = document.getElementById('legacy');
    updateRichText(container, html);
    ensureCodeCopyButtons(container, { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' });
    applyStreamFade(container, { streaming, html });
  },
};
render('Ready');
