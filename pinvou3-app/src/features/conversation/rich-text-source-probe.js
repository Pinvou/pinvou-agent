// Source decorations are rendered only into a detached, sanitized diagnostic
// document. Unlike text similarity, surviving annotations retain ownership when
// the HTML parser moves nodes or the sanitizer removes equal occurrences.
function boundary(positions, point) {
  let left = 0;
  let right = positions.length;
  while (left < right) {
    const middle = (left + right) >>> 1;
    if (positions[middle] < point) left = middle + 1;
    else right = middle;
  }
  return left;
}

function visibleText(node) {
  if (node.nodeType === 3) return node.data;
  let text = '';
  for (const child of node.childNodes) text += visibleText(child);
  return text;
}

function readDecorations(text, pattern) {
  let value = '';
  const records = [];
  let active = null;
  let cursor = 0;
  const append = part => {
    const plain = part.replaceAll(/\s/g, '');
    value += plain;
    if (active) active.value += plain;
  };
  for (const match of text.matchAll(pattern)) {
    append(text.slice(cursor, match.index));
    const id = Number(match[1]);
    if (match[2] === 's') active = { id, start: value.length, value: '' };
    else {
      if (active?.id === id) records.push(active);
      active = null;
    }
    cursor = match.index + match[0].length;
  }
  append(text.slice(cursor));
  return { value, records };
}

function ownedCharacters(records, tokens, origins, projection) {
  const entries = [];
  for (const record of records) {
    const origin = origins.get(tokens[record.id]);
    if (!origin) continue;
    const start = boundary(projection.positions, origin.start);
    const end = boundary(projection.positions, origin.end);
    if (record.value !== projection.value.slice(start, end)) continue;
    for (let index = start; index < end; index += 1) {
      entries.push({ source: projection.positions[index], dom: record.start + index - start });
    }
  }
  return entries.sort((left, right) => left.source - right.source || left.dom - right.dom);
}

function imageIntersection(images) {
  const ranges = [];
  for (const image of [...images].sort((left, right) => left.start - right.start || right.end - left.end)) {
    const last = ranges.at(-1);
    if (last && image.start <= last.end) last.end = Math.max(last.end, image.end);
    else ranges.push({ start: image.start, end: image.end });
  }
  const ends = ranges.map(range => range.end);
  return origin => {
    const index = boundary(ends, origin.start + 1);
    return index < ranges.length && ranges[index].start < origin.end;
  };
}

export function createAnnotatedSourceMapper(tokens, origins, projection, actual, context, images) {
  if (!context.sourceDocument || !context.renderSourceProbe) return null;
  const crypto = context.sourceDocument.defaultView?.crypto;
  if (!crypto?.getRandomValues) return null;
  const nonce = crypto.getRandomValues(new Uint32Array(4));
  const prefix = `\uE000pinvou${Array.from(nonce, value => value.toString(16)).join('')}:`;
  const suffix = '\uE001';
  const decorated = [];
  const intersects = imageIntersection(images);
  const html = context.renderSourceProbe(tokens, (token, rendered) => {
    const origin = origins.get(token);
    if (token.type === 'html' || !origin || !intersects(origin) || !/\S/.test(token.text || '')) return rendered;
    const id = decorated.length;
    decorated.push(token);
    return `${prefix}${id}:s${suffix}${rendered}${prefix}${id}:e${suffix}`;
  });
  if (html === null) return null;
  const root = context.sourceDocument.createElement('div');
  root.innerHTML = html;
  const pattern = new RegExp(String.raw`${prefix}(\d+):([se])${suffix}`, 'g');
  const parsed = readDecorations(visibleText(root), pattern);
  // A diagnostic decoration must not change the actual visible text. No
  // inferred ownership is used if the parser handled decorated text differently.
  if (parsed.value !== actual.value) return null;
  const entries = ownedCharacters(parsed.records, decorated, origins, projection);
  const positions = entries.map(entry => entry.source);
  return (start, end, mask) => {
    const selected = entries.slice(boundary(positions, start), boundary(positions, end))
      .map(entry => actual.positions[entry.dom]).sort((left, right) => left - right);
    let previous = -1;
    for (const position of selected) {
      mask[position] = 1;
      if (previous >= 0 && !/\S/.test(context.before.text.slice(previous + 1, position))) mask.fill(1, previous, position + 1);
      previous = position;
    }
  };
}

// Resolve an EOF entity through the previous actual presentation. Mark whole
// leaf output so a marker never changes the entity spelling itself. Markers
// swallowed by attributes/comments/inert content cannot claim visible prose.
export function visibleSourceTail(start, end, context, previousState) {
  if (!context.sourceDocument || !context.renderSourceProbe || typeof context.before?.text !== 'string') return;
  const crypto = context.sourceDocument.defaultView?.crypto;
  if (!crypto?.getRandomValues) return;
  const { tokens, origins } = previousState();
  const nonce = Array.from(crypto.getRandomValues(new Uint32Array(4)), value => value.toString(16)).join('');
  const open = `\uE000pinvou${nonce}:start\uE001`;
  const close = `\uE000pinvou${nonce}:end\uE001`;
  let decorated = false;
  const html = context.renderSourceProbe(tokens, (token, rendered) => {
    const origin = origins.get(token);
    if (decorated || !origin || origin.start > start || origin.end < end
      || (token.type !== 'text' && token.type !== 'html')) return rendered;
    decorated = true;
    return `${open}${rendered}${close}`;
  });
  if (html === null || !decorated) return null;
  const root = context.sourceDocument.createElement('div');
  root.innerHTML = html;
  const text = visibleText(root);
  const from = text.indexOf(open);
  const to = text.indexOf(close, Math.max(0, from));
  const plain = text.replaceAll(open, '').replaceAll(close, '');
  if (plain !== context.before.text || from < 0 || to < from) return null;
  const value = text.slice(from + open.length, to);
  return { start: from, end: from + value.length, text: value };
}
