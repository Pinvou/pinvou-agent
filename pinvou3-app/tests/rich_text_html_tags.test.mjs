import assert from 'node:assert/strict';
import { test } from 'node:test';
import { preTagOffsets, sourceBlockTagOffsets, tagEnd } from '../src/features/conversation/rich-text-html-tags.js';
import { markdownHtmlTagIsLiteral } from '../src/shared/markdown-renderer.js';

test('PRE source tags exclude quoted attributes and closing-tag attributes', () => {
  const html = `<pre data-x="<pre>" title='> <pre>'>first</pre title="<pre>"><PRE>second</PRE>`;
  assert.deepEqual(preTagOffsets(html), [0, html.indexOf('<PRE>')]);
});

test('PRE source tags ignore comments and declarations', () => {
  const html = '<!-- <pre> --> <!DOCTYPE x "<pre>"><pre>real</pre>';
  assert.deepEqual(preTagOffsets(html), [html.indexOf('<pre>real')]);
});

test('abrupt and alternative comment endings do not hide following tags', () => {
  for (const comment of ['<!-->', '<!--->', '<!-- text --!>']) {
    const html = `${comment}<pre>real</pre>`;
    assert.deepEqual(preTagOffsets(html), [comment.length]);
  }
});

test('raw text cannot supply a PRE tag', () => {
  for (const name of ['textarea', 'title', 'xmp', 'noscript', 'script']) {
    const html = `<${name}><pre>text</pre></${name}><pre>real</pre>`;
    assert.deepEqual(preTagOffsets(html), [html.lastIndexOf('<pre>')]);
  }
  assert.deepEqual(preTagOffsets('<plaintext><pre>text</pre></plaintext><pre>still text</pre>'), []);
});

test('renderer-literal dangerous tags leave their inner markup active', () => {
  const html = '<script><pre>content</pre></script><pre>tail</pre>';
  assert.deepEqual(preTagOffsets(html, markdownHtmlTagIsLiteral), [html.indexOf('<pre>'), html.lastIndexOf('<pre>')]);
});

test('literal less-than signs do not consume a following tag', () => {
  const html = '<div>x < y <pre>real</pre></div>';
  assert.deepEqual(preTagOffsets(html), [html.indexOf('<pre>')]);
});

test('unclosed quoted tags cannot supply a PRE element', () => {
  const html = '<pre title="> <pre>';
  assert.equal(tagEnd(html, 0), -1);
  assert.deepEqual(preTagOffsets(html), []);
});


test('bogus comments ignore quotes and unquoted attribute quotes remain unquoted', () => {
  for (const prefix of ['<?x "\'>', '<!x "\'>', '<div data-x=a"b>', "<div data-x=a'b>"]) {
    const html = `${prefix}<pre>real</pre>`;
    assert.equal(tagEnd(html, 0), prefix.length);
    assert.deepEqual(preTagOffsets(html), [prefix.length]);
  }
});

test('raw text end tags accept a self-closing flag before following PRE', () => {
  const html = '<textarea>text</textarea/><pre>real</pre>';
  assert.deepEqual(preTagOffsets(html), [html.indexOf('<pre>')]);
});


test('HTML attribute names and non-HTML spaces cannot start a quoted value', () => {
  for (const prefix of ['<div ="x>', "<div ='x>", '<div /="x>', '<div a=\u00a0"x>',
    '<div a=\v"x>', '<div a=x\u00a0="y>', '<div a="x"="y>']) {
    const html = `${prefix}<pre>real</pre>`;
    assert.equal(tagEnd(html, 0), prefix.length);
    assert.deepEqual(preTagOffsets(html), [prefix.length]);
  }
});

test('CDATA-looking markup in HTML is a bogus comment ending at the first greater-than', () => {
  for (const prefix of ['<![CDATA[>', '<![CDATA[>plain text']) {
    const html = `${prefix}<pre>real</pre>]]>`;
    assert.deepEqual(preTagOffsets(html), [prefix.length]);
  }
});

test('source block tags preserve names and insertion lengths across tables and code', () => {
  const html = '<div title="<td>fake</td>"><!--<caption>--><table><caption>C</caption><tr><th>H</th><td><pre>P</pre></td></tr></table></div>';
  assert.deepEqual(sourceBlockTagOffsets(html), ['table', 'caption', 'th', 'td', 'pre']
    .map(name => ({ offset: html.lastIndexOf(`<${name}>`), name })));
  const raw = '<textarea><table><td>hidden</td></table></textarea><TABLE><TD>visible</TD></TABLE>';
  assert.deepEqual(sourceBlockTagOffsets(raw),
    [{ offset: raw.indexOf('<TABLE>'), name: 'table' }, { offset: raw.indexOf('<TD>'), name: 'td' }]);
});
