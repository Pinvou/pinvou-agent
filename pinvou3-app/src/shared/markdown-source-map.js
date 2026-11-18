export function lexerSourceMap(source) {
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

export function composeMaps(inner, outer) {
  return inner.map(index => outer[index]);
}

// Marked 14+ keeps literal tabs in list/blockquote token raws while the mapped
// source text expands leading tabs to spaces, so raw subsequence matching fails.
// Expand tabs on the matched side with the same rule as lexerSourceMap and
// collapse the per-character match back onto the original string.
export function matchTokenText(value, parentText, cursor) {
  const raw = String(value);
  if (!raw.includes('\t')) {
    const map = subsequenceMap(raw, parentText, cursor);
    if (!map) return null;
    return { text: raw, offsets: raw.split('').map((_, index) => index), map };
  }
  const expansion = lexerSourceMap(raw);
  const expandedMap = subsequenceMap(expansion.text, parentText, cursor);
  if (!expandedMap) return null;
  const map = Array.from({length: raw.length}).fill(null);
  for (let index = 0; index < expandedMap.length; index += 1) {
    map[expansion.offsets[index]] = expandedMap[index];
  }
  let last = null;
  for (let index = 0; index < map.length; index += 1) {
    if (map[index] === null) map[index] = last;
    else last = map[index];
  }
  return { text: expansion.text, offsets: expansion.offsets, map };
}

// Turn a matchTokenText result into a recursion map: from character positions of
// the (possibly tab-expanded) matched text back to offsets in the original source.
export function tokenSourceMap(match, parentMap) {
  return composeMaps(composeMaps(match.offsets, match.map), parentMap);
}
