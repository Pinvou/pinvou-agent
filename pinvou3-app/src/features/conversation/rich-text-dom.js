import { markdownDefinitionTitles, markdownSourceForHtml, markdownSourceTokens, renderMarkdownSourceProbe, markdownHtmlTagIsLiteral, markdownCodeIsClosed } from '../../shared/markdown-renderer.js';
import { CODE_COPY_BLOCK_SELECTOR, isCodeCopyControl, ensureCodeCopyButtons, observeCodeCopyButtons } from './code-block-copy.js';
import { sourceBlockKeys, SOURCE_BLOCK_SELECTOR } from './rich-text-source.js';
import { preserveRichTextSelection } from './rich-text-selection.js';
import { prepareStreamFadeUpdate, applyStreamFade } from './stream-fade.js';

// Keep the committed tuple atomic: failed parsing/reconciliation must not
// advance source ownership independently of the visible frame.
const renderedFrames = new WeakMap();

function updateText(current, next) {
  const before = current.data;
  const after = next.data;
  if (before === after) return;
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start += 1;
  let suffix = 0;
  while (suffix < before.length - start && suffix < after.length - start
    && before[before.length - suffix - 1] === after[after.length - suffix - 1]) suffix += 1;
  // Replacing nodeValue resets native Range offsets even when the text node
  // itself survives. A splice leaves selection in an unchanged prefix intact.
  current.replaceData(start, before.length - start - suffix, after.slice(start, after.length - suffix));
}

function updateAttributes(current, next) {
  for (const attribute of [...current.attributes]) {
    if (!next.hasAttribute(attribute.name)) current.removeAttribute(attribute.name);
  }
  for (const attribute of next.attributes) {
    if (current.getAttribute(attribute.name) !== attribute.value) {
      current.setAttribute(attribute.name, attribute.value);
    }
  }
}

function nodeKey(node, sourceKeys) {
  const kind = `${node.nodeType}:${node.namespaceURI || ''}:${node.nodeName}`;
  const source = sourceKeys?.get(node);
  return source === undefined ? kind : `${kind}:${source}`;
}

// HTML template children belong to an inert DocumentFragment.
function childContainer(node) {
  return node.nodeName === 'TEMPLATE' && node.content ? node.content : node;
}

// Source-owned code and table parts can survive surrounding wrapper changes.
// Keep a message-wide pool, while ordinary nodes still match locally.
function sourceBlockMatches(root, sourceKeys) {
  if (!sourceKeys) return null;
  const blocks = new Map();
  const used = new Set();
  for (const block of root.querySelectorAll(SOURCE_BLOCK_SELECTOR)) {
    const key = nodeKey(block, sourceKeys);
    if (!blocks.has(key)) blocks.set(key, []);
    blocks.get(key).push(block);
  }
  return {
    take(incoming, parent) {
      if (!sourceKeys.has(incoming)) return null;
      const candidates = blocks.get(nodeKey(incoming, sourceKeys));
      const block = candidates?.find(candidate => !used.has(candidate) && !candidate.contains(parent));
      if (!block) return null;
      used.add(block);
      return block;
    },
  };
}

// A new ancestor must adopt retained source blocks; inserting its fresh
// subtree wholesale would lose code feedback and table endpoint ownership.
function adoptSourceBlocks(incoming, sourceKeys, matches) {
  if (!matches || incoming.nodeType !== 1) return;
  const parent = childContainer(incoming);
  for (const child of [...parent.childNodes]) {
    const block = matches.take(child, parent);
    if (block) {
      updateAttributes(block, child);
      reconcileChildren(childContainer(block), childContainer(child), sourceKeys, matches);
      parent.replaceChild(block, child);
    } else adoptSourceBlocks(child, sourceKeys, matches);
  }
}

function localChildMatches(originals, sourceKeys, reused) {
  let candidates;
  return incoming => {
    // Construct queues only when structure differs. Matching by node kind
    // keeps an existing pre/code subtree when loose-list paragraph wrappers
    // or whitespace are inserted before it. Queues bound sibling searches.
    if (!candidates) {
      candidates = new Map();
      for (const node of originals) {
        const key = nodeKey(node, sourceKeys);
        if (!candidates.has(key)) candidates.set(key, { nodes: [], index: 0 });
        candidates.get(key).nodes.push(node);
      }
    }
    const queue = candidates.get(nodeKey(incoming, sourceKeys));
    if (!queue) return null;
    while (queue.index < queue.nodes.length) {
      const node = queue.nodes[queue.index++];
      if (!reused.has(node)) return node;
    }
    return null;
  };
}

function canPromoteParagraph(cursor, incoming, sourceKeys, matches) {
  return cursor?.nodeType === 1 && cursor.nodeName === 'P' && cursor.hasChildNodes() && incoming.nodeType === 1
    && incoming.nodeName !== 'P' && !(matches && sourceKeys.has(incoming))
    && cursor.namespaceURI === incoming.namespaceURI;
}

function matchingChild(current, cursor, incoming, sourceKeys, matches, findMatching) {
  if (matches && sourceKeys.has(incoming)) return matches.take(incoming, current);
  return cursor && nodeKey(cursor, sourceKeys) === nodeKey(incoming, sourceKeys) ? cursor : findMatching(incoming);
}

function pendingChild(current, cursor, keepControl, reused) {
  if (cursor && cursor.parentNode !== current) cursor = current.firstChild;
  while (cursor && (keepControl(cursor) || reused.has(cursor))) cursor = cursor.nextSibling;
  return cursor;
}

function reconcileChildren(current, next, sourceKeys, matches) {
  const keepControl = node => Boolean(next.matches?.(CODE_COPY_BLOCK_SELECTOR) && isCodeCopyControl(node));
  const originals = [...current.childNodes].filter(node => !keepControl(node));
  const reused = new Set();
  const controls = [...current.childNodes].filter(keepControl);
  const findMatching = localChildMatches(originals, sourceKeys, reused);
  let cursor = current.firstChild;
  for (const incoming of [...next.childNodes]) {
    // A descendant may have adopted the cursor block from this parent.
    cursor = pendingChild(current, cursor, keepControl, reused);
    if (canPromoteParagraph(cursor, incoming, sourceKeys, matches)) {
      // A nonempty paragraph promoted to a heading/table owns this source position.
      // Parser-generated empty paragraphs cannot replace a later retained table.
      // Do not save its nodes for a newly appended paragraph of the old type.
      // Retain children where possible so a heading keeps the original text.
      const following = cursor.nextSibling;
      const replacement = incoming.cloneNode(false);
      // eslint-disable-next-line unicorn/prefer-dom-node-append -- DOM fixtures use domino, which has no ParentNode#append
      while (cursor.firstChild) childContainer(replacement).appendChild(cursor.firstChild);
      current.replaceChild(replacement, cursor);
      reused.add(cursor);
      reused.add(replacement);
      reconcileChildren(childContainer(replacement), childContainer(incoming), sourceKeys, matches);
      cursor = following;
      continue;
    }
    const node = matchingChild(current, cursor, incoming, sourceKeys, matches, findMatching);
    if (!node) {
      adoptSourceBlocks(incoming, sourceKeys, matches);
      // Adoption can also move the insertion cursor itself. Recompute its
      // boundary before inserting, so following siblings stay after the block.
      cursor = pendingChild(current, cursor, keepControl, reused);
      current.insertBefore(incoming, cursor || controls[0] || null);
      reused.add(incoming);
      continue;
    }
    const following = node === cursor ? node.nextSibling : cursor;
    if (node !== cursor) current.insertBefore(node, cursor || controls[0] || null);
    reused.add(node);
    if (node.nodeType === 3) updateText(node, incoming);
    else if (node.nodeType === 1) {
      updateAttributes(node, incoming);
      reconcileChildren(childContainer(node), childContainer(incoming), sourceKeys, matches);
    } else if (node.nodeValue !== incoming.nodeValue) node.nodeValue = incoming.nodeValue;
    cursor = following;
  }
  for (const node of originals) {
    if (!reused.has(node) && node.parentNode === current) node.remove();
  }
}

/**
 * Commit already-sanitized assistant HTML without replacing the message tree.
 * Parsing/sanitization stay with the existing Markdown pipeline; this module
 * only reconciles its DOM output. It also serves legacy HTML-only messages.
 */
export function updateRichText(container, html, { markdownSource, sourceHtmlTransform, sourceHtmlFiltered = false } = {}) {
  if (!container) return;
  const source = markdownSource ?? markdownSourceForHtml(html);
  const previous = renderedFrames.get(container) || {};
  const { source: previousSource, transform: previousTransform, filtered: previousFiltered, blocks: previousBlockState } = previous;
  // Equal markup can hide a replaced opening or a different presentation
  // source (for example, backticks changed to tildes around equal code).
  if (previous.html === html && source === previousSource
    && sourceHtmlTransform === previousTransform && sourceHtmlFiltered === Boolean(previousFiltered)) return;
  const next = container.ownerDocument.createElement('div');
  next.innerHTML = html;
  const context = { previousSource, source,
    hasPresentationTransform: Boolean(previousTransform || sourceHtmlTransform || previousFiltered || sourceHtmlFiltered),
    previousBlockState: previousBlockState?.source === previousSource ? previousBlockState : null,
    previousBlockProbe: previous.probe,
    isLiteralHtml: markdownHtmlTagIsLiteral, isClosedCode: markdownCodeIsClosed, sourceTokens: () => markdownSourceTokens(source),
    previousSourceTokens: () => markdownSourceTokens(previousSource),
    sourceDocument: container.ownerDocument,
    retireFilteredCode: sourceHtmlFiltered && Boolean(container.querySelector('pre')),
    renderSourceProbe: (tokens, decorate) => {
      const diagnostic = renderMarkdownSourceProbe(tokens, decorate);
      // Provenance belongs to the previous displayed frame, including its
      // presentation folds. Completion or locale changes may use a new fold.
      return diagnostic === null || !previousTransform ? diagnostic : previousTransform(diagnostic);
    },
    renderCurrentSourceProbe: (tokens, decorate) => {
      const diagnostic = renderMarkdownSourceProbe(tokens, decorate);
      return diagnostic === null || !sourceHtmlTransform ? diagnostic : sourceHtmlTransform(diagnostic);
    },
    definitionTitles: () => markdownDefinitionTitles(source) };
  // Markdown itself can remove source blocks inside an image label. Code
  // feedback and table endpoint hints need provenance before kind matching,
  // even when no presentation filter is active.
  const hasSources = typeof previousSource === 'string' && typeof source === 'string';
  const keys = (hasSources || sourceHtmlFiltered || previousFiltered) && container.querySelector(SOURCE_BLOCK_SELECTOR)
    ? sourceBlockKeys(container, next, context) : null;
  preserveRichTextSelection(container, () => {
    prepareStreamFadeUpdate(container);
    reconcileChildren(container, next, keys, sourceBlockMatches(container, keys));
  }, context);
  renderedFrames.set(container, {
    html, source: typeof source === 'string' ? source : undefined,
    transform: typeof sourceHtmlTransform === 'function' ? sourceHtmlTransform : undefined,
    filtered: sourceHtmlFiltered,
    blocks: context.nextBlockState ? { ...context.nextBlockState, keys } : undefined,
    probe: context.nextBlockProbe,
  });
}

/** Commit a conversation frame and own its controls/fade observer lifecycle. */
export function commitRichTextFrame(container, html, { streaming, copyLabels, ...sourceOptions }) {
  if (!container) return;
  updateRichText(container, html, sourceOptions);
  ensureCodeCopyButtons(container, copyLabels);
  // The previous effect disconnected its observer before this commit, so
  // reconciliation and one-shot fade mutations are not self-observed.
  applyStreamFade(container, { streaming, html });
  return observeCodeCopyButtons(container, copyLabels);
}
