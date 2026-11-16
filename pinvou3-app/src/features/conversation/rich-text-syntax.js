import { normalizeRichTextLabel as normalizeLabel, visibleMarkupLabel as plainAlt } from './rich-text-labels.js';
import { textSyntaxAtoms } from './rich-text-atoms.js';

// Correspond visible Markdown candidates with actual rendered links/images.
// Literal references and code stay ordinary text anchors; only a candidate
// that becomes a rendered element contributes disappearing syntax.
const MAX_EVENT_CELLS = 262_144;
const MAX_CANDIDATE_CHARACTERS = 1_048_576;

function candidates(text, context) {
  const pairs = new Map();
  const stack = [];
  const openings = [];
  const excluded = context.excluded || [];
  let range = 0;
  for (let index = 0; index < text.length; index += 1) {
    while (range < excluded.length && excluded[range].end <= index) range += 1;
    if (range < excluded.length && excluded[range].start <= index) {
      // eslint-disable-next-line sonarjs/updated-loop-counter -- Skip an opaque DOM range in one step.
      index = excluded[range].end - 1;
      continue;
    }
    if (text[index] === '\\') { index += 1; continue; }
    if (text[index] === '[') { stack.push(index); openings.push(index); }
    else if (text[index] === ']' && stack.length) pairs.set(stack.pop(), index);
  }
  const images = (context.roles || []).filter(role => role.kind === 'image');
  let remaining = MAX_CANDIDATE_CHARACTERS;
  return openings.flatMap(open => {
    if (remaining <= 0) return [];
    const pairedClose = pairs.get(open);
    const image = open > 0 && text[open - 1] === '!';
    const partialLabel = pairedClose === undefined;
    if (partialLabel && !image) return [];
    const close = pairedClose ?? text.trimEnd().length;
    const start = image ? open - 1 : open;
    let end = partialLabel ? close : close + 1;
    let partial = partialLabel;
    const tail = end;
    if (!partialLabel && text[end] === '[') {
      partial = !pairs.has(end);
      end = pairs.has(end) ? pairs.get(end) + 1 : text.trimEnd().length;
    }
    else if (!partialLabel && text[end] === '(') {
      let depth = 1;
      end += 1;
      while (end < text.length && text[end] !== '\n' && depth > 0) {
        if (text[end] === '\\') end += 1;
        else if (text[end] === '(') depth += 1;
        else if (text[end] === ')') depth -= 1;
        end += 1;
      }
      partial = depth > 0;
    }
    remaining -= (end - start) * 2;
    if (remaining < 0) return [];
    let label = '';
    let position = open + 1;
    // A nested rendered image has no DOM text. Include its alt only when
    // comparing the enclosing candidate with a newly rendered outer image.
    let lower = 0;
    let upper = images.length;
    while (lower < upper) {
      const middle = (lower + upper) >>> 1;
      if (images[middle].start < position) lower = middle + 1;
      else upper = middle;
    }
    for (let index = lower; index < images.length && images[index].start <= close; index += 1) {
      const role = images[index];
      remaining -= role.label.length + 1;
      if (remaining < 0) return [];
      label += text.slice(position, role.start) + role.label;
      position = role.start;
    }
    label += text.slice(position, close);
    return [{ kind: 'candidate', start, end, open, close, tail, image, partial, partialLabel,
      label: normalizeLabel(label), visibleLabel: normalizeLabel(text.slice(open + 1, close)), raw: text.slice(start, end) }];
  });
}

function labelTokens(candidate, images, budget) {
  const children = candidate.children;
  let lower = 0;
  let upper = images.length;
  while (lower < upper) {
    const middle = (lower + upper) >>> 1;
    if (images[middle].start < candidate.open + 1) lower = middle + 1;
    else upper = middle;
  }
  const nested = [...children];
  let childIndex = 0;
  for (let index = lower; index < images.length && images[index].start <= candidate.close; index += 1) {
    if (budget.remaining-- <= 0) return [candidate.visibleLabel];
    const role = images[index];
    while (childIndex < children.length && children[childIndex].end <= role.start) childIndex += 1;
    if (childIndex < children.length && children[childIndex].start <= role.start) continue;
    nested.push(role);
  }
  nested.sort((left, right) => left.start - right.start || right.end - left.end);
  const tokens = [];
  let position = candidate.open + 1;
  for (const child of nested) {
    if (budget.remaining-- <= 0) return [candidate.visibleLabel];
    tokens.push(normalizeLabel(candidate.text.slice(position, child.start)), child);
    position = child.end;
  }
  tokens.push(normalizeLabel(candidate.text.slice(position, candidate.close)));
  return tokens;
}

function events(text, context, identity, sourceOwnsPartialImages = false, sourceOwnsImages = false) {
  const found = candidates(text, context);
  const tails = found.filter(candidate => candidate.end > candidate.tail)
    .map(candidate => ({ start: candidate.tail, end: candidate.end })).sort((left, right) => left.start - right.start);
  const literal = [];
  let range = 0;
  for (const candidate of found) {
    while (range < tails.length && tails[range].end <= candidate.start) range += 1;
    if (range < tails.length && tails[range].start <= candidate.start && candidate.end <= tails[range].end) continue;
    literal.push({ ...candidate, text });
  }
  const parents = [];
  for (const candidate of literal) {
    while (parents.length && candidate.end > parents.at(-1).close) parents.pop();
    candidate.children = [];
    if (parents.length) parents.at(-1).children.push(candidate);
    parents.push(candidate);
  }
  const images = (context.roles || []).filter(role => role.kind === 'image');
  const budget = { remaining: MAX_CANDIDATE_CHARACTERS };
  for (const candidate of literal) candidate.tokens = labelTokens(candidate, images, budget);
  return [...literal, ...(context.atoms || []), ...(context.roles || [])].map(event => ({
    ...event,
    sourceOwnedPartial: sourceOwnsPartialImages && event.image && event.partial,
    sourceOwnedImage: sourceOwnsImages && event.image,
    continuationText: event.kind === 'candidate' ? normalizeLabel(plainAlt(event.raw)) : '',
    labelId: identity(event.entity || event.kind === 'entity' ? event.label : normalizeLabel(event.label)),
    visibleLabelId: identity(event.entity ? event.label : event.kind === 'candidate' ? event.visibleLabel : normalizeLabel(event.label)),
    plainLabelId: identity(normalizeLabel(plainAlt(event.label))),
    identityId: identity(JSON.stringify(event.kind === 'candidate'
      ? ['candidate', event.raw] : [event.kind, event.label, event.destination])),
  })).sort((left, right) => left.start - right.start || right.end - left.end);
}

// Match nested label alternatives against the actual rendered label instead
// of enumerating combinations. A child image contributes no visible link text
// and contributes its alt inside another image. Literal unresolved syntax is
// always an alternative. Memoization and a shared work budget bound this walk.
function labelMatcher() {
  let remaining = MAX_CANDIDATE_CHARACTERS;
  const results = new WeakMap();
  return (candidate, rendered) => {
    let cache = results.get(candidate);
    if (!cache) { cache = new Map(); results.set(candidate, cache); }
    const key = `${rendered.kind}:${rendered.labelId}`;
    if (cache.has(key)) return cache.get(key);
    const targets = [...new Set([normalizeLabel(rendered.label), normalizeLabel(plainAlt(rendered.label))])];
    const matches = targets.some(target => {
      const memo = new WeakMap();
      const literal = (text, position) => {
        remaining -= text.length + 1;
        return remaining >= 0 && target.startsWith(text, position) ? [position + text.length] : [];
      };
      const consume = (node, mode, position) => {
        if (remaining-- <= 0) return [];
        if (typeof node === 'string') return literal(node, position);
        if (node.kind === 'image') return mode === 'link' ? [position]
          : [...new Set([normalizeLabel(node.label), normalizeLabel(plainAlt(node.label))])].flatMap(value => literal(value, position));
        let states = memo.get(node);
        if (!states) { states = new Map(); memo.set(node, states); }
        const stateKey = `${mode}:${position}`;
        if (states.has(stateKey)) return states.get(stateKey);
        let positions = [position];
        for (const token of node.tokens) positions = [...new Set(positions.flatMap(offset => {
          if (typeof token === 'string' || token.kind === 'image') return consume(token, mode, offset);
          const raw = literal(normalizeLabel(token.raw), offset);
          const renderedLabel = consume(token, mode, offset);
          if (!token.image) return [...raw, ...renderedLabel];
          const escapedLink = literal('!', offset).flatMap(next => consume(token, mode, next));
          return [...raw, ...(mode === 'link' ? [offset] : renderedLabel), ...escapedLink];
        }))];
        states.set(stateKey, positions);
        return positions;
      };
      const positions = consume(candidate, rendered.kind, 0);
      return candidate.partialLabel ? positions.some(position => position > 0) : positions.includes(target.length);
    });
    cache.set(key, matches);
    return matches;
  };
}

function matchWeight(before, after, matchesLabel) {
  if (before.identityId === after.identityId) return 4;
  if (before.kind === 'candidate' && after.kind === 'candidate'
    && (after.raw.startsWith(before.raw)
      || (before.partial && after.continuationText.startsWith(before.continuationText)))) return 4;
  if (before.kind !== 'candidate' || after.kind === 'candidate') return 0;
  // With append-only source context, the actual parser owns unfinished image
  // label/destination/reference retirement. Inline entity/HTML/formatting completion can change a retained
  // label arbitrarily; a later similar image is no evidence it disappeared.
  if (before.sourceOwnedPartial || (before.sourceOwnedImage && after.kind === 'image')) return 0;
  if (before.entity) return after.kind === 'entity' && before.labelId === after.labelId ? 1 : 0;
  if (after.kind !== 'link' && (!before.image || after.kind !== 'image')) return 0;
  const label = after.kind === 'link' ? before.visibleLabelId : before.labelId;
  return label === after.labelId || (after.kind === 'image' && label === after.plainLabelId)
    || matchesLabel(before, after) ? 1 : 0;
}

function largeCandidateMatch(event, after, next, matchesLabel) {
  const kinds = event.entity ? ['entity'] : event.image && !event.sourceOwnedImage ? ['link', 'image'] : ['link'];
  const roles = kinds.map(kind => next(`kind:${kind}`)).filter(index => index >= 0);
  const literal = next('kind:candidate');
  // A still-literal unfinished candidate can grow or close before a later
  // rendered element. Preserve that continuation before consuming new roles.
  if (literal >= 0 && literal <= Math.min(...roles) && matchWeight(event, after[literal], matchesLabel) === 4) return literal;
  if (event.sourceOwnedPartial) return -1;
  const exactLabels = kinds.map(kind => next(`${kind}:${kind === 'image' ? event.labelId : event.visibleLabelId}`))
    .filter(index => index >= 0);
  // At most one next role per kind is probed. The same memoized nested-label
  // matcher and shared character budget apply above the allocation threshold.
  const semantic = roles.filter(index => matchWeight(event, after[index], matchesLabel) > 0);
  const candidates = [...exactLabels, ...semantic];
  return candidates.length ? Math.min(...candidates) : -1;
}

function alignLargeEvents(before, after, visit, matchesLabel) {
  const queues = new Map();
  const add = (key, index) => {
    if (!queues.has(key)) queues.set(key, { values: [], index: 0 });
    queues.get(key).values.push(index);
  };
  after.forEach((event, index) => {
    add(`exact:${event.identityId}`, index);
    add(`kind:${event.kind}`, index);
    if (event.kind !== 'candidate') add(`${event.kind}:${event.labelId}`, index);
  });
  let target = 0;
  const next = key => {
    const queue = queues.get(key);
    if (!queue) return -1;
    while (queue.index < queue.values.length && queue.values[queue.index] < target) queue.index += 1;
    return queue.values[queue.index] ?? -1;
  };
  for (const event of before) {
    let match = next(`exact:${event.identityId}`);
    if (match < 0 && event.kind === 'candidate') match = largeCandidateMatch(event, after, next, matchesLabel);
    if (match < 0) continue;
    visit(event, after[match]);
    target = match + 1;
  }
}

function alignEvents(before, after, visit) {
  const matchesLabel = labelMatcher();
  const width = after.length + 1;
  if ((before.length + 1) * width > MAX_EVENT_CELLS) {
    // Bound work on documents with thousands of semantic elements. Preserve
    // exact events first; consume candidate/element matches only in order.
    alignLargeEvents(before, after, visit, matchesLabel);
    return;
  }
  const scores = new Uint32Array((before.length + 1) * width);
  for (let a = before.length - 1; a >= 0; a -= 1) {
    for (let b = after.length - 1; b >= 0; b -= 1) {
      scores[a * width + b] = Math.max(scores[(a + 1) * width + b], scores[a * width + b + 1],
        scores[(a + 1) * width + b + 1] + matchWeight(before[a], after[b], matchesLabel));
    }
  }
  let a = 0;
  let b = 0;
  while (a < before.length && b < after.length) {
    const weight = matchWeight(before[a], after[b], matchesLabel);
    if (weight > 0 && scores[a * width + b] === scores[(a + 1) * width + b + 1] + weight) {
      visit(before[a++], after[b++]);
    } else if (scores[(a + 1) * width + b] >= scores[a * width + b + 1]) a += 1;
    else b += 1;
  }
}

export function disappearingSyntaxMask(before, after, context = {}) {
  const mask = new Uint8Array(before.length);
  const atoms = textSyntaxAtoms(before, after, context, mask);
  if (!atoms.after.roles.length) return mask;
  const identities = new Map();
  const identity = value => {
    if (!identities.has(value)) identities.set(value, identities.size);
    return identities.get(value);
  };
  const sourceOwnsPartialImages = typeof context.previousSource === 'string' && typeof context.source === 'string'
    && context.source.startsWith(context.previousSource) && typeof context.sourceTokens === 'function';
  const sourceOwnsImages = sourceOwnsPartialImages && context.sourceDocument && typeof context.renderSourceProbe === 'function';
  const left = events(before, atoms.before, identity, sourceOwnsPartialImages, sourceOwnsImages);
  const right = events(after, atoms.after, identity);
  alignEvents(left, right, (previous, next) => {
    if (previous.kind !== 'candidate' || next.kind === 'candidate') return;
    if (previous.entity) { mask.fill(1, previous.start, previous.end); return; }
    mask.fill(1, previous.tail, previous.end);
    if (next.kind === 'image') mask.fill(1, previous.start, previous.tail);
    else { mask[previous.open] = 1; if (!previous.partialLabel) mask[previous.close] = 1; }
  });
  return mask;
}
