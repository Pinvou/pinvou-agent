import assert from 'node:assert/strict';
import { test } from 'node:test';
import { markdownDefinitionTitles, markdownSourceTokens, markdownHtmlTagIsLiteral } from '../src/shared/markdown-renderer.js';
import { disappearingSyntaxMask } from '../src/features/conversation/rich-text-syntax.js';

const role = (kind, label, start, end = start) => ({ kind, label, start, end });
const masked = (mask, text, needle, occurrence = 0) => {
  let start = -1;
  for (let index = 0; index <= occurrence; index += 1) start = text.indexOf(needle, start + 1);
  assert.ok(start >= 0);
  return [...mask.slice(start, start + needle.length)];
};

test('resolved references lose identifiers while unresolved references remain literal anchors', () => {
  const before = '[A][ref] [x][word] word';
  const after = 'A [x][word] word';
  const mask = disappearingSyntaxMask(before, after, { after: { roles: [role('link', 'A', 0, 1)] } });
  assert.deepEqual(masked(mask, before, 'ref'), [1, 1, 1]);
  assert.deepEqual(masked(mask, before, 'word'), [0, 0, 0, 0]);
});

test('code containing identical reference labels cannot consume a rendered link correspondence', () => {
  const before = '[A][word] [A][ref] word';
  const after = '[A][word] A word';
  const mask = disappearingSyntaxMask(before, after, {
    before: { excluded: [{ start: 0, end: 9 }] },
    after: { excluded: [{ start: 0, end: 9 }], roles: [role('link', 'A', 10, 11)] },
  });
  assert.deepEqual(masked(mask, before, 'word'), [0, 0, 0, 0]);
  assert.deepEqual(masked(mask, before, 'ref'), [1, 1, 1]);
});

test('escaped image-like link and real image with identical labels keep distinct roles', () => {
  const before = '![word][ref] ![word][ref] word';
  const after = '!word  word';
  const mask = disappearingSyntaxMask(before, after, {
    after: { roles: [role('link', 'word', 1, 5), role('image', 'word', 6)] },
  });
  assert.deepEqual(masked(mask, before, 'word', 0), [0, 0, 0, 0]);
  assert.deepEqual(masked(mask, before, 'word', 1), [1, 1, 1, 1]);
  assert.deepEqual(masked(mask, before, 'word', 2), [0, 0, 0, 0]);
});

test('nested image replacement identifies the disappearing outer label without a count change', () => {
  const before = '![word ][ref] word';
  const after = ' word';
  const mask = disappearingSyntaxMask(before, after, {
    before: { roles: [role('image', 'alt', 7)] },
    after: { roles: [role('image', 'word alt', 0)] },
  });
  assert.deepEqual(masked(mask, before, 'word', 0), [1, 1, 1, 1]);
  assert.deepEqual(masked(mask, before, 'word', 1), [0, 0, 0, 0]);
});

test('large semantic histories preserve literal events before resolving a new tail reference', () => {
  const history = '[x][literal] '.repeat(800);
  const before = `${history}[gone][ref] word`;
  const after = `${history}gone word`;
  const mask = disappearingSyntaxMask(before, after, {
    after: { roles: [role('link', 'gone', history.length, history.length + 4)] },
  });
  assert.equal(mask.slice(0, history.length).some(Boolean), false);
  assert.deepEqual(masked(mask, before, 'ref'), [1, 1, 1]);
});

test('linked reference images discard the outer identifier and retain distinct nested roles', () => {
  const before = '[![alt][image]][word] word';
  const after = ' word';
  const mask = disappearingSyntaxMask(before, after, {
    after: { roles: [role('link', '', 0), role('image', 'alt', 0)] },
  });
  assert.deepEqual(masked(mask, before, 'word', 0), [1, 1, 1, 1]);
  assert.deepEqual(masked(mask, before, 'word', 1), [0, 0, 0, 0]);
});

test('simultaneously resolved nested image labels contribute semantic alt text', () => {
  const before = '![word ![alt][image]][outer] word';
  const mask = disappearingSyntaxMask(before, ' word', {
    after: { roles: [role('image', 'word alt', 0)] },
  });
  assert.deepEqual(masked(mask, before, 'word', 0), [1, 1, 1, 1]);
  assert.deepEqual(masked(mask, before, 'word', 1), [0, 0, 0, 0]);
});

test('literal HTML in an image alt corresponds to the old visible label', () => {
  const before = '![wordword][ref] word';
  const mask = disappearingSyntaxMask(before, ' word', {
    after: { roles: [role('image', 'word<br>word', 0)] },
  });
  assert.deepEqual(masked(mask, before, 'wordword'), Array(8).fill(1));
});

const decodeEntity = value => value === '&copy;' ? '©' : value;

test('completed entities retain existing symbols and consume only their own old spelling', () => {
  const before = '© &copy';
  const mask = disappearingSyntaxMask(before, '© © copy tail', { decodeEntity });
  assert.equal(mask[0], 0);
  assert.deepEqual(masked(mask, before, 'copy'), [1, 1, 1, 1]);
});

test('literal code entity spellings cannot consume another completed entity', () => {
  const before = '&copy [A][ref] &copy';
  const mask = disappearingSyntaxMask(before, '&copy A © copy', {
    decodeEntity,
    before: { literal: [{ start: 0, end: 5 }], excluded: [{ start: 0, end: 5 }] },
    after: { literal: [{ start: 0, end: 5 }], excluded: [{ start: 0, end: 5 }], roles: [role('link', 'A', 6, 7)] },
  });
  assert.deepEqual(masked(mask, before, 'copy', 0), [0, 0, 0, 0]);
  assert.deepEqual(masked(mask, before, 'copy', 1), [1, 1, 1, 1]);
});

test('a completed definition line disappears while code with the same spelling remains', () => {
  const before = '[word]:\n\n[word]:\n';
  const after = '[word]:\nword\n';
  const mask = disappearingSyntaxMask(before, after, {
    before: { excluded: [{ start: 0, end: 7 }] },
    after: { excluded: [{ start: 0, end: 7 }] },
  });
  assert.deepEqual(masked(mask, before, 'word', 0), [0, 0, 0, 0]);
  assert.deepEqual(masked(mask, before, 'word', 1), [1, 1, 1, 1]);
});

test('parser metadata identifies retired multiline reference titles', () => {
  const before = '  "tit\n';
  const mask = disappearingSyntaxMask(before, 'New paragraph\n', {
    definitionTitles: () => markdownDefinitionTitles('[x]: https://example.com\n  "title"\n\nNew paragraph'),
  });
  assert.deepEqual(masked(mask, before, 'tit'), [1, 1, 1]);
  const retained = `${'earlier '.repeat(100)}\n  "tit`;
  const keep = disappearingSyntaxMask(retained, `${retained}le"`, {
    definitionTitles: () => ['title'],
  });
  assert.equal(keep.some(Boolean), false);
});

test('an unclosed image alt contracts when its eventual image materializes', () => {
  const before = 'word ![word word\n';
  const mask = disappearingSyntaxMask(before, 'word  word word\n', {
    after: { roles: [role('image', 'word word', 5)] },
  });
  assert.deepEqual(masked(mask, before, 'word', 1), [1, 1, 1, 1]);
});


test('actual parser source positions retire incomplete HTML and delimiter prefixes', () => {
  for (const [before, source, needle] of [
    ['Prefix **', 'Prefix **word** then **', '**'],
    ['Prefix `', 'Prefix `word` then `', '`'],
    ['Prefix \\', 'Prefix \\* word tail', '\\'],
    ['Prefix &', 'Prefix &copy; word & tail', '&'],
    ['Prefix [', 'Prefix [word](https://example.com) [ tail', '['],
    ['Prefix ![', 'Prefix ![word](https://example.com/a.png) ![ tail', '!['],
    ['Prefix <span', 'Prefix <span>word</span> span tail', '<span'],
    ['Prefix <!--word', 'Prefix <!--word--> word tail', '<!--word'],
  ]) {
    const mask = disappearingSyntaxMask(before, '', { previousSource: before, source,
      sourceTokens: () => markdownSourceTokens(source), decodeEntity: value => value === '&copy;' ? '©' : value,
      isLiteralHtml: markdownHtmlTagIsLiteral });
    assert.equal(masked(mask, before, needle).every(Boolean), true, before);
  }
});

test('source syntax ownership preserves code and deliberately literal dangerous tags', () => {
  for (const [before, previousSource, source, needle, literal] of [
    ['Prefix <script', 'Prefix <script', 'Prefix <script>word</script> script tail', 'script', []],
    ['Prior word\n\n`**', 'Prior **word**\n\n`**', 'Prior **word**\n\n`**word**`', '**', []],
    ['Prefix **', 'Prefix `**', 'Prefix `**word`', '**', [{ start: 7, end: 9 }]],
  ]) {
    const mask = disappearingSyntaxMask(before, '', { previousSource, source,
      before: { literal }, sourceTokens: () => markdownSourceTokens(source),
      isLiteralHtml: markdownHtmlTagIsLiteral });
    assert.equal(masked(mask, before, needle).some(Boolean), false, previousSource);
  }
});


test('container projection retires incomplete multiline HTML attributes at their source boundary', () => {
  for (const [previousSource, source, before] of [
    ['> <span\n> title="word', '> <span\n> title="word">label</span> word tail', '\n<spantitle="word\n\n'],
    ['- <span\n  title="word', '- <span\n  title="word">label</span> word tail', '\n<spantitle="word\n\n'],
  ]) {
    const mask = disappearingSyntaxMask(before, '', { previousSource, source,
      sourceTokens: () => markdownSourceTokens(source) });
    assert.equal(mask[1], 1);
    assert.equal(masked(mask, before, 'word').every(Boolean), true);
  }
});


test('literal HTML policy recognizes the tag itself rather than nested attribute text', () => {
  assert.equal(markdownHtmlTagIsLiteral('<script title=">">'), true);
  assert.equal(markdownHtmlTagIsLiteral('<span title="<script>">'), false);
  assert.equal(markdownHtmlTagIsLiteral('<!-- <script> -->'), false);
});


test('normalized CRLF and tabbed containers retain original HTML source positions', () => {
  for (const previousSource of ['Prefix <span\r\n title="word', '- <span\n\ttitle="word']) {
    const source = `${previousSource}">label</span> word tail`;
    const before = previousSource.startsWith('-') ? '\n<span  title="word\n' : 'Prefix <span title="word\n';
    const mask = disappearingSyntaxMask(before, '', { previousSource, source, sourceTokens: () => markdownSourceTokens(source) });
    assert.equal(masked(mask, before, 'word').every(Boolean), true);
  }
});

test('completed closing fences retire former code characters but preserve retained code', () => {
  const previousSource = '```js\nword\n`';
  const source = `${previousSource}\`\`\n\nnew`;
  const before = 'word\n`\n';
  const mask = disappearingSyntaxMask(before, 'word\nnew\n', { previousSource, source,
    before: { literal: [{ start: 0, end: 6 }] }, sourceTokens: () => markdownSourceTokens(source) });
  assert.deepEqual([...mask.slice(0, 6)], [0, 0, 0, 0, 1, 1]);
});

test('a selected live tail after a large table uses source ownership without cell rescans', () => {
  const table = '| Header | Value |\n| --- | --- |\n' + Array.from({ length: 16000 }, (_, index) => `| Key${index} | Value${index} |`).join('\n');
  const previousSource = `${table}\n\nPrefix <span`;
  const source = `${previousSource}>label</span> span tail`;
  const mask = disappearingSyntaxMask('Prefix <span', 'Prefix label span tail', { previousSource, source,
    sourceTokens: () => markdownSourceTokens(source) });
  assert.deepEqual([...mask.slice(7)], [1, 1, 1, 1, 1]);
});


test('incomplete ordered-list digits retire for populated and empty items', () => {
  for (const [before, source] of [['1', '1. alpha\n\n1 tail'], ['42', '42) alpha\n\n42 tail'], ['1', '1.\n\n1 tail'], ['42', '42)\n\n42 tail']]) {
    const mask = disappearingSyntaxMask(before, '', { previousSource: before, source,
      sourceTokens: () => markdownSourceTokens(source) });
    assert.equal([...mask].every(Boolean), true, source);
  }
});


test('parser task metadata retires trimmed empty checkboxes while literal labels survive', () => {
  for (const [previousSource, source, before] of [
    ['- [x', '- [x] \n\nx tail', '\n[x\n\n'],
    ['- [X]', '- [X] \n\nX tail', '\n[X]\n\n'],
    ['- [', '- [ ] \n\n[ tail', '\n[\n\n'],
  ]) {
    const mask = disappearingSyntaxMask(before, '', { previousSource, source, sourceTokens: () => markdownSourceTokens(source) });
    assert.equal(mask[1], 1);
    assert.equal(mask.slice(1, before.trimEnd().length).every(Boolean), true);
  }
  const previousSource = '- [x';
  const source = '- [x]';
  const retained = disappearingSyntaxMask('\n[x\n\n', '\n[x]\n\n', { previousSource, source,
    sourceTokens: () => markdownSourceTokens(source) });
  assert.equal(retained.some(Boolean), false);
});


test('bounded event fallback matches nested image labels after large literal histories', () => {
  const history = '[q][literal] '.repeat(600);
  const before = `${history}![word x ![alt][image]][outer]`;
  const mask = disappearingSyntaxMask(before, `${history}\nword tail`, {
    after: { roles: [role('image', 'word x alt', history.length)] },
  });
  assert.equal(masked(mask, before, 'word').every(Boolean), true);
  assert.equal(mask.slice(0, history.length).some(Boolean), false);
  const growing = `${history}![word`;
  const retained = disappearingSyntaxMask(growing, `${history}![word] word`, {
    after: { roles: [role('link', 'word', history.length + 8, history.length + 12)] },
  });
  assert.equal(masked(retained, growing, 'word').some(Boolean), false);
});

test('inline entity prefixes remain literal until Marked receives a semicolon', () => {
  const previousSource = 'Prefix &';
  const source = 'Prefix &copy word & tail';
  const mask = disappearingSyntaxMask(previousSource, source, { previousSource, source,
    sourceTokens: () => markdownSourceTokens(source),
    decodeEntity: value => value === '&copy' || value === '&copy;' ? '©' : value });
  assert.equal(mask.some(Boolean), false);
});


test('growing literal entities retain their identity before later decoded symbols', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const suffix of ['cat and ©']) {
      const before = `${history}Prefix &copy`;
      const mask = disappearingSyntaxMask(before, `${before}${suffix}`, { decodeEntity });
      assert.equal(mask.some(Boolean), false);
    }
  }
});

test('unclosed literal destinations retain their identity before new links and images', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const [tail, suffix, kind] of [
      ['![word](', 'bad\n\n', 'image'], ['[word](http', 'bad\n\n', 'link'],
    ]) {
      const before = `${history}Prefix ${tail}`;
      const after = `${before}${suffix}${kind === 'link' ? 'word' : ''}`;
      const mask = disappearingSyntaxMask(before, after, {
        after: { roles: [role(kind, 'word', before.length + suffix.length, after.length)] },
      });
      assert.equal(mask.some(Boolean), false);
    }
  }
});

test('previous parser projection retires formatted image labels before new literal duplicates', () => {
  for (const [tail, rendered, literal] of [
    ['**bold**', 'bold', []], ['`code`', 'code', [{ start: 7, end: 11 }]], ['<b>html</b>', 'html', []],
  ]) {
    const previousSource = `![gone ${tail}`;
    const source = `${previousSource}](https://example.com/i.png)\n\n![gone ${rendered}`;
    const before = `![gone ${rendered}\n`;
    const mask = disappearingSyntaxMask(before, `\n![gone ${rendered}\n`, { previousSource, source,
      before: { literal }, sourceTokens: () => markdownSourceTokens(source),
      previousSourceTokens: () => markdownSourceTokens(previousSource),
      after: { roles: [role('image', `gone ${rendered}`, 0)] },
    });
    assert.equal(masked(mask, before, 'gone').every(Boolean), true);
    assert.equal(masked(mask, before, rendered).every(Boolean), true);
  }
});


test('unfinished literal labels retain continuation identity when their inner formatting closes', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const marker of ['**', '`']) {
      const before = `${history}![gone ${marker}word`;
      const after = `${history}![gone word\n\n`;
      const mask = disappearingSyntaxMask(before, after, { after: { roles: [role('image', 'gone word', after.length)] } });
      assert.equal(masked(mask, before, 'gone').some(Boolean), false);
    }
  }
});


test('actual browser partial entity decoding retires the original entity prefix', () => {
  const previousSource = 'Prefix &copy';
  const source = 'Prefix &copycat; and ©';
  const mask = disappearingSyntaxMask(previousSource, 'Prefix ©cat; and ©', { previousSource, source,
    sourceTokens: () => markdownSourceTokens(source),
    decodeEntity: value => value === '&copycat;' ? '©cat;' : decodeEntity(value),
  });
  assert.equal(masked(mask, previousSource, 'copy').every(Boolean), true);
});


test('source ownership preserves partial image labels through entity and HTML completion', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const [tail, suffix, rendered] of [
      ['&co', 'py;', '©'], ['<em', '>bold</em>', 'bold'],
    ]) {
      const previousSource = `${history}![gone ${tail}`;
      const source = `${previousSource}${suffix}\n\n![gone ${tail}](https://example.com/i.png)`;
      const before = `${previousSource}\n`;
      const after = `${history}![gone ${rendered}\n\n`;
      const mask = disappearingSyntaxMask(before, after, { previousSource, source,
        sourceTokens: () => markdownSourceTokens(source),
        decodeEntity, after: { roles: [role('image', `gone ${tail}`, after.length)] },
      });
      assert.equal(masked(mask, before, 'gone').some(Boolean), false);
    }
  }
});


test('source ownership preserves unfinished image destinations through inline parser changes', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const [tail, suffix, rendered] of [
      ['<em', '>bold</em>', 'bold'], ['a &co', 'py;', 'a ©'],
    ]) {
      const previousSource = `${history}![gone](${tail}`;
      const source = `${previousSource}${suffix}\n\n![gone](https://example.com/i.png)`;
      const after = `${history}![gone](${rendered}\n\n`;
      const mask = disappearingSyntaxMask(`${previousSource}\n`, after, { previousSource, source,
        sourceTokens: () => markdownSourceTokens(source), decodeEntity,
        after: { roles: [role('image', 'gone', after.length)] },
      });
      assert.equal(masked(mask, previousSource, 'gone').some(Boolean), false);
    }
  }
});


test('source authority still retires actually completed image destinations and references', () => {
  for (const [previousSource, suffix, before] of [
    ['![gone](https://example.com', '/i.png)', '![gone](https://example.com\n'],
    ['![**gone**](<em', '>)', '![gone](<em\n'],
    ['![**gone**][re', 'f]\n\n[ref]: https://example.com/i.png', '![gone][re\n'],
  ]) {
    const source = `${previousSource}${suffix}\n\ngone tail`;
    const mask = disappearingSyntaxMask(before, '\ngone tail\n', { previousSource, source,
      sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
      after: { roles: [role('image', 'gone', 0)] },
    });
    assert.equal(masked(mask, before, 'gone').every(Boolean), true, previousSource);
  }
});


test('closed literal image shortcuts retain identity while a reference or destination is appended', () => {
  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const suffix of ['[missing]', '(bad']) {
      const before = `${history}![gone]`;
      const after = `${before}${suffix}\n\n`;
      const mask = disappearingSyntaxMask(before, after, { after: { roles: [role('image', 'gone', after.length)] } });
      assert.equal(mask.some(Boolean), false);
    }
  }
});

test('literal HTML policy reuses the established dangerous-tag word boundary', () => {
  for (const name of ['script', 'style', 'iframe', 'object', 'embed', 'link', 'meta']) {
    assert.equal(markdownHtmlTagIsLiteral(`<${name}-x>`), true);
    assert.equal(markdownHtmlTagIsLiteral(`</${name}-x>`), true);
    assert.equal(markdownHtmlTagIsLiteral(`<${name}_x>`), false);
  }
});


test('a formatted image shortcut at the old source tail retires before a new literal duplicate', () => {
  const previousSource = '![**gone**]';
  const source = `${previousSource}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`;
  const before = '![gone]\n';
  const mask = disappearingSyntaxMask(before, '\n![gone]\n', { previousSource, source,
    sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
    after: { roles: [role('image', 'gone', 0)] },
  });
  assert.equal(masked(mask, before, 'gone').every(Boolean), true);
});


test('source projection retires formatted image shortcuts before retained later paragraphs', () => {
  for (const tail of [' more', '\n\nMore paragraph']) {
    const previousSource = `![**gone**]${tail}`;
    const source = `${previousSource}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`;
    const before = `![gone]${tail}\n`;
    const mask = disappearingSyntaxMask(before, `${tail}\n![gone]\n`, { previousSource, source,
      sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
      after: { roles: [role('image', 'gone', 0)] },
    });
    assert.equal(masked(mask, before, 'gone').every(Boolean), true);
    assert.equal(mask.slice(before.indexOf(tail)).some(Boolean), false);
  }
});


test('unrelated headings and raw HTML entities preserve exact image source ownership', () => {
  for (const [prefix, visible] of [
    ['Title\n=====\n\n', 'Title\n'], ['# Title #\n\n', 'Title\n'],
    ['<p>&amp;</p>\n\n', '&\n'], ['<div>&copy;</div>\n\n', '©\n'],
  ]) {
    const previousSource = `${prefix}![**gone**] more`;
    const source = `${previousSource}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`;
    const before = `${visible}![gone] more\n`;
    const mask = disappearingSyntaxMask(before, `${visible} more\n![gone]\n`, { previousSource, source,
      sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
      decodeEntity: value => value === '&amp;' ? '&' : decodeEntity(value),
      after: { roles: [role('image', 'gone', visible.length)] },
    });
    assert.equal(masked(mask, before, 'gone').every(Boolean), true, prefix);
    assert.equal(mask.slice(0, visible.length).some(Boolean), false, prefix);
  }
});


test('parser-decoded raw HTML entities retire inside CODE and PRE literal DOM ranges', () => {
  for (const [opening, closing] of [['<code>', '</code>'], ['<pre>', '</pre>'], ['<pre><code>', '</code></pre>']]) {
    const previousSource = `${opening}Prefix &cop`;
    const source = `${previousSource}y;${closing} cop tail`;
    const before = 'Prefix &cop';
    const mask = disappearingSyntaxMask(before, 'Prefix © cop tail', { previousSource, source,
      sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
      before: { literal: [{ start: 0, end: before.length }] }, decodeEntity,
    });
    assert.equal(masked(mask, before, 'cop').every(Boolean), true);
  }
});


test('already decoded HTML entities retire changed values and retain decoded prefixes', () => {
  const decode = value => ({ '&#65': 'A', '&#650;': 'ʊ', '&#650': 'ʊ', '&not': '¬', '&notin;': '∉',
    '&copy': '©', '&copycat;': '©cat;', '&#9': '\t', '&#97;': 'a' })[value] || value;
  for (const [entity, append, before, after, retired] of [
    ['&#65', '0;', 'A', 'ʊ', true], ['&#65', '0', 'A', 'ʊ', true],
    ['&not', 'in;', '¬', '∉', true], ['&copy', 'cat;', '©', '©cat;', false],
    ['&#9', '7;', '\t', 'a', true],
  ]) {
    for (const [opening, closing] of [['<code>', '</code>'], ['<pre>', '</pre>']]) {
      const previousSource = `${opening}${entity}`;
      const source = `${previousSource}${append}${closing} ${before} tail`;
      for (const layout of ['', '\n']) {
        const mask = disappearingSyntaxMask(before + layout, `${after} ${before} tail`, { previousSource, source,
          sourceTokens: () => markdownSourceTokens(source), previousSourceTokens: () => markdownSourceTokens(previousSource),
          before: { literal: [{ start: 0, end: before.length + layout.length }] }, decodeEntity: decode,
        });
        assert.equal([...mask.slice(0, before.length)].every(Boolean), retired, `${previousSource} + ${append}`);
      }
    }
  }
});
