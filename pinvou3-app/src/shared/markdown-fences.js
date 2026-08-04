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
 * Locate top-level CommonMark fenced code blocks without rendering their content.
 * Offsets refer to the original string; content follows CommonMark's opening-indent removal.
 */
export function scanMarkdownFences(value) {
  const source = String(value || '');
  const lines = sourceLines(source);
  const fences = [];
  for (let lineIndex = 0; lineIndex < lines.length; lineIndex += 1) {
    const opening = openingFence(lines[lineIndex].text);
    if (!opening) continue;
    let closingIndex = -1;
    for (let candidate = lineIndex + 1; candidate < lines.length; candidate += 1) {
      if (closingFence(lines[candidate].text, opening)) {
        closingIndex = candidate;
        break;
      }
    }
    const contentEnd = closingIndex >= 0 ? closingIndex : lines.length;
    const content = lines
      .slice(lineIndex + 1, contentEnd)
      .map(line => stripOpeningIndent(line.text, opening.indent))
      .join('\n');
    fences.push({
      start: lines[lineIndex].start,
      end: closingIndex >= 0 ? lines[closingIndex].end : source.length,
      info: opening.info,
      content,
      marker: opening.marker,
      markerLength: opening.length,
      closed: closingIndex >= 0,
    });
    lineIndex = closingIndex >= 0 ? closingIndex : lines.length;
  }
  return fences;
}
