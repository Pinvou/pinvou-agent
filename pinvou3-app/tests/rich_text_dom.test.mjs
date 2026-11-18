import assert from 'node:assert/strict';
import { test } from 'node:test';
import domino from '@mixmark-io/domino';
import { updateRichText } from '../src/features/conversation/rich-text-dom.js';
import { ensureCodeCopyButtons, copyCodeBlockFromButton, isCodeCopyControl } from '../src/features/conversation/code-block-copy.js';
import { renderMarkdownMarkup } from '../src/shared/markdown-renderer.js';

const labels = { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' };
const createRoot = () => domino.createDocument('<main></main>').querySelector('main');
const codeHtml = text => `<p>Earlier paragraph</p><pre class="pinvou-code-block"><code>${text}</code></pre>`;

test('streamed text retains earlier blocks, the current text node and code controls', () => {
  const root = createRoot();
  updateRichText(root, codeHtml('const alpha = 1;'));
  ensureCodeCopyButtons(root, labels);
  const paragraph = root.querySelector('p');
  const pre = root.querySelector('pre');
  const text = root.querySelector('code').firstChild;
  const button = root.querySelector('button');
  updateRichText(root, codeHtml('const alpha = 1;\nconst beta = 2;'));
  ensureCodeCopyButtons(root, labels);
  assert.equal(root.querySelector('p'), paragraph);
  assert.equal(root.querySelector('pre'), pre);
  assert.equal(root.querySelector('code').firstChild, text);
  assert.equal(root.querySelector('button'), button);
  assert.equal(root.querySelector('code').textContent, 'const alpha = 1;\nconst beta = 2;');
});

test('an in-flight copy snapshots code and delivers feedback to the visible button', async () => {
  const root = createRoot();
  updateRichText(root, codeHtml('first'));
  ensureCodeCopyButtons(root, labels);
  const button = root.querySelector('button');
  const writes = [];
  let finish;
  const pending = copyCodeBlockFromButton(button, { labels, writeText: value => {
    writes.push(value);
    return new Promise(resolve => { finish = resolve; });
  } });
  updateRichText(root, codeHtml('first and more'));
  ensureCodeCopyButtons(root, labels);
  finish(true);
  assert.equal(await pending, true);
  assert.deepEqual(writes, ['first']);
  assert.equal(root.querySelector('button'), button);
  assert.equal(button.title, 'Copied');
  updateRichText(root, codeHtml('first and more again'));
  ensureCodeCopyButtons(root, labels);
  assert.equal(button.title, 'Copied');
});

test('source changes remove stale structure and attributes without retaining forged controls', () => {
  const root = createRoot();
  updateRichText(root, '<h1 id="old">Title</h1><p>Removed</p>');
  updateRichText(root, '<h1 class="new">Replacement</h1>');
  assert.equal(root.innerHTML, '<h1 class="new">Replacement</h1>');
  updateRichText(root, '<pre class="pinvou-code-block"><code>x</code><button class="pinvou-code-copy-btn">fake</button></pre>');
  ensureCodeCopyButtons(root, labels);
  assert.equal(isCodeCopyControl(root.querySelector('pre').children[1]), false);
  updateRichText(root, codeHtml('new'));
  ensureCodeCopyButtons(root, labels);
  assert.equal(root.querySelectorAll('button').length, 1);
  assert.equal(isCodeCopyControl(root.querySelector('button')), true);
  updateRichText(root, '<pre><code>ordinary preformatted HTML</code></pre>');
  assert.equal(root.querySelectorAll('button').length, 0, 'a block losing its code-block role must lose its controls');
});

test('highlight changes and repeated appends keep final Markdown DOM equivalent', () => {
  const root = createRoot();
  const values = [
    '<p>Before</p><pre class="pinvou-code-block"><code>let x</code></pre>',
    '<p>Before</p><pre class="pinvou-code-block"><code><span class="hljs-keyword">let</span> x = 1;</code></pre><p>After</p>',
    '<h2>Changed</h2><ul><li>A</li><li>B</li></ul><table><tbody><tr><td>Cell</td></tr></tbody></table>',
    '',
  ];
  for (const html of values) {
    updateRichText(root, html);
    assert.equal(root.innerHTML, html);
  }
});

test('every streamed Markdown prefix matches a fresh render across block transitions', () => {
  const source = [
    'Heading\n=======',
    'A paragraph with **bold**, `inline code` and [a link](https://example.com).',
    '- first item\n- second item\n\n  nested paragraph',
    '| Column | Value |\n| --- | --- |\n| A | B |',
    '```javascript\nconst alpha = 1;\nconsole.log(alpha);\n```',
    '> Quoted **text**\n>\n> second paragraph',
  ].join('\n\n');
  const root = createRoot();
  for (let end = 1; end <= source.length; end += 1) {
    const html = renderMarkdownMarkup(source.slice(0, end));
    updateRichText(root, html);
    ensureCodeCopyButtons(root, labels);
    const content = root.cloneNode(true);
    for (const control of content.querySelectorAll('.pinvou-code-copy-btn, .pinvou-code-copy-live')) control.remove();
    const expected = createRoot();
    expected.innerHTML = html;
    assert.equal(content.innerHTML, expected.innerHTML, `DOM diverged at Markdown prefix ${end}`);
  }
});

test('loose-list wrappers retain a nested code block and its in-flight feedback', async () => {
  const root = createRoot();
  const before = '- Step\n  ```js\n  const x = 1;';
  updateRichText(root, renderMarkdownMarkup(before));
  ensureCodeCopyButtons(root, labels);
  const button = root.querySelector('button');
  const pre = root.querySelector('pre');
  let finish;
  const pending = copyCodeBlockFromButton(button, { labels, writeText: () => new Promise(resolve => { finish = resolve; }) });
  const html = renderMarkdownMarkup(`${before}\n  \`\`\`\n\n  More output`);
  updateRichText(root, html);
  ensureCodeCopyButtons(root, labels);
  assert.equal(root.querySelector('pre'), pre);
  assert.equal(root.querySelector('button'), button);
  finish(true);
  assert.equal(await pending, true);
  assert.equal(root.querySelector('button').title, 'Copied');
});

test('paragraph promotion keeps its text in the heading instead of a new duplicate paragraph', () => {
  const root = createRoot();
  updateRichText(root, '<p>Title</p>\n');
  const text = root.querySelector('p').firstChild;
  updateRichText(root, '<h1>Title</h1>\n<p>Title</p>\n');
  assert.equal(root.querySelector('h1').firstChild, text);
  assert.notEqual(root.querySelector('p').firstChild, text);
});

test('equal HTML revalidates a replaced source opening while identical input remains stable', async () => {
  const root = createRoot();
  const before = '```js\nsame\n```';
  const after = '~~~js\nsame\n~~~';
  const html = renderMarkdownMarkup(before);
  assert.equal(renderMarkdownMarkup(after), html);
  updateRichText(root, html, { markdownSource: before });
  ensureCodeCopyButtons(root, labels);
  const button = root.querySelector('button');
  updateRichText(root, html, { markdownSource: before });
  assert.equal(root.querySelector('button'), button);
  let finish;
  const pending = copyCodeBlockFromButton(button, { labels,
    writeText: () => new Promise(resolve => { finish = resolve; }) });
  updateRichText(root, html, { markdownSource: after });
  ensureCodeCopyButtons(root, labels);
  const replacement = root.querySelector('button');
  assert.notEqual(replacement, button);
  finish(true);
  await pending;
  assert.equal(root.contains(button), false);
  assert.equal(replacement.title, labels.copyCode);
});
