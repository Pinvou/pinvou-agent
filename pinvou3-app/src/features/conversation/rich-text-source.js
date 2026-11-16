import { tagEnd } from './rich-text-html-tags.js';
import { maskFilteredSourceCode, createSourceBlockKeys, cachedSourceBlockKeys, sourceBlockState, SOURCE_BLOCK_SELECTOR } from './rich-text-block-visibility.js';
import { createAnnotatedSourceMapper, visibleSourceTail } from './rich-text-source-probe.js';
import { lexerSourceMap, matchTokenText, tokenSourceMap, composeMaps } from '../../shared/markdown-source-map.js';
export { SOURCE_BLOCK_SELECTOR } from './rich-text-block-visibility.js';

const ENTITY = /&(?:#(?:x[\da-f]+|\d+)|[a-z][a-z\d]+);?/gi;


function hiddenHtml(raw, start, add, isLiteralHtml, decode) {
  // Raw HTML text is decoded by the browser, unlike escaped Markdown text.
  // Tag/comment spans dominate entity spans inside their hidden attributes.
  if (decode) for (const match of raw.matchAll(ENTITY)) {
    const label = decode(match[0]);
    if (label !== match[0]) add(start + match.index, start + match.index + match[0].length, label, true);
  }
  let position = 0;
  while (position < raw.length) {
    const open = raw.indexOf('<', position);
    if (open < 0) return;
    let end;
    if (raw.startsWith('<!--', open)) {
      const close = raw.indexOf('-->', open + 4);
      if (close < 0) return;
      end = close + 3;
    } else {
      if (!/^<\/?[a-z]|^<[!?]/i.test(raw.slice(open, open + 3))) { position = open + 1; continue; }
      end = tagEnd(raw, open);
      if (end < 0) return;
    }
    const tag = raw.slice(open, end);
    // The established sanitizer deliberately renders dangerous tags literally.
    if (!isLiteralHtml?.(tag)) add(start + open, start + end);
    position = end;
  }
}

function hiddenInlineToken(token, start, add, decode) {
  const raw = token.raw;
  const leading = length => add(start, start + length);
  if (['strong', 'em', 'del', 'codespan'].includes(token.type)) {
    const opening = raw.match(/^(?:\*+|_+|~+|`+)/)?.[0] || '';
    leading(opening.length);
    add(start + raw.length - opening.length, start + raw.length);
  } else if (token.type === 'escape') leading(1);
  else if (token.type === 'image') add(start, start + raw.length, undefined, true);
  else if (token.type === 'link') {
    if (raw.startsWith('<')) { leading(1); add(start + raw.length - 1, start + raw.length); }
    else if (raw.startsWith('[')) {
      let depth = 1;
      let close = 1;
      for (; close < raw.length; close += 1) {
        if (raw[close] === '\\') close += 1;
        else if (raw[close] === '[') depth += 1;
        else if (raw[close] === ']' && --depth === 0) break;
      }
      leading(1);
      add(start + close, start + raw.length);
    }
  }
  if (token.type !== 'text' || !decode) return;
  for (const match of raw.matchAll(ENTITY)) {
    // These text tokens are parser-decoded even inside raw HTML CODE/PRE.
    // Markdown code/codespan contents never take this entity path.
    // Marked escapes inline ampersands until the entity has a semicolon.
    if (!token.escaped && !match[0].endsWith(';')) continue;
    const label = decode(match[0]);
    if (label !== match[0]) add(start + match.index, start + match.index + match[0].length, label, true);
  }
}

function hiddenHeadingToken(token, start, add) {
  const raw = token.raw;
  if (!token.text) { add(start, start + raw.length); return; }
  const marker = raw.match(/^ {0,3}#{1,6}(?:[\t ]+|(?=\n|$))/)?.[0] || '';
  const content = matchTokenText(token.text, raw, marker.length);
  if (content?.map.length) {
    add(start, start + content.map[0]);
    add(start + content.map.at(-1) + 1, start + raw.length);
  } else add(start, start + marker.length);
}

function hiddenBlockToken(token, start, add) {
  const raw = token.raw;
  const leading = length => add(start, start + length);
  if (token.type === 'hr' || token.type === 'def') leading(raw.length);
  else if (token.type === 'heading') hiddenHeadingToken(token, start, add);
  else if (token.type === 'list_item') {
    const marker = raw.match(/^ {0,3}(?:[-+*]|\d+[.)])(?:[\t ]+|(?=\n|$))/)?.[0];
    if (marker) {
      const task = token.task ? raw.slice(marker.length).match(/^\[[ xX]\](?:[\t ]+|(?=\n|$))/)?.[0] : '';
      leading(marker.length + (task?.length || 0));
    }
  } else if (token.type === 'blockquote') {
    for (const match of raw.matchAll(/^ {0,3}>[\t ]?/gm)) add(start + match.index, start + match.index + match[0].length);
  } else if (token.type === 'table') {
    const divider = raw.indexOf('\n') + 1;
    const end = raw.indexOf('\n', divider);
    add(start + divider, start + (end < 0 ? raw.length : end));
    for (let index = 0; index < raw.length; index += 1) {
      if (raw[index] === '\\') index += 1;
      else if (raw[index] === '|') add(start + index, start + index + 1);
    }
  } else if (token.type === 'code' && /^ {0,3}(?:`{3,}|~{3,})/.test(raw)) {
    const newline = raw.indexOf('\n');
    leading(newline < 0 ? raw.length : newline + 1);
    const opening = raw.match(/^ {0,3}(`{3,}|~{3,})/)[1];
    const closing = raw.match(/\n {0,3}(`{3,}|~{3,})[ \t]*(?:\n)?$/);
    if (closing && closing[1][0] === opening[0] && closing[1].length >= opening.length) {
      add(start + closing.index, start + raw.length, undefined, true);
    }
  }
}

// Reuse the UI's existing newline/tab and container source maps. A cursor
// advances across siblings, including raw-less table cells, so each cell does
// not rescan the table. Completed source before the literal tail is skipped.
function containsImage(token, cache) {
  if (cache.has(token)) return cache.get(token);
  const children = [...(token.tokens || []), ...(token.items || [])];
  if (token.type === 'table') children.push(...(token.header || []), ...(token.rows || []).flat());
  const found = token.type === 'image' || children.some(child => containsImage(child, cache));
  cache.set(token, found);
  return found;
}

function sourceSpans(source, tokens, context, lowerBound) {
  const spans = [];
  const images = new WeakMap();
  const visit = (sequence, parentText, parentMap, initialCursor = 0) => {
    let cursor = initialCursor;
    for (const token of sequence || []) {
      if (!token.raw) {
        cursor = visit(token.tokens, parentText, parentMap, cursor);
        continue;
      }
      const match = matchTokenText(token.raw, parentText, cursor);
      if (!match) continue;
      cursor = match.map.at(-1) + 1;
      const map = composeMaps(match.map, parentMap);
      context.captureToken?.(token, map);
      const inTail = map.at(-1) >= lowerBound;
      if (!inTail && !containsImage(token, images)) continue;
      const add = (left, right, label, retireLiteral = false) => {
        if (right <= left) return;
        spans.push({ start: map[left], end: map[right - 1] + 1, label, retireLiteral, image: token.type === 'image', html: token.type === 'html',
          raw: token.raw.slice(left, right), at: index => map[left + index] });
      };
      if (inTail || token.type === 'image') {
        hiddenInlineToken(token, 0, add, context.decodeEntity);
        hiddenBlockToken(token, 0, add);
        if (token.type === 'html') hiddenHtml(token.raw, 0, add, context.isLiteralHtml, context.decodeEntity);
      }
      let text = match.text;
      let offsets = tokenSourceMap(match, parentMap);
      if (['blockquote', 'list_item'].includes(token.type) && typeof token.text === 'string') {
        const content = matchTokenText(token.text, text, 0);
        if (!content) continue;
        text = content.text;
        offsets = tokenSourceMap(content, offsets);
      }
      visit(token.tokens, text, offsets);
      visit(token.items, text, offsets);
      if (token.type === 'table') visit([...(token.header || []), ...(token.rows || []).flat()], text, offsets);
    }
    return cursor;
  };
  const normalized = lexerSourceMap(source);
  visit(tokens, normalized.text, normalized.offsets);
  return spans;
}

function withoutWhitespace(text) {
  let value = '';
  const positions = [];
  for (let index = 0; index < text.length; index += 1) {
    if (/\s/.test(text[index])) continue;
    value += text[index];
    positions.push(index);
  }
  return { value, positions };
}

// Project only syntax which was already hidden by the previous parse. This
// preserves literal delimiters while accounting for formatting inside a label
// that becomes an image during this update.
function previousVisibleFragment(previous, start, end, spans) {
  let fragment = '';
  let cursor = start;
  for (const span of spans) {
    if (span.end <= cursor || span.start >= end) continue;
    fragment += previous.slice(cursor, Math.max(cursor, span.start));
    if (span.start >= cursor && span.end <= end) fragment += span.label || '';
    cursor = Math.min(end, span.end);
  }
  return fragment + previous.slice(cursor, end);
}

// Build one exact, whitespace-free projection from the previous parser. Its
// source positions are validated against unchanged DOM context before use. This covers image definitions that change an earlier paragraph,
// including formatted labels followed by new literal duplicates.
function previousSourceProjection(previous, spans) {
  let value = '';
  const positions = [];
  let cursor = 0;
  const append = (text, sourceAt) => {
    for (let index = 0; index < text.length; index += 1) {
      if (/\s/.test(text[index])) continue;
      value += text[index];
      positions.push(sourceAt(index));
    }
  };
  for (const span of spans) {
    if (span.end <= cursor) continue;
    if (span.start > cursor) {
      const start = cursor;
      append(previous.slice(start, span.start), index => start + index);
    }
    if (span.start >= cursor && span.label) append(span.label, () => span.start);
    cursor = span.end;
  }
  append(previous.slice(cursor), index => cursor + index);
  return { value, positions };
}

function sourceBoundary(positions, sourceOffset) {
  let left = 0;
  let right = positions.length;
  while (left < right) {
    const middle = (left + right) >>> 1;
    if (positions[middle] < sourceOffset) left = middle + 1;
    else right = middle;
  }
  return left;
}

function createImageRetirementMapper(previous, actual, context, getPrevious, images) {
  let projection;
  let annotated;
  let state;
  let existingImages;
  return (span, mask) => {
    if (!span.image || span.start >= previous.length || !context.previousSourceTokens) return false;
    if (!state) {
      state = getPrevious();
      existingImages = new Set(state.spans.filter(item => item.image).map(item => `${item.start}:${item.end}`));
    }
    // Already-rendered images had no old label text to retire. Avoid a second
    // diagnostic parse for the normal case of streaming after existing images.
    if (existingImages.has(`${span.start}:${span.end}`)) return true;
    if (!projection) {
      projection = previousSourceProjection(previous, state.spans);
      annotated = createAnnotatedSourceMapper(state.tokens, state.origins, projection, actual, context, images);
    }
    if (annotated) {
      annotated(span.start, span.end, mask);
      return true;
    }
    // Pure non-DOM callers can still trust an exact complete projection.
    if (projection.value === actual.value) {
      const start = sourceBoundary(projection.positions, span.start);
      const end = sourceBoundary(projection.positions, span.end);
      if (start < end) mask.fill(1, actual.positions[start], actual.positions[end - 1] + 1);
    }
    return false;
  };
}

function sourceTailStart(span, sourceEnd, actual, decode, projectPrevious) {
  if (span.start >= sourceEnd || span.end < sourceEnd || (span.end === sourceEnd && !span.image)) return -1;
  let end = 0;
  while (end < span.raw.length && span.at(end) < sourceEnd) end += 1;
  let fragment = span.raw.slice(0, end);
  if (span.label === fragment) return -1;
  if (decode) fragment = fragment.replaceAll(ENTITY, value => decode(value));
  const expected = withoutWhitespace(fragment).value;
  if (!expected) return -1;
  if (actual.value.endsWith(expected)) return actual.positions[actual.value.length - expected.length];
  const projected = withoutWhitespace(projectPrevious(span.start, sourceEnd)).value;
  return projected && actual.value.endsWith(projected) ? actual.positions[actual.value.length - projected.length] : -1;
}

function unownedHtmlPrefix(span, sourceEnd, context, previousState) {
  if (!span.html || span.label !== undefined || span.start >= sourceEnd || span.end <= sourceEnd) return false;
  return visibleSourceTail(sourceEnd - 1, sourceEnd, context, previousState) === null;
}

function maskDisplayedEntity(span, fragment, displayed, context, mask) {
  const values = [context.decodeEntity(fragment), fragment];
  for (const value of values) {
    if (!value) continue;
    const start = displayed.text.lastIndexOf(value);
    if (start < 0 || !/^\s*$/.test(displayed.text.slice(start + value.length))) continue;
    if (!span.label.startsWith(value)) mask.fill(1, displayed.start + start, displayed.start + start + value.length);
    return true;
  }
  return true;
}

function maskDecodedEntityPrefix(span, previous, sourceEnd, visible, context, mask, previousState) {
  if (span.label === undefined || span.start >= sourceEnd || span.end <= sourceEnd || !context.decodeEntity) return false;
  const fragment = previous.slice(span.start, sourceEnd);
  const displayed = visibleSourceTail(span.start, sourceEnd, context, previousState);
  if (displayed === null) return true;
  if (displayed) return maskDisplayedEntity(span, fragment, displayed, context, mask);
  const decoded = context.decodeEntity(fragment);
  if (!decoded || decoded === fragment) return false;
  // Unclosed inline HTML can leave browser-reconstructed code around a
  // paragraph newline. That layout suffix is outside the entity value.
  const start = visible.lastIndexOf(decoded);
  if (start < 0 || !/^\s*$/.test(visible.slice(start + decoded.length))) return false;
  // A decoded prefix can remain stable: &copy -> &copycat; renders © -> ©cat;.
  if (!span.label.startsWith(decoded)) mask.fill(1, start, start + decoded.length);
  return true;
}

// Markdown resolution or presentation folds can replace blocks at the same
// DOM position. Keep controls and table endpoint hints with their source.
function distinctBlockKeys(previousRoot, root) {
  const keys = new WeakMap();
  for (const [side, host] of [['before', previousRoot], ['after', root]]) {
    for (const [index, node] of [...host.querySelectorAll(SOURCE_BLOCK_SELECTOR)].entries()) keys.set(node, `${side}:${index}`);
  }
  return keys;
}

function isSourceAppend(context) {
  return typeof context.previousSource === 'string' && typeof context.source === 'string'
    && context.source.startsWith(context.previousSource);
}

export function sourceBlockKeys(previousRoot, root, context) {
  if (typeof context.previousSource !== 'string' || typeof context.source !== 'string') return distinctBlockKeys(previousRoot, root);
  const tokens = context.sourceTokens();
  const cached = cachedSourceBlockKeys(previousRoot, root, tokens, context);
  if (cached) return cached;
  const collect = (source, sequence) => {
    const origins = new WeakMap();
    sourceSpans(source, sequence, { ...context,
      captureToken: (token, map) => origins.set(token, { start: map[0], end: map.at(-1) + 1,
        at: token.type === 'html' ? index => map[index] : undefined }),
    }, 0);
    return origins;
  };
  const keys = createSourceBlockKeys(() => {
    const previousTokens = context.previousSourceTokens();
    return { tokens: previousTokens, origins: collect(context.previousSource, previousTokens) };
  }, tokens, collect(context.source, tokens), context, previousRoot, root);
  if (keys) context.nextBlockState = sourceBlockState(root, tokens, context, keys);
  return keys || distinctBlockKeys(previousRoot, root);
}

function hasSourceSyntax(visible, previous) {
  // A raw HTML entity may already be decoded to an ordinary visible letter.
  return previous.includes('&') || /[0-9<>&\\*_~`[\]#+|=-]/.test(visible);
}

function maskSourceRange(mask, start, end, literal, retireLiteral) {
  for (let index = start; index < end; index += 1) {
    if (retireLiteral || !literal[index]) mask[index] = 1;
  }
}

/** Retire source-owned syntax in an unchanged literal tail during append. */
export function maskRetiredSourceSyntax(before, context, mask) {
  const previous = context.previousSource;
  const source = context.source;
  if (!isSourceAppend(context) || !context.sourceTokens) return;
  const visibleEnd = (context.before?.literal || []).reduce((end, range) => Math.max(end, range.end), before.trimEnd().length);
  const visible = before.slice(0, visibleEnd);
  const oldSource = previous.trimEnd();
  let suffix = 0;
  while (suffix < visible.length && suffix < oldSource.length
    && visible[visible.length - suffix - 1] === oldSource[oldSource.length - suffix - 1]) suffix += 1;
  const sourceStart = oldSource.length - suffix;
  const visibleStart = visible.length - suffix;
  if (!context.retireFilteredCode && !hasSourceSyntax(visible, previous)) return;
  const literal = new Uint8Array(before.length);
  for (const range of context.before?.literal || []) literal.fill(1, range.start, range.end);
  const actual = withoutWhitespace(visible);
  let previousSpans;
  let previousTokens;
  const origins = new WeakMap();
  const oldSpans = () => {
    previousTokens ??= context.previousSourceTokens();
    previousSpans ??= sourceSpans(previous, previousTokens, { ...context, captureToken: (token, map) => origins.set(token, { start: map[0], end: map.at(-1) + 1, at: token.type === 'html' ? index => map[index] : undefined }) }, 0)
      .sort((left, right) => left.start - right.start || right.end - left.end);
    return previousSpans;
  };
  const projectPrevious = (start, end) => context.previousSourceTokens
    ? previousVisibleFragment(previous, start, end, oldSpans()) : '';

  const tokens = context.sourceTokens();
  if (context.retireFilteredCode && context.previousSourceTokens) {
    oldSpans();
    const currentOrigins = new WeakMap();
    sourceSpans(source, tokens, { ...context,
      captureToken: (token, map) => currentOrigins.set(token, { start: map[0], end: map.at(-1) + 1, at: token.type === 'html' ? index => map[index] : undefined }),
    }, 0);
    maskFilteredSourceCode(previousTokens, origins, tokens, currentOrigins, context, mask);
  }
  const spans = sourceSpans(source, tokens, context, sourceStart);
  const images = spans.filter(span => span.image && span.start < previous.length);
  const maskImage = createImageRetirementMapper(previous, actual, context, () => ({ spans: oldSpans(), tokens: previousTokens, origins }), images);
  const previousState = () => { oldSpans(); return { tokens: previousTokens, origins }; };
  for (const span of spans) {
    if (maskImage(span, mask) || unownedHtmlPrefix(span, oldSource.length, context, previousState)) continue;
    if (maskDecodedEntityPrefix(span, previous, oldSource.length, visible, context, mask, previousState)) continue;
    const partial = sourceTailStart(span, oldSource.length, actual, context.decodeEntity, projectPrevious);
    if (partial >= 0) {
      maskSourceRange(mask, partial, visible.length, literal, span.retireLiteral);
    }
    const start = Math.max(span.start, sourceStart);
    const end = Math.min(span.end, oldSource.length);
    if (start >= end) continue;
    const mapped = visibleStart + start - sourceStart;
    // Entity completion can leave an identical visible character, e.g. &amp;.
    if (span.label === visible.slice(mapped, mapped + end - start)) continue;
    maskSourceRange(mask, mapped, mapped + end - start, literal, span.retireLiteral);
  }
}
