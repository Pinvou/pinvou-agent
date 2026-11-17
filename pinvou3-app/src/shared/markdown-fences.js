/* eslint-disable sonarjs/super-linear-regex, unicorn/prefer-number-is-safe-integer, sonarjs/cognitive-complexity, import-x/namespace -- existing fence-parsing algorithm; regex behavior pinned by tests */
import { Marked } from 'marked';
import { lexerSourceMap, matchTokenText, tokenSourceMap, composeMaps } from './markdown-source-map.js';

export const MARKDOWN_OPTIONS = Object.freeze({
  gfm: true,
  breaks: true,
});
const markdownLexer = new Marked(MARKDOWN_OPTIONS);

function lineStart(source, index) {
  let cursor = index;
  while (cursor > 0 && !['\r', '\n'].includes(source.charAt(cursor - 1))) cursor -= 1;
  return cursor;
}

function lineEnd(source, index) {
  let cursor = index;
  while (cursor < source.length && !['\r', '\n'].includes(source.charAt(cursor))) cursor += 1;
  return cursor;
}

function openingFence(raw) {
  const firstLine = String(raw || '').split('\n', 1)[0];
  const match = /^( {0,3})(`{3,}|~{3,})([^\n]*)$/.exec(firstLine);
  if (!match) return null;
  return {
    indent: match[1].length,
    marker: match[2].charAt(0),
    length: match[2].length,
  };
}

function fenceIsClosed(raw, opening) {
  const lines = String(raw || '').replace(/\n$/, '').split('\n');
  if (lines.length < 2) return false;
  const closing = /^( {0,3})(`{3,}|~{3,})[ \t]*$/.exec(lines[lines.length - 1]);
  return Boolean(
    closing
    && closing[2].charAt(0) === opening.marker
    && closing[2].length >= opening.length,
  );
}

function mappedFence(token, tokenMap, source) {
  const opening = openingFence(token.raw);
  if (!opening || !tokenMap.length) return null;
  const markerOffset = opening.indent;
  const markerSourceOffset = tokenMap[markerOffset];
  if (!Number.isInteger(markerSourceOffset)) return null;
  const start = lineStart(source, markerSourceOffset);
  const end = lineEnd(source, tokenMap[tokenMap.length - 1]);
  return {
    start,
    end,
    info: String(token.lang || '').trim(),
    content: String(token.text || ''),
    marker: opening.marker,
    markerLength: opening.length,
    closed: fenceIsClosed(token.raw, opening),
    nested: markerSourceOffset - start > opening.indent,
  };
}

function walkTokenSequence(tokens, parentText, parentMap, source, fences) {
  let cursor = 0;
  for (const token of tokens || []) {
    if (!token?.raw) continue;
    let match = matchTokenText(token.raw, parentText, cursor);
    if (!match) {
      if (token.type === 'blockquote' && token.text && token.tokens) {
        const mappedText = token.text.endsWith('\n') ? token.text.slice(0, -1) : token.text;
        match = matchTokenText(mappedText, parentText, cursor);
        if (match) {
          cursor = match.map[match.map.length - 1] + 1;
          walkTokenSequence(token.tokens, match.text, tokenSourceMap(match, parentMap), source, fences);
        }
      }
      continue;
    }
    cursor = match.map[match.map.length - 1] + 1;
    const frameMap = tokenSourceMap(match, parentMap);
    const tokenMap = composeMaps(match.map, parentMap);

    if (token.type === 'code' && token.codeBlockStyle !== 'indented') {
      const fence = mappedFence(token, tokenMap, source);
      if (fence) fences.push(fence);
      continue;
    }

    if (token.type === 'blockquote' && token.text && token.tokens) {
      const textMatch = matchTokenText(token.text, match.text, 0);
      if (textMatch) {
        walkTokenSequence(
          token.tokens,
          textMatch.text,
          tokenSourceMap(textMatch, frameMap),
          source,
          fences,
        );
      }
      continue;
    }

    if (token.type !== 'list' || !Array.isArray(token.items)) continue;
    let itemCursor = 0;
    for (const item of token.items) {
      if (!item?.raw || !item.text) continue;
      const itemMatch = matchTokenText(item.raw, match.text, itemCursor);
      if (!itemMatch) continue;
      itemCursor = itemMatch.map[itemMatch.map.length - 1] + 1;
      const itemFrameMap = tokenSourceMap(itemMatch, frameMap);
      const textMatch = matchTokenText(item.text, itemMatch.text, 0);
      if (textMatch) {
        walkTokenSequence(
          item.tokens,
          textMatch.text,
          tokenSourceMap(textMatch, itemFrameMap),
          source,
          fences,
        );
      }
    }
  }
}

/**
 * Return only fenced blocks recognized by the same Marked grammar used by the UI.
 * The token tree is authoritative; subsequence maps recover offsets in the original
 * Markdown after Marked removes blockquote/list prefixes and expands tabs.
 */
export function scanMarkdownFences(value) {
  const source = String(value || '');
  if (!source) return [];
  const mappedSource = lexerSourceMap(source);
  const fences = [];
  walkTokenSequence(
    markdownLexer.lexer(source),
    mappedSource.text,
    mappedSource.offsets,
    source,
    fences,
  );
  return fences.sort((left, right) => left.start - right.start);
}
