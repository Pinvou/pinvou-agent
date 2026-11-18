const HTML_SPACE = /[\t\n\f\r ]/;

function attributeState(state, character) {
  const space = HTML_SPACE.test(character);
  if (state === 'tag') return space || character === '/' ? 'before' : state;
  if (state === 'before' || state === 'after-quoted') return space || character === '/' ? 'before' : 'name';
  if (state === 'name') {
    if (space) return 'after-name';
    return character === '=' ? 'value' : character === '/' ? 'before' : state;
  }
  if (state === 'after-name') return space ? state : character === '=' ? 'value' : character === '/' ? 'before' : 'name';
  if (state === 'unquoted') return space ? 'before' : state;
  return state;
}

export function tagEnd(raw, open) {
  // HTML bogus comments ignore quotes and end at the first greater-than.
  if (raw.startsWith('<?', open) || /^<!(?!doctype\b)/i.test(raw.slice(open))) {
    const end = raw.indexOf('>', open + 2);
    return end < 0 ? -1 : end + 1;
  }
  let quote = '';
  let state = 'tag';
  for (let end = open + 1; end < raw.length; end += 1) {
    const character = raw[end];
    if (quote) {
      if (character === quote) { quote = ''; state = 'after-quoted'; }
      continue;
    }
    if (character === '>') return end + 1;
    if (state === 'value') {
      if (HTML_SPACE.test(character)) continue;
      if (character === '"' || character === "'") quote = character;
      else state = 'unquoted';
    } else state = attributeState(state, character);
  }
  return -1;
}

const RAW_TEXT = new Set(['script', 'style', 'iframe', 'xmp', 'noembed', 'noframes', 'noscript', 'textarea', 'title', 'plaintext']);

function specialMarkupEnd(html, start) {
  if (html.startsWith('<!--', start)) {
    if (html.startsWith('<!-->', start)) return start + 5;
    if (html.startsWith('<!--->', start)) return start + 6;
    const closing = /--!?>/g;
    closing.lastIndex = start + 4;
    return closing.exec(html) ? closing.lastIndex : -1;
  }
  return null;
}

function rawTextClose(html, name, cursor) {
  if (name === 'plaintext') return -1;
  const closing = new RegExp(String.raw`</${name}(?=[\t\n\f\r />])`, 'gi');
  closing.lastIndex = cursor;
  return closing.exec(html)?.index ?? -1;
}

// Scan tag boundaries without treating quoted attributes, comments or raw
// text as markup. Dangerous tags made literal by the renderer are exempt.
function sourceTagOffsets(html, isLiteralHtml, names) {
  const positions = [];
  let cursor = 0;
  while (cursor < html.length) {
    const start = html.indexOf('<', cursor);
    if (start < 0) break;
    const specialEnd = specialMarkupEnd(html, start);
    if (specialEnd !== null) {
      if (specialEnd < 0) break;
      cursor = specialEnd;
      continue;
    }
    const match = /^<(\/?)([a-z][a-z0-9:-]*)(?=[\t\n\f\r />])/i.exec(html.slice(start));
    if (!match && html[start + 1] !== '!' && html[start + 1] !== '?') { cursor = start + 1; continue; }
    const end = tagEnd(html, start);
    if (end < 0) break;
    if (!match) { cursor = end; continue; }
    const name = match[2].toLowerCase();
    if (!match[1] && names.has(name)) positions.push({ offset: start, name });
    cursor = end;
    if (match[1] || !RAW_TEXT.has(name) || isLiteralHtml?.(html.slice(start, end))) continue;
    const close = rawTextClose(html, name, cursor);
    if (close < 0) break;
    cursor = close;
  }
  return positions;
}

export function preTagOffsets(html, isLiteralHtml) {
  return sourceTagOffsets(html, isLiteralHtml, new Set(['pre'])).map(tag => tag.offset);
}

export function sourceBlockTagOffsets(html, isLiteralHtml) {
  return sourceTagOffsets(html, isLiteralHtml, new Set(['pre', 'table', 'td', 'th', 'caption']));
}
