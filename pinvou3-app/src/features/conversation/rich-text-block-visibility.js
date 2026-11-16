import { sourceBlockTagOffsets, tagEnd } from './rich-text-html-tags.js';
import { isCodeCopyControl } from './code-block-copy.js';

export const SOURCE_BLOCK_SELECTOR = 'pre, table, td, th, caption';
const SOURCE_BLOCK_NAMES = new Set(['PRE', 'TABLE', 'TD', 'TH', 'CAPTION']);

// Markdown resolution can retire code or table parts while equal replacements
// occupy their DOM positions. Detached source identities distinguish retained
// blocks without changing visible text or presentation filtering.
function probeBlocks(tokens, origins, render, context, expected, nonce) {
  const sources = [];
  const html = render(tokens, (token, value) => {
    const origin = origins.get(token);
    if (!origin || (token.type !== 'code' && (token.type !== 'html' || token.raw !== value))) return value;
    const tags = token.type === 'code' ? [{ offset: 0, name: 'pre' }] : sourceBlockTagOffsets(value, context.isLiteralHtml);
    let html = '';
    let cursor = 0;
    for (const { offset, name } of tags) {
      const start = token.type === 'code' ? origin.start : origin.at(offset);
      if (start === undefined) continue;
      const id = `${nonce}:${sources.length}`;
      sources.push({ start, name, kind: token.type, raw: token.raw.slice(token.type === 'code' ? 0 : offset), id });
      html += `${value.slice(cursor, offset + 1 + name.length)} data-pinvou-source-block="${id}"`;
      cursor = offset + 1 + name.length;
    }
    return html + value.slice(cursor);
  });
  if (html === null) return null;
  const root = context.sourceDocument.createElement('div');
  root.innerHTML = html;
  const indexed = indexBlockText(root);
  const visible = new Map(indexed.blocks.filter(block => block.node.dataset.pinvouSourceBlock)
    .map(block => [block.node.dataset.pinvouSourceBlock, { start: block.start, end: block.end }]));
  return indexed.text === expected ? { sources, visible, blocks: indexed.blocks } : null;
}

function indexBlockText(root) {
  const blocks = [];
  let text = '';
  const visit = node => {
    if (isCodeCopyControl(node)) return;
    if (node.nodeType === 3) { text += node.data; return; }
    const block = SOURCE_BLOCK_NAMES.has(node.nodeName) ? { node, start: text.length, end: text.length } : null;
    if (block) blocks.push(block);
    for (const child of node.childNodes) visit(child);
    if (block) block.end = text.length;
  };
  visit(root);
  return { text, blocks };
}

function commonSourcePrefix(context) {
  const before = context.previousSource;
  const after = context.source;
  let length = 0;
  while (length < before.length && length < after.length && before[length] === after[length]) length += 1;
  return length === before.length ? Infinity : length;
}

function retainedOpening(block, source, prefix) {
  if (prefix === Infinity) return true;
  const end = block.kind === 'code' ? source.indexOf('\n', block.start) + 1 : tagEnd(source, block.start);
  return end > block.start && end <= prefix;
}

export function createSourceBlockKeys(previousTokens, previousOrigins, tokens, origins, context, previousRoot, root) {
  const crypto = context.sourceDocument?.defaultView?.crypto;
  if (!crypto?.getRandomValues || !context.renderCurrentSourceProbe) return null;
  const nonce = Array.from(crypto.getRandomValues(new Uint32Array(4)), value => value.toString(16)).join('');
  const actualBefore = indexBlockText(previousRoot);
  const actualAfter = indexBlockText(root);
  const before = probeBlocks(previousTokens, previousOrigins, context.renderSourceProbe, context, actualBefore.text, nonce);
  const after = probeBlocks(tokens, origins, context.renderCurrentSourceProbe, context, actualAfter.text, nonce);
  if (!before || !after) return null;
  const keys = new WeakMap();
  const prefix = commonSourcePrefix(context);
  for (const [side, actual, probe] of [['before', actualBefore, before], ['after', actualAfter, after]]) {
    if (actual.blocks.length !== probe.blocks.length) return null;
    const sources = new Map(probe.sources.map(block => [block.id, block]));
    for (let index = 0; index < actual.blocks.length; index += 1) {
      const block = actual.blocks[index];
      const diagnostic = probe.blocks[index];
      if (block.node.nodeName !== diagnostic.node.nodeName || block.start !== diagnostic.start || block.end !== diagnostic.end) return null;
      const origin = sources.get(diagnostic.node.dataset.pinvouSourceBlock);
      const source = side === 'before' ? context.previousSource : context.source;
      // Unowned or replaced openings cannot lend state to a new source block.
      keys.set(block.node, origin && retainedOpening(origin, source, prefix)
        ? `source:${origin.start}` : `${nonce}:${side}:${index}`);
    }
  }
  return keys;
}

export function maskFilteredSourceCode(previousTokens, previousOrigins, tokens, origins, context, mask) {
  const crypto = context.sourceDocument?.defaultView?.crypto;
  if (!crypto?.getRandomValues || !context.renderCurrentSourceProbe) return;
  const nonce = Array.from(crypto.getRandomValues(new Uint32Array(4)), value => value.toString(16)).join('');
  const before = probeBlocks(previousTokens, previousOrigins, context.renderSourceProbe,
    context, context.before.text, nonce);
  if (!before) return;
  const after = probeBlocks(tokens, origins, context.renderCurrentSourceProbe,
    context, context.after.text, nonce);
  if (!after) return;
  const current = new Map(after.sources.filter(block => block.name === 'pre').map(code => [code.start, code]));
  for (const code of before.sources.filter(block => block.name === 'pre')) {
    const displayed = before.visible.get(code.id);
    const next = current.get(code.start);
    // Only a source-retained code block removed by presentation filtering is
    // retired here. Markdown structure changes keep the existing text mapper.
    if (displayed && next?.raw.startsWith(code.raw) && !after.visible.has(next.id)) {
      mask.fill(1, displayed.start, displayed.end);
    }
  }
}

function simpleCodeSources(source, tokens) {
  const codes = [];
  let cursor = 0;
  for (const token of tokens) {
    if (!token.raw) return null;
    // Require complete source coverage. Hidden reference definitions or lexer
    // gaps cannot lend an equal raw code spelling to a different source token.
    const start = cursor;
    if (!source.startsWith(token.raw, cursor)) return null;
    cursor += token.raw.length;
    if (token.type === 'code') codes.push({ start, raw: token.raw });
    else if (token.type === 'table' || token.raw.includes('<')) return null;
  }
  return codes.length && cursor === source.length ? codes : null;
}

export function sourceBlockState(root, tokens, context, keys) {
  if (context.hasPresentationTransform) return null;
  const codes = simpleCodeSources(context.source, tokens);
  const indexed = indexBlockText(root);
  if (!codes || codes.length !== indexed.blocks.length) return null;
  for (let index = 0; index < codes.length; index += 1) {
    const block = indexed.blocks[index];
    if (block.node.nodeName !== 'PRE' || keys.get(block.node) !== `source:${codes[index].start}`
      || !context.isClosedCode?.({ raw: codes[index].raw })) return null;
    codes[index].html = block.node.outerHTML;
  }
  return { source: context.source, codes, text: indexed.text };
}

export function cachedSourceBlockKeys(previousRoot, root, tokens, context) {
  const cache = context.previousBlockState;
  if (!cache || context.hasPresentationTransform || !context.source.startsWith(context.previousSource)) return null;
  const codes = simpleCodeSources(context.source, tokens);
  if (!codes || codes.length !== cache.codes.length || codes.some((code, index) =>
    code.start !== cache.codes[index].start || code.raw !== cache.codes[index].raw)) return null;
  const before = indexBlockText(previousRoot);
  const after = indexBlockText(root);
  if (before.text !== cache.text || before.blocks.length !== codes.length || after.blocks.length !== codes.length) return null;
  const keys = new WeakMap();
  for (let index = 0; index < codes.length; index += 1) {
    const current = before.blocks[index].node;
    const next = after.blocks[index].node;
    const key = `source:${codes[index].start}`;
    if (current.nodeName !== 'PRE' || next.nodeName !== 'PRE' || cache.keys.get(current) !== key
      || next.outerHTML !== cache.codes[index].html) return null;
    keys.set(current, key);
    keys.set(next, key);
  }
  // Keep only one validated frame per mounted message. Fade and JS-owned copy
  // feedback do not change the indexed payload or source block identity.
  context.nextBlockState = { source: context.source, codes: cache.codes, text: after.text };
  return keys;
}
