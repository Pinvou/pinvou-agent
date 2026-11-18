import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createTextOffsetMapper } from '../src/features/conversation/rich-text-offsets.js';

const linkContext = (label, start = 0) => ({ after: { roles: [{ kind: 'link', label, start, end: start + label.length }] } });
const imageContext = label => ({ after: { roles: [{ kind: 'image', label, start: 0, end: 0 }] } });

test('multiple delimiter deletions and appended output retain unchanged selected text', () => {
  for (const [before, after, word] of [
    ['Prefix **selected', 'Prefix selected trailing', 'selected'],
    ['Prefix [selected](https://example.com', 'Prefix selected trailing', 'selected'],
    ['Alpha\nBravo selected\n', 'Alpha\n\nBravo selected\n\nC\n', 'Bravo'],
    ['| Bravo | Charlie |\n| --- | ', '\n\nBravo\nCharlie\n', 'Charlie'],
  ]) {
    const map = createTextOffsetMapper(before, after);
    const from = map(before.indexOf(word), true);
    const to = map(before.indexOf(word) + word.length);
    assert.equal(after.slice(from, to), word);
  }
});

test('appended text does not extend an endpoint at the old end', () => {
  const map = createTextOffsetMapper('selected', 'selected more');
  assert.equal(map('selected'.length), 'selected'.length);
});

test('large rewrites use retained context without allocating a quadratic alignment', () => {
  const word = 'unique selected word';
  const before = `Prefix **${'earlier '.repeat(100)}${word}`;
  const after = `Prefix ${'earlier '.repeat(100)}${word}${' trailing'.repeat(1000)}`;
  const map = createTextOffsetMapper(before, after);
  const from = map(before.indexOf(word), true);
  const to = map(before.indexOf(word) + word.length);
  assert.equal(after.slice(from, to), word);
});

test('a repeated suffix belongs to the retained text, not newly appended words', () => {
  for (const ending of [' and', ' selected', ' repeated selected', ` ${'selected '.repeat(1000)}`]) {
    const before = 'Prefix **selected\n';
    const after = `Prefix selected${ending}\n`;
    const map = createTextOffsetMapper(before, after);
    assert.equal(map(before.indexOf('selected'), true), after.indexOf('selected'));
    assert.equal(map(before.indexOf('selected') + 'selected'.length), after.indexOf('selected') + 'selected'.length);
  }
});

test('removed Markdown delimiters do not align with new unclosed markup', () => {
  const before = 'Prefix **selected';
  for (const suffix of [' then **selecte', ' then **selected']) {
    const after = `Prefix selected${suffix}`;
    const map = createTextOffsetMapper(before, after, { start: 9, end: 17 });
    assert.equal(map(9, true), 7);
    assert.equal(map(17), 15);
  }
});

test('joint endpoints retain the selected occurrence beyond the alignment cap', () => {
  const before = 'Prefix **selected\n';
  const after = `Prefix selected${' selected'.repeat(4000)}\n`;
  const map = createTextOffsetMapper(before, after, { start: 9, end: 17 });
  assert.equal(map(9, true), 7);
  assert.equal(map(17), 15);
});

test('selection anchors use transformed coordinates after delimiters or long prefixes disappear', () => {
  for (const word of ['word', '选择', '😀', 'a']) {
    for (const extra of ['', ' '.repeat(4000)]) {
      const before = `Prefix ***${word}\n`;
      const after = `Prefix ${word} ${word}${extra}\n`;
      const start = before.indexOf(word);
      const map = createTextOffsetMapper(before, after, { start, end: start + word.length });
      assert.equal(map(start, true), after.indexOf(word));
      assert.equal(map(start + word.length), after.indexOf(word) + word.length);
    }
    const before = `[A][long_reference] ${word} ${word}\n`;
    const after = `A ${word} ${word}\n`;
    const start = before.indexOf(word);
    const map = createTextOffsetMapper(before, after, { start, end: start + word.length }, linkContext('A'));
    assert.equal(map(start, true), after.indexOf(word));
    assert.equal(map(start + word.length), after.indexOf(word) + word.length);
  }
});

test('nested formatting and new link tails retain repeated-word positions', () => {
  for (const [before, after, word, occurrence] of [
    ['Prefix **word word', 'Prefix word word trailing', 'word', 1],
    ['word **word ', 'word word word word', 'word', 1],
    ['[cherry cherry', 'cherry cherry cherry', 'cherry', 1],
    ['**cherry cherry', '*cherry cherry', 'cherry', 1],
  ]) {
    let from = -1;
    let expected = -1;
    for (let index = 0; index <= occurrence; index += 1) {
      from = before.indexOf(word, from + 1);
      expected = after.indexOf(word, expected + 1);
    }
    const map = createTextOffsetMapper(before, after, { start: from, end: from + word.length });
    assert.equal(map(from, true), expected);
    assert.equal(map(from + word.length), expected + word.length);
  }
});

test('resolved reference identifiers cannot align with retained or appended prose', () => {
  for (const id of ['ref', 'word', '选择', '😀']) {
    for (const padding of ['', ' '.repeat(4000)]) {
      const before = `[A][${id}] word word\n`;
      const after = `A word word word${padding}\n`;
      for (let occurrence = 0; occurrence < 2; occurrence += 1) {
        const start = before.indexOf(' word') + 1 + occurrence * 5;
        const map = createTextOffsetMapper(before, after, { start, end: start + 4 }, linkContext('A'));
        assert.equal(map(start, true), 2 + occurrence * 5);
        assert.equal(map(start + 4), 6 + occurrence * 5);
      }
    }
  }
});

test('materialized images remove alt-text anchors while escaped image syntax retains its label', () => {
  for (const label of ['word', '选择', '😀', 'word [word]']) {
    for (const padding of ['', ' '.repeat(4000)]) {
      const before = `![${label}][ref] word word\n`;
      const after = ` word word word${padding}\n`;
      const start = before.indexOf(' word') + 1;
      const map = createTextOffsetMapper(before, after, { start, end: start + 4 }, imageContext(label));
      assert.equal(map(start, true), 1);
      assert.equal(map(start + 4), 5);
    }
  }
  const before = '![word][ref] word word\n';
  const after = '!word word word word\n';
  const map = createTextOffsetMapper(before, after, { start: 13, end: 17 }, linkContext('word', 1));
  assert.equal(map(13, true), 6);
  assert.equal(map(17), 10);
});

test('punctuation-only selections preserve occurrence through emphasis closure', () => {
  const map = createTextOffsetMapper('**[] []\n', '[] [] []\n', { start: 5, end: 7 });
  assert.equal(map(5, true), 3);
  assert.equal(map(7), 5);
});

test('removed selected syntax cannot attach to equal text appended elsewhere', () => {
  const before = '[A][word] word\n';
  const after = 'A word word\n';
  const map = createTextOffsetMapper(before, after, { start: 4, end: 8 }, linkContext('A'));
  assert.equal(map(4, true), map(8));
  const emphasis = createTextOffsetMapper('**a\n', 'a then **\n', { start: 0, end: 2 });
  assert.equal(emphasis(0, true), emphasis(2));
});

test('large list whitespace expansion retains the numbered item containing repeated prose', () => {
  const lines = Array.from({ length: 40 }, (_, index) => `Task ${String(index + 1).padStart(2, '0')}: pending`);
  const before = `\n${lines.join('\n')}\n\n`;
  const after = `\n${lines.join('\n\n')}\n\nMore output\n\n\n`;
  const start = before.indexOf('pending', before.indexOf('Task 31'));
  const expected = after.indexOf('pending', after.indexOf('Task 31'));
  const map = createTextOffsetMapper(before, after, { start, end: start + 7 });
  assert.equal(map(start, true), expected);
  assert.equal(map(start + 7), expected + 7);
});

test('entirely removed image-alt selections collapse at the image boundary', () => {
  const before = 'word ![word word]\n';
  const after = 'word  word word\n';
  const map = createTextOffsetMapper(before, after, { start: 7, end: 11 }, {
    after: { roles: [{ kind: 'image', label: 'word word', start: 5, end: 5 }] },
  });
  assert.equal(map(7, true), 5);
  assert.equal(map(11), 5);
});

test('partial selections contract at removed syntax and generated paragraph endings', () => {
  const before = 'Prefix &copy\n';
  const after = 'Prefix © copy tail\n';
  const map = createTextOffsetMapper(before, after, { start: 0, end: before.length }, {
    decodeEntity: value => value === '&copy;' ? '©' : value,
  });
  assert.equal(map(0, true), 0);
  assert.equal(map(before.length), 7);
  const paragraph = createTextOffsetMapper('word\n', 'word more word\n', { start: 0, end: 5 });
  assert.equal(paragraph(0, true), 0);
  assert.equal(paragraph(5), 4);
});

test('bounded normalization preserves literal punctuation inside collapsed whitespace runs', () => {
  const lines = Array.from({ length: 40 }, (_, index) => `Task ${String(index + 1).padStart(2, '0')}: * pending`);
  const before = `\n${lines.join('\n')}\n\n`;
  const after = `\n${lines.join('\n\n')}\n\nMore output\n`;
  const start = before.indexOf('*', before.indexOf('Task 31'));
  const expected = after.indexOf('*', after.indexOf('Task 31'));
  const map = createTextOffsetMapper(before, after, { start, end: start + 1 });
  assert.equal(map(start, true), expected);
  assert.equal(map(start + 1), expected + 1);
});


test('large layout rewrites preserve isolated literal markers despite surrounding whitespace growth', () => {
  for (const marker of ['*', '**', '`', '~~', '_', '#', '>', '|']) {
    for (const position of ['start', 'end']) {
      const lines = Array.from({ length: 100 }, (_, index) => `Task ${index}: payload payload`);
      lines[50] = position === 'end' ? `Task 50: ${marker}` : `${marker} Task 50: payload`;
      const before = `\n${lines.join('\n')}\n\n`;
      const after = `\n${lines.join('\n\n')}\n\nMore output\n`;
      const start = before.indexOf(marker, before.indexOf(lines[50]));
      const expected = after.indexOf(marker, after.indexOf(lines[50]));
      const map = createTextOffsetMapper(before, after, { start, end: start + marker.length });
      assert.equal(map(start, true), expected, `${marker} ${position} start`);
      assert.equal(map(start + marker.length), expected + marker.length, `${marker} ${position} end`);
    }
  }
});


test('literal code tail preserves selected spaces and tabs through prefix rewrites', () => {
  for (const padding of ['', ' earlier'.repeat(1000)]) {
    for (const whitespace of ['  ', '\t', ' \t ']) {
      const before = `[A][ref]\n${padding}hello${whitespace}`;
      const after = `A\n${padding}hello${whitespace}world\n`;
      const literalStart = before.indexOf('\n') + 1;
      for (const value of [`hello${whitespace}`, whitespace]) {
        const start = before.lastIndexOf(value);
        const end = start + value.length;
        const map = createTextOffsetMapper(before, after, { start, end }, {
          before: { literal: [{ start: literalStart, end: before.length }] },
          ...linkContext('A'),
        });
        assert.equal(after.slice(map(start, true), map(end)), value);
        assert.equal(map(start, true), after.lastIndexOf(value));
      }
    }
  }
});


test('independent retained table endpoints preserve source positions through fostered duplicates', () => {
  const before = 'word';
  const after = 'word outsideword';
  for (const points of [
    [{ before: 0, after: 12, character: 0 }],
    [{ before: 4, after: 16, character: 3 }],
    [{ before: 0, after: 12, character: 0 }, { before: 4, after: 16, character: 3 }],
  ]) {
    const map = createTextOffsetMapper(before, after, { start: 0, end: 4 }, { retainedPoints: points });
    for (const point of points) assert.equal(map(point.before, point.before === 0), point.after);
  }
});

test('source retirement overrides retained structural endpoint hints', () => {
  const map = createTextOffsetMapper('![gone]', 'gone', { start: 2, end: 6 }, {
    ...imageContext('gone'), retainedPoints: [{ before: 2, after: 0, character: 2 }, { before: 6, after: 4, character: 5 }],
  });
  assert.equal(map(2, true), map(6));
});


test('part alignment reuses the global source retirement mask through formatting changes', () => {
  for (const start of [0, 2]) {
    const map = createTextOffsetMapper('**word', 'word retained', { start, end: 6 }, {
      retirementMask: Uint8Array.from([1, 1, 0, 0, 0, 0]),
    });
    assert.equal(map(start, true), 0);
    assert.equal(map(6), 4);
  }
});

test('a supplied source mask retires identical visible prefixes inside a part', () => {
  const map = createTextOffsetMapper('A', 'A new', { start: 0, end: 1 }, {
    retirementMask: Uint8Array.from([1]),
  });
  assert.equal(map(0, true), map(1));
});

test('native endpoints sharing a text coordinate retain independent source targets', () => {
  const anchor = {};
  const focus = {};
  const points = [{ before: 3, after: 3, identity: anchor }, { before: 3, after: 12, identity: focus }];
  for (const retainedPoints of [points, [...points].reverse()]) {
    const map = createTextOffsetMapper('onetwo', 'onenew prosetwo', { start: 3, end: 3 }, { retainedPoints });
    assert.equal(map(3, false, anchor), 3);
    assert.equal(map(3, true, focus), 12);
  }
});

test('source retirement still overrides independently identified native endpoints', () => {
  const anchor = {};
  const focus = {};
  const map = createTextOffsetMapper('same', 'same new', { start: 0, end: 4 }, {
    retirementMask: Uint8Array.from([1, 1, 1, 1]),
    retainedPoints: [{ before: 0, after: 4, character: 0, identity: anchor },
      { before: 4, after: 8, character: 3, identity: focus }],
  });
  assert.equal(map(0, true, anchor), map(4, false, focus));
});
