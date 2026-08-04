import { Marked } from 'marked';

export const MARKDOWN_OPTIONS = Object.freeze({
  gfm: true,
  breaks: true,
  headerIds: false,
  mangle: false,
});
const markdownLexer = new Marked(MARKDOWN_OPTIONS);

function lexerSourceMap(source) {
  let text = '';
  const offsets = [];
  let indentPhase = 'spaces';
  for (let index = 0; index < source.length; index += 1) {
    const char = source.charAt(index);
    if (char === '\r') {
      if (source.charAt(index + 1) === '\n') index += 1;
      text += '\n';
      offsets.push(index);
      indentPhase = 'spaces';
    } else if (char === '\n') {
      text += char;
      offsets.push(index);
      indentPhase = 'spaces';
    } else if (char === '\t' && indentPhase !== 'done') {
      text += '    ';
      offsets.push(index, index, index, index);
      indentPhase = 'tabs';
    } else {
      text += char;
      offsets.push(index);
      if (indentPhase === 'tabs' || char !== ' ') indentPhase = 'done';
    }
  }
  return { text, offsets };
}

function subsequenceMap(derived, parent, start = 0) {
  const offsets = [];
  let cursor = start;
  for (let index = 0; index < derived.length; index += 1) {
    const char = derived.charAt(index);
    while (cursor < parent.length && parent.charAt(cursor) !== char) cursor += 1;
    if (cursor >= parent.length) return null;
    offsets.push(cursor);
    cursor += 1;
  }
  return offsets;
}

function composeMaps(inner, outer) {
  return inner.map(index => outer[index]);
}

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
    const relativeMap = subsequenceMap(token.raw, parentText, cursor);
    if (!relativeMap) {
      if (token.type === 'blockquote' && token.text && token.tokens) {
        const mappedText = token.text.endsWith('\n') ? token.text.slice(0, -1) : token.text;
        const textMap = subsequenceMap(mappedText, parentText, cursor);
        if (textMap) {
          cursor = textMap[textMap.length - 1] + 1;
          walkTokenSequence(
            token.tokens,
            mappedText,
            composeMaps(textMap, parentMap),
            source,
            fences,
          );
        }
      }
      continue;
    }
    cursor = relativeMap[relativeMap.length - 1] + 1;
    const tokenMap = composeMaps(relativeMap, parentMap);

    if (token.type === 'code' && token.codeBlockStyle !== 'indented') {
      const fence = mappedFence(token, tokenMap, source);
      if (fence) fences.push(fence);
      continue;
    }

    if (token.type === 'blockquote' && token.text && token.tokens) {
      const textMap = subsequenceMap(token.text, token.raw);
      if (textMap) {
        walkTokenSequence(token.tokens, token.text, composeMaps(textMap, tokenMap), source, fences);
      }
      continue;
    }

    if (token.type === 'list' && Array.isArray(token.items)) {
      let itemCursor = 0;
      for (const item of token.items) {
        if (!item?.raw || !item.text) continue;
        const itemRelativeMap = subsequenceMap(item.raw, token.raw, itemCursor);
        if (!itemRelativeMap) continue;
        itemCursor = itemRelativeMap[itemRelativeMap.length - 1] + 1;
        const itemMap = composeMaps(itemRelativeMap, tokenMap);
        const textMap = subsequenceMap(item.text, item.raw);
        if (textMap) {
          walkTokenSequence(item.tokens, item.text, composeMaps(textMap, itemMap), source, fences);
        }
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
