import { disappearingSyntaxMask } from './rich-text-syntax.js';

// Map DOM text coordinates through multiple Markdown edits in one stream
// frame. Prefix/suffix-only mapping treats removed delimiters and inserted
// block whitespace as one replacement and shifts unchanged selected words.
const MAX_ALIGNMENT_CELLS = 262_144;
const CONTEXT_LENGTH = 64;
const WORD_CHARACTER = /[\p{L}\p{N}\p{M}\p{S}]/u;
const MARKDOWN_DELIMITERS = new Set('*_~`[]()|><!#'.split(''));

function isAnchorCharacter(character) {
  const code = character.codePointAt(0);
  return !MARKDOWN_DELIMITERS.has(character)
    && (WORD_CHARACTER.test(character) || (code >= 0xd800 && code <= 0xdfff));
}

function nearestOccurrence(text, needle, expected) {
  if (!needle) return -1;
  const left = text.lastIndexOf(needle, expected);
  const right = text.indexOf(needle, expected);
  if (left < 0) return right;
  if (right < 0) return left;
  return expected - left <= right - expected ? left : right;
}

function matchingRuns(before, after, prefix, syntaxMask, selectedPunctuation) {
  const left = before.slice(prefix);
  const right = after.slice(prefix);
  const width = right.length + 1;
  if ((left.length + 1) * width > MAX_ALIGNMENT_CELLS) return null;
  const textWeight = left.length + right.length + 1;
  const weights = Array.from({ length: left.length }, (_, index) => syntaxMask[prefix + index] ? 0 : (isAnchorCharacter(left[index]) ? textWeight : selectedPunctuation.has(left[index]) ? 1 : 0));
  const scores = new Uint32Array((left.length + 1) * width);
  for (let a = left.length - 1; a >= 0; a -= 1) {
    for (let b = right.length - 1; b >= 0; b -= 1) {
      scores[a * width + b] = Math.max(
        scores[(a + 1) * width + b], scores[a * width + b + 1],
        left[a] === right[b] ? scores[(a + 1) * width + b + 1] + weights[a] : 0,
      );
    }
  }
  const runs = prefix ? [{ start: 0, end: prefix, target: 0 }] : [];
  let a = 0;
  let b = 0;
  while (a < left.length && b < right.length) {
    // Delimiters that disappeared during parsing must not win alignment by
    // matching a new unclosed Markdown delimiter farther into the output.
    if (weights[a] > 0 && left[a] === right[b]
      && scores[a * width + b] === scores[(a + 1) * width + b + 1] + weights[a]) {
      const start = a;
      const target = b;
      while (a < left.length && b < right.length && weights[a] > 0 && left[a] === right[b]) { a += 1; b += 1; }
      runs.push({ start: prefix + start, end: prefix + a, target: prefix + target });
    } else if (scores[(a + 1) * width + b] >= scores[a * width + b + 1]) a += 1;
    else b += 1;
  }
  return runs;
}

// Ignore layout-only whitespace expansion while keeping a coordinate back to
// each original DOM character. Neighboring words disambiguate repeated payloads
// in large list/table rewrites without a quadratic character-alignment table.
function normalizedText(text, mask) {
  let value = '';
  const starts = [];
  const ends = [];
  const points = new Uint32Array(text.length + 1);
  for (let index = 0; index < text.length; index += 1) {
    points[index] = value.length;
    if (mask?.[index] || '*_~`|#>'.includes(text[index])) continue;
    const character = /\s/.test(text[index]) ? ' ' : text[index];
    if (character === ' ' && value.endsWith(' ')) ends[ends.length - 1] = index + 1;
    else { starts.push(index); ends.push(index + 1); value += character; }
  }
  points[text.length] = value.length;
  return { value, starts, ends, points, length: text.length };
}

function normalizedContextPoint(left, right, point) {
  let best = { length: 0, position: Math.min(point, right.length) };
  for (let length = CONTEXT_LENGTH; length >= 1; length = Math.floor(length / 2)) {
    for (const side of ['left', 'right']) {
      const context = side === 'left' ? left.slice(Math.max(0, point - length), point) : left.slice(point, point + length);
      const distance = side === 'left' ? context.length : 0;
      const match = nearestOccurrence(right, context, Math.max(0, point - distance));
      if (match < 0) continue;
      const position = match + distance;
      if (context.length > best.length || (context.length === best.length
        && Math.abs(position - point) < Math.abs(best.position - point))) best = { length: context.length, position };
    }
    if (best.length >= length) break;
  }
  return best.position;
}

// A literal marker can share a normalized whitespace run with new paragraph
// separators. Match the run's non-whitespace characters in order, then map
// exact character boundaries independently from the changed whitespace.
function literalRunOffset(before, after, offset, preferNext) {
  if (before === after) return offset;
  const left = [];
  const right = [];
  for (let index = 0; index < before.length; index += 1) if (!/\s/.test(before[index])) left.push(index);
  for (let index = 0; index < after.length; index += 1) if (!/\s/.test(after[index])) right.push(index);
  if (!left.length || left.length !== right.length || left.some((position, index) => before[position] !== after[right[index]])) return;
  let point = 0;
  while (point < left.length && left[point] < offset) point += 1;
  if (offset > 0 && !/\s/.test(before[offset - 1])) return right[point - 1] + 1;
  if (offset < before.length && !/\s/.test(before[offset])) return right[point];
  if (offset === 0) return 0;
  if (offset === before.length) return after.length;
  return preferNext ? right[point] ?? after.length : point === 0 ? 0 : right[point - 1] + 1;
}

function normalizedMapper(before, after, mask) {
  const left = normalizedText(before, mask);
  const right = normalizedText(after);
  let prefix = 0;
  while (prefix < left.value.length && prefix < right.value.length && left.value[prefix] === right.value[prefix]) prefix += 1;
  return (offset, preferNext) => {
    const point = left.points[offset];
    const position = point <= prefix ? point : normalizedContextPoint(left.value, right.value, point);
    // A skipped marker can sit inside a collapsed whitespace run, as in
    // "Task 31: * pending". Preserve that source run before using its gap.
    const previous = point - 1;
    const targetPrevious = position - 1;
    if (previous >= 0 && targetPrevious >= 0 && offset >= left.starts[previous] && offset <= left.ends[previous]) {
      const retained = literalRunOffset(before.slice(left.starts[previous], left.ends[previous]),
        after.slice(right.starts[targetPrevious], right.ends[targetPrevious]), offset - left.starts[previous], preferNext);
      if (retained !== undefined) return right.starts[targetPrevious] + retained;
    }
    // Formatting markers are omitted only for locating retained context. If
    // the skipped run itself survives at that location, retain its exact
    // offsets too (including literal punctuation outside code).
    const leftStart = point === 0 ? 0 : left.ends[point - 1];
    const leftEnd = left.starts[point] ?? left.length;
    const rightStart = position === 0 ? 0 : right.ends[position - 1];
    const rightEnd = right.starts[position] ?? right.length;
    if (offset >= leftStart && offset <= leftEnd) {
      const retained = literalRunOffset(before.slice(leftStart, leftEnd), after.slice(rightStart, rightEnd), offset - leftStart, preferNext);
      if (retained !== undefined) return rightStart + retained;
    }
    if (preferNext) return right.starts[position] ?? right.length;
    return position === 0 ? 0 : right.ends[position - 1] ?? right.length;
  };
}

function precedingSyntaxBoundary(mask, offset, preferNext) {
  if (!mask[offset] && (offset === 0 || !mask[offset - 1] || preferNext)) return offset;
  let boundary = offset;
  while (boundary > 0 && mask[boundary - 1]) boundary -= 1;
  return boundary;
}

function resolveRetainedPoints(points = [], mask) {
  const offsets = new Map();
  const identities = new Map();
  const conflicts = new Set();
  for (const point of points) {
    const after = point.resolve ? point.resolve(mask) : mask?.[point.character] ? undefined : point.after;
    if (after === undefined) continue;
    if (point.identity !== undefined) identities.set(point.identity, after);
    if (conflicts.has(point.before)) continue;
    if (offsets.has(point.before) && offsets.get(point.before) !== after) {
      // Two native endpoints may share a UTF-16 coordinate while straddling
      // a paragraph break. Neither source may override the other's target.
      offsets.delete(point.before);
      conflicts.add(point.before);
    } else offsets.set(point.before, after);
  }
  return { offsets, identities };
}

export function createTextOffsetMapper(before, after, selectedRange, syntaxContext) {
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) prefix += 1;
  // A shared suffix may belong to newly appended text (selected -> selected
  // and shares d, and repeated words can share the entire old selection).
  // Let alignment choose the earliest retained occurrence instead of pinning
  // old endpoints to the end of the new frame.
  // Code whitespace is visible payload, including at the live output tail.
  // Only whitespace outside literal code may be renderer-generated layout.
  const tailStart = (syntaxContext?.before?.literal || []).reduce(
    (end, span) => Math.max(end, span.end), before.trimEnd().length);
  let runs;
  let syntaxMask = syntaxContext?.retirementMask;
  // Equal visible prefixes can belong to newly appended text after a parser
  // rewrite (e.g. a retired closing fence followed by a new literal tick).
  // Establish source ownership before treating that prefix as unchanged.
  if (selectedRange && typeof syntaxContext?.previousSource === 'string'
    && typeof syntaxContext.source === 'string' && (syntaxContext.source !== syntaxContext.previousSource || syntaxContext.retireFilteredCode)) {
    syntaxMask ??= disappearingSyntaxMask(before, after, syntaxContext);
  }
  if (syntaxContext?.retainedPoints?.length) syntaxMask ??= disappearingSyntaxMask(before, after, syntaxContext);
  if (syntaxMask) {
    const retired = syntaxMask.findIndex(Boolean);
    if (retired >= 0) prefix = Math.min(prefix, retired);
  }
  const retainedPoints = resolveRetainedPoints(syntaxContext?.retainedPoints, syntaxMask);
  let mapNormalized;
  const selected = selectedRange && before.slice(selectedRange.start, selectedRange.end);
  const selectedLiteral = selectedRange && syntaxContext?.before?.literal?.some(span =>
    span.start <= selectedRange.start && span.end >= selectedRange.end);
  const selectedPunctuation = new Set(selected && ![...selected].some(isAnchorCharacter)
    ? [...selected].filter(character => selectedLiteral || !/\s/.test(character)) : []);
  let retainedStart = -1;
  const mapOffset = (offset, preferNext = false, identity) => {
    // Source retirement takes precedence over structural endpoint hints.
    if (identity !== undefined && retainedPoints.identities.has(identity)) return retainedPoints.identities.get(identity);
    if (retainedPoints.offsets.has(offset)) return retainedPoints.offsets.get(offset);
    // Anchor the two endpoints together when the selected text survives.
    // Unselected Markdown delimiters and new trailing output cannot compete
    // with that retained selection, even beyond the alignment allocation cap.
    if (retainedStart >= 0) {
      if (offset === selectedRange.start) return retainedStart;
      if (offset === selectedRange.end) return retainedStart + selected.length;
    }
    // A boundary at the end of the old output stays before appended text.
    if (offset < prefix || (offset === prefix && (!preferNext || offset === before.length))) return offset;
    if (offset > tailStart) {
      const boundary = Math.max(tailStart, Math.min(offset, prefix));
      if (boundary < offset) return mapOffset(boundary);
    }
    if (runs === undefined) {
      syntaxMask ??= disappearingSyntaxMask(before, after, syntaxContext);
      runs = matchingRuns(before, after, prefix, syntaxMask, selectedPunctuation);
    }
    const boundary = precedingSyntaxBoundary(syntaxMask, offset, preferNext);
    if (boundary < offset) return mapOffset(boundary);
    if (!runs) {
      mapNormalized ??= normalizedMapper(before, after, syntaxMask);
      return mapNormalized(offset, preferNext);
    }
    let previous = { end: 0, target: 0, start: 0 };
    for (let index = 0; index < runs.length; index += 1) {
      const run = runs[index];
      const previousEnd = previous.target + previous.end - previous.start;
      if (offset < run.start) return previousEnd + Math.min(offset - previous.end, run.target - previousEnd);
      if (offset <= run.end) {
        const next = runs[index + 1];
        if (offset === run.end && preferNext && next?.start === offset) return next.target;
        if (offset === run.start && !preferNext && previous.end === offset) return previousEnd;
        return run.target + offset - run.start;
      }
      previous = run;
    }
    return Math.min(after.length, previous.target + previous.end - previous.start + offset - previous.end);
  };
  if (selected && selectedRange.end <= tailStart && (selectedLiteral || [...selected].some(isAnchorCharacter))) {
    // Resolve the transformed coordinate before disambiguating repeated text.
    // Node identity cannot establish source ownership after reconciliation.
    const expected = mapOffset(selectedRange.start, true);
    if (selectedRange.end > prefix) syntaxMask ??= disappearingSyntaxMask(before, after, syntaxContext);
    // A selected identifier/alt/marker that disappears cannot be retained by
    // finding the same payload in unrelated new prose.
    if (!syntaxMask?.slice(selectedRange.start, selectedRange.end).some(Boolean)) {
      retainedStart = nearestOccurrence(after, selected, expected);
    }
  }
  if (selected && selectedRange.end > prefix) {
    syntaxMask ??= disappearingSyntaxMask(before, after, syntaxContext);
    if (syntaxMask.slice(selectedRange.start, selectedRange.end).every(Boolean)) {
      let boundary = selectedRange.start;
      while (boundary > 0 && syntaxMask[boundary - 1]) boundary -= 1;
      const collapsed = mapOffset(boundary);
      return (offset, preferNext, identity) => offset === selectedRange.start || offset === selectedRange.end
        ? collapsed : mapOffset(offset, preferNext, identity);
    }
  }
  if (!selectedRange || selectedRange.start === selectedRange.end || retainedStart >= 0) return mapOffset;
  const start = mapOffset(selectedRange.start, true);
  const end = mapOffset(selectedRange.end);
  // Removed selected characters must not turn the range backward and select
  // an unrelated expanse of newly appended output.
  if (start <= end) return mapOffset;
  return (offset, preferNext, identity) => offset === selectedRange.start || offset === selectedRange.end
    ? Math.min(start, end) : mapOffset(offset, preferNext, identity);
}
