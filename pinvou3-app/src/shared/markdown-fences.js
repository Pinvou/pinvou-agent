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
    indent += match[0].length;
    depth += 1;
  }
  return { cursor, indent, depth };
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
    let spaces = 0;
    while (spaces < opening.listIndent && line.charAt(cursor + spaces) === ' ') spaces += 1;
    if (spaces < opening.listIndent) return null;
    cursor += spaces;
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
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const opening = containerFenceOpening(lines[lineIndex].text);
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
