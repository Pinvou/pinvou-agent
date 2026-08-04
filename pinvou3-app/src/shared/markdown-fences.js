function sourceLines(source) {
  const lines = [];
  let start = 0;
  for (let index = 0; index <= source.length; index += 1) {
    if (index !== source.length && source.charAt(index) !== '\n') continue;
    const contentEnd = index > start && source.charAt(index - 1) === '\r' ? index - 1 : index;
    lines.push({ start, end: index, text: source.slice(start, contentEnd) });
    start = index + 1;
  }
  return lines;
}

function openingFence(line) {
  const match = /^( {0,3})(`{3,}|~{3,})([^\n]*)$/.exec(line);
  if (!match) return null;
  const marker = match[2];
  const info = match[3] || '';
  if (marker.charAt(0) === '`' && info.includes('`')) return null;
  return {
    indent: match[1].length,
    marker: marker.charAt(0),
    length: marker.length,
    info: info.trim(),
  };
}

function consumeBlockquotePrefix(line, start, expectedDepth = null) {
  let cursor = start;
  let depth = 0;
  while (expectedDepth == null || depth < expectedDepth) {
    const match = /^( {0,3})>[ \t]?/.exec(line.slice(cursor));
    if (!match) break;
    cursor += match[0].length;
    depth += 1;
  }
  return expectedDepth == null || depth === expectedDepth ? { cursor, depth } : null;
}

function consumeListPrefix(line, start) {
  let cursor = start;
  let indent = 0;
  let depth = 0;
  while (true) {
    const match = /^( {0,3})(?:[*+-]|\d{1,9}[.)])([ \t]{1,4})/.exec(line.slice(cursor));
    if (!match) break;
    cursor += match[0].length;
    indent = visualWidth(line.slice(start, cursor));
    depth += 1;
  }
  return { cursor, indent, depth };
}

function visualWidth(value) {
  let width = 0;
  for (const char of String(value || '')) {
    width += char === '\t' ? 4 - (width % 4) : 1;
  }
  return width;
}

function leadingIndentWidth(value) {
  const match = /^[ \t]*/.exec(String(value || ''));
  return visualWidth(match[0]);
}

function stripIndent(value, width) {
  const source = String(value || '');
  let cursor = 0;
  let consumed = 0;
  while (cursor < source.length && consumed < width) {
    const char = source.charAt(cursor);
    if (char !== ' ' && char !== '\t') break;
    consumed += char === '\t' ? 4 - (consumed % 4) : 1;
    cursor += 1;
  }
  return consumed >= width ? source.slice(cursor) : null;
}

function listMarker(line) {
  const match = /^( {0,3})(?:[*+-]|\d{1,9}[.)])([ \t]{1,4})/.exec(line);
  if (!match) return null;
  return {
    markerIndent: visualWidth(match[1]),
    contentIndent: visualWidth(match[0]),
  };
}

const LIST_INTERRUPTING_HTML_TAGS = new Set([
  'address', 'article', 'aside', 'base', 'basefont', 'blockquote', 'body', 'caption', 'center',
  'col', 'colgroup', 'dd', 'details', 'dialog', 'dir', 'div', 'dl', 'dt', 'fieldset',
  'figcaption', 'figure', 'footer', 'form', 'frame', 'frameset', 'h1', 'h2', 'h3', 'h4',
  'h5', 'h6', 'head', 'header', 'hr', 'html', 'legend', 'li', 'main', 'menu', 'menuitem',
  'nav', 'noframes', 'ol', 'optgroup', 'option', 'p', 'param', 'search', 'section', 'summary',
  'table', 'tbody', 'td', 'tfoot', 'th', 'thead', 'title', 'tr', 'track', 'ul',
]);

function startsListInterruptingBlock(content) {
  const line = String(content || '');
  if (/^(?: {0,3})(?:#{1,6}(?:[ \t]+|$)|`{3,}|~{3,}|(?:\*[ \t]*){3,}|(?:_[ \t]*){3,}|(?:-[ \t]*){3,})/.test(line)) {
    return true;
  }
  const html = /^(?: {0,3})<([a-z][\w-]*)(?:[ \t\n/>]|$)/i.exec(line);
  return Boolean(html && LIST_INTERRUPTING_HTML_TAGS.has(html[1].toLowerCase()));
}

function updateListContexts(line, contexts) {
  const quote = consumeBlockquotePrefix(line, 0);
  const content = line.slice(quote.cursor);
  const marker = listMarker(content);
  let next = contexts.filter(context => context.quoteDepth <= quote.depth);
  if (marker) {
    next = next.filter(context => context.markerIndent < marker.markerIndent);
    next.push({ ...marker, quoteDepth: quote.depth, afterBlank: false });
    return next;
  }
  if (!content.trim()) {
    return next.map(context => ({ ...context, afterBlank: true }));
  }
  const indent = leadingIndentWidth(content);
  if (startsListInterruptingBlock(content)) {
    next = next.filter(context => indent >= context.contentIndent);
  }
  return next.filter(context => indent >= context.contentIndent || !context.afterBlank)
    .map(context => ({ ...context, afterBlank: false }));
}

function listContinuationOpening(line, contexts) {
  const quote = consumeBlockquotePrefix(line, 0);
  const candidates = contexts
    .filter(context => context.quoteDepth <= quote.depth)
    .sort((left, right) => right.contentIndent - left.contentIndent);
  for (const context of candidates) {
    const content = stripIndent(line.slice(quote.cursor), context.contentIndent);
    if (content == null) continue;
    const opening = openingFence(content);
    if (opening) {
      return {
        ...opening,
        quoteDepth: quote.depth,
        listIndent: context.contentIndent,
        nested: true,
      };
    }
  }
  return null;
}

function containerFenceOpening(line) {
  const quote = consumeBlockquotePrefix(line, 0);
  const list = consumeListPrefix(line, quote.cursor);
  const opening = openingFence(line.slice(list.cursor));
  return opening ? {
    ...opening,
    quoteDepth: quote.depth,
    listIndent: list.indent,
    nested: quote.depth > 0 || list.depth > 0,
  } : null;
}

function containerLineContent(line, opening) {
  const quote = consumeBlockquotePrefix(line, 0, opening.quoteDepth);
  if (!quote) return null;
  let cursor = quote.cursor;
  if (opening.listIndent > 0 && line.slice(cursor).trim()) {
    const stripped = stripIndent(line.slice(cursor), opening.listIndent);
    if (stripped == null) return null;
    return stripped;
  }
  return line.slice(cursor);
}

function closingFence(line, opening) {
  const match = /^( {0,3})(`{3,}|~{3,})[ \t]*$/.exec(line);
  return Boolean(
    match
    && match[2].charAt(0) === opening.marker
    && match[2].length >= opening.length,
  );
}

function stripOpeningIndent(line, count) {
  let removed = 0;
  while (removed < count && line.charAt(removed) === ' ') removed += 1;
  return line.slice(removed);
}

/**
 * Locate CommonMark fenced code blocks, including blockquote and list containers,
 * without rendering their content.
 * Offsets refer to the original string; content follows CommonMark's opening-indent removal.
 */
export function scanMarkdownFences(value) {
  const source = String(value || '');
  const lines = sourceLines(source);
  const fences = [];
  let listContexts = [];
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    listContexts = updateListContexts(lines[lineIndex].text, listContexts);
    const opening = containerFenceOpening(lines[lineIndex].text)
      || listContinuationOpening(lines[lineIndex].text, listContexts);
    if (!opening) continue;
    let closingIndex = -1;
    let boundaryIndex = lines.length;
    const contentLines = [];
    for (let candidate = lineIndex + 1; candidate < lines.length; candidate += 1) {
      const content = containerLineContent(lines[candidate].text, opening);
      if (content == null) {
        boundaryIndex = candidate;
        break;
      }
      if (closingFence(content, opening)) {
        closingIndex = candidate;
        break;
      }
      contentLines.push(stripOpeningIndent(content, opening.indent));
    }
    const lastContentIndex = boundaryIndex < lines.length ? boundaryIndex - 1 : lines.length - 1;
    fences.push({
      start: lines[lineIndex].start,
      end: closingIndex >= 0
        ? lines[closingIndex].end
        : lastContentIndex > lineIndex ? lines[lastContentIndex].end : lines[lineIndex].end,
      info: opening.info,
      content: contentLines.join('\n'),
      marker: opening.marker,
      markerLength: opening.length,
      closed: closingIndex >= 0,
      nested: opening.nested,
    });
    lineIndex = closingIndex >= 0
      ? closingIndex
      : boundaryIndex < lines.length ? boundaryIndex - 1 : lines.length;
  }
  return fences;
}
