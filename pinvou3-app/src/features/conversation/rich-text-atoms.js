import { maskRetiredSourceSyntax } from './rich-text-source.js';
import { normalizeRichTextLabel, visibleMarkupLabel } from './rich-text-labels.js';

// Text-level parser transitions which have no link/image DOM element: entity
// completion and a final, previously incomplete reference-definition line.
const MAX_ATOM_SCAN = 1_048_576;
const ENTITY = /&(?:#(?:x[\da-f]+|\d+)|[a-z][a-z\d]+);?/gi;
function rangeLookup(ranges) {
  let index = 0;
  return position => {
    while (index < ranges.length && ranges[index].end <= position) index += 1;
    return index < ranges.length && ranges[index].start <= position;
  };
}

function entityCandidates(text, context, decode) {
  if (!decode) return [];
  const result = [];
  const isLiteral = rangeLookup(context.literal || []);
  for (const match of text.matchAll(ENTITY)) {
    if (isLiteral(match.index)) continue;
    const raw = match[0];
    const decoded = decode(raw.endsWith(';') ? raw : `${raw};`);
    const label = decoded && decoded !== raw && decoded !== `${raw};` ? decoded : '';
    // Keep undecodable spellings as literal continuations, so a growing
    // unfinished entity cannot consume an unrelated later decoded symbol.
    result.push({ kind: 'candidate', entity: true, partial: !raw.endsWith(';'), raw, label, visibleLabel: label,
      start: match.index, end: match.index + raw.length });
  }
  return result;
}

function entityRoles(text, context, labels) {
  const roles = [];
  let remaining = MAX_ATOM_SCAN;
  for (const label of labels) {
    remaining -= text.length;
    if (remaining < 0) break;
    const isLiteral = rangeLookup(context.literal || []);
    let start = text.indexOf(label);
    while (start >= 0) {
      if (!isLiteral(start)) roles.push({ kind: 'entity', label, start, end: start + label.length });
      start = text.indexOf(label, start + Math.max(1, label.length));
    }
  }
  return roles;
}

function definitionMask(before, after, left, right, mask) {
  const lines = [...before.matchAll(/(^|\n)( {0,3}\[[^\]\n]+\]:[^\n]*)(?=\n|$)/g)];
  const candidate = lines.at(-1);
  if (!candidate) return;
  const start = candidate.index + candidate[1].length;
  const raw = candidate[2];
  if (before.slice(start + raw.length).trim() || rangeLookup(left.excluded || [])(start)) return;
  const count = (text, context) => {
    let total = 0;
    let position = 0;
    const isExcluded = rangeLookup(context.excluded || []);
    for (const line of text.split('\n')) {
      if (line.startsWith(raw) && !isExcluded(position)) total += 1;
      position += line.length + 1;
    }
    return total;
  };
  if (count(after, right) < count(before, left)) mask.fill(1, start, start + raw.length);
}

function titleMask(before, after, context, mask) {
  if (!context.definitionTitles) return;
  const openings = [...before.matchAll(/(^|\n)([ \t]{0,3}["'(])/g)];
  if (!openings.length) return;
  const normalize = normalizeRichTextLabel;
  const titles = context.definitionTitles().flatMap(title => [normalize(title), normalize(visibleMarkupLabel(title))]);
  if (!titles.length) return;
  const count = (text, fragment, excluded) => {
    const isExcluded = rangeLookup(excluded || []);
    let position = 0;
    let total = 0;
    for (const line of text.split('\n')) {
      if (!isExcluded(position) && normalize(line).startsWith(fragment)) total += 1;
      position += line.length + 1;
    }
    return total;
  };
  let budget = MAX_ATOM_SCAN;
  for (const match of openings.reverse()) {
    const start = match.index + match[1].length;
    if (rangeLookup(context.before?.excluded || [])(start)) continue;
    const body = before.slice(start + match[2].length, start + match[2].length + 4096);
    budget -= body.length;
    if (budget < 0) return;
    const partial = normalize(body).slice(0, 64);
    if (!partial || titles.every(title => !title.startsWith(partial))) continue;
    const fragment = normalize(match[2] + body).slice(0, 65);
    if (count(after, fragment, context.after?.excluded) >= count(before, fragment, context.before?.excluded)) continue;
    mask.fill(1, start, before.trimEnd().length);
    return;
  }
}

export function textSyntaxAtoms(before, after, context, mask) {
  const left = context.before || {};
  const right = context.after || {};
  maskRetiredSourceSyntax(before, context, mask);
  definitionMask(before, after, left, right, mask);
  titleMask(before, after, context, mask);
  const beforeAtoms = entityCandidates(before, left, context.decodeEntity);
  const afterAtoms = entityCandidates(after, right, context.decodeEntity);
  const labels = new Set([...beforeAtoms, ...afterAtoms].map(atom => atom.label).filter(Boolean));
  return {
    before: { ...left, atoms: beforeAtoms, roles: [...(left.roles || []), ...entityRoles(before, left, labels)] },
    after: { ...right, atoms: afterAtoms, roles: [...(right.roles || []), ...entityRoles(after, right, labels)] },
  };
}
