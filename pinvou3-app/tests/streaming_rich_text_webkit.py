"""Opt-in native WebKitGTK selection regression against the React fixture.

Run through PINVOU_NATIVE_WEBKIT=1 npm run test:streaming-rich-text. Requires
Python GI, GTK 3, WebKit2 4.1 and a display; no system clipboard is written.
"""

import json
import os
import sys

os.environ.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")

import gi  # noqa: E402

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import GLib, Gtk, WebKit2  # noqa: E402


CHECKS = r"""
(async () => {
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  const check = (condition, message) => { if (!condition) throw new Error(message); };
  for (let i = 0; i < 100 && !window.streamingFixture; i++) await wait(100);
  const fixture = window.streamingFixture;
  check(Boolean(fixture), 'React fixture did not load');
  const render = async (text, active = true) => { fixture.render(text, active); await wait(350); };
  const selected = () => getSelection().toString();
  await render('Native first paragraph\n\nNative current paragraph');
  const cross = fixture.select('#root .conversation-markdown', 'paragraph\nNative');
  await render('Native first paragraph\n\nNative current paragraph grows');
  check(selected() === cross, 'Cross-paragraph selection changed');
  fixture.select('#root .conversation-markdown', 'current paragraph', true);
  await render('Native first paragraph\n\nNative current paragraph grows again');
  check(selected() === 'current paragraph', 'Current paragraph selection changed');
  const range = getSelection().getRangeAt(0);
  check(getSelection().anchorNode !== range.startContainer || getSelection().anchorOffset !== range.startOffset,
    'Backward selection direction changed');
  await render('正在生成😀，选择这段文字');
  fixture.select('#root .conversation-markdown', '😀，选择');
  await render('正在生成😀，选择这段文字，后续继续出现');
  check(selected() === '😀，选择', 'CJK/emoji selection changed');
  for (const animation of [false, true]) {
    fixture.setAnimation(animation);
    for (const [before, after, word] of [
      ['Prefix **selected', 'Prefix **selected** trailing', 'selected'],
      ['Prefix **selected', 'Prefix **selected** and', 'selected'],
      ['Prefix **selected', 'Prefix **selected** then **selecte', 'selected'],
      ['Prefix **selected\n', 'Prefix **selected**' + ' selected'.repeat(4000) + '\n', 'selected'],
      ['Prefix **selected', 'Prefix **selected** selected', 'selected'],
      ['Prefix `selected', 'Prefix `selected` trailing', 'selected'],
      ['Prefix [selected', 'Prefix [selected](https://example.com) trailing', 'selected'],
      ['- Alpha item\n- Bravo selected\n\n  ', '- Alpha item\n- Bravo selected\n\n  C', 'Bravo'],
    ]) {
      await render(before);
      fixture.select('#root .conversation-markdown', word, true);
      await render(after);
      check(selected() === word, 'Markdown closure or loose-list conversion lost selection');
      if (before.startsWith('Prefix **')) {
        const range = getSelection().getRangeAt(0);
        const prefix = document.createRange();
        prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
        prefix.setEnd(range.startContainer, range.startOffset);
        check(prefix.toString() === 'Prefix ', 'Selection jumped to repeated appended text');
      }
    }
  }
  for (const word of ['选择', '😀', 'word']) {
    await render(`[A][long_reference] ${word} ${word}`);
    fixture.select('#root .conversation-markdown', word, true);
    await render(`[A][long_reference] ${word} ${word}\n\n[long_reference]: https://example.com`);
    const range = getSelection().getRangeAt(0);
    const prefix = document.createRange();
    prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
    prefix.setEnd(range.startContainer, range.startOffset);
    check(selected() === word && prefix.toString() === 'A ', 'Reference prefix conversion selected a later occurrence');
  }
  for (const [whitespace, value, backward] of [['  ', 'hello  ', false], ['\t', '\t', true]]) {
    const before = `[A][ref]\n\n\`\`\`\nhello${whitespace}`;
    await render(before);
    fixture.select('#root code', value, backward);
    check(selected() === value, 'Fixture must select source-owned code whitespace');
    await render(`${before}world\n\`\`\`\n\n[ref]: /x`);
    check(selected() === value, 'Code whitespace lost after preceding reference rewrite');
    check(getSelection().anchorNode.parentElement.closest('code') !== null, 'Code whitespace selected a different block');
  }
  for (const [opening, closing, expected] of [
    ['<code>', '</code>', ''], ['<pre><code>', '</code></pre>', ''], ['`', '`', 'cop'],
  ]) {
    const before = `${opening}Prefix &cop`;
    await render(before);
    fixture.select('#root .conversation-markdown', 'cop', true);
    await render(`${before}y;${closing} cop tail`);
    check(selected() === expected, 'HTML entity decoding confused Markdown code ownership');
    if (!expected) check(getSelection().isCollapsed, 'Retired HTML entity selected appended spelling');
  }

  for (const [opening, closing, entity, suffix, word, expected] of [
    ['<pre>', '</pre>', '&#65', '0;', 'A', ''],
    ['<code>', '</code>', '&not', 'in;', '¬', ''],
    ['<pre>', '</pre>', '&#9', '7;', '\t', ''],
    ['<code>', '</code>', '&copy', 'cat;', '©', '©'],
  ]) {
    const before = `${opening}Prefix ${entity}`;
    await render(before);
    fixture.select('#root .conversation-markdown', word, true);
    check(selected() === word, 'Fixture must select the already decoded entity');
    await render(`${before}${suffix}${closing} ${word} tail`);
    check(selected() === expected && getSelection().isCollapsed === !expected,
      'Already decoded entity moved selection onto an appended occurrence');
  }


  for (const [before, suffix, word] of [
    ['<div>A<span title="&#65', '0;"></span></div> A tail', 'A'],
    ['<div>cop<span title="&cop', 'y;"></span></div> cop tail', 'cop'],
    ['<div>A<!--&#65', '0;--></div> A tail', 'A'],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word, true);
    await render(before + suffix);
    check(selected() === word, 'Hidden HTML entities retired normal visible text');
    const range = getSelection().getRangeAt(0);
    const prefix = document.createRange();
    prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
    prefix.setEnd(range.startContainer, range.startOffset);
    check(prefix.toString() === '', 'Hidden HTML entities retargeted duplicate text');
  }


  await render('<table><tr><td>word</td></tr>');
  fixture.select('#root td', 'word', true);
  await render('<table><tr><td>word</td></tr>word outside</table>');
  check(selected() === 'word' && getSelection().anchorNode.parentElement.closest('td'),
    'HTML foster parenting moved selection to newly inserted prose');


  {
    const host = document.createElement('div');
    const external = document.createElement('p');
    external.textContent = 'Other message';
    host.id = 'native-table-endpoints';
    document.body.append(host, external);
    try {
      for (const caption of [false, true]) {
        for (const backward of [false, true]) {
          getSelection().removeAllRanges();
          const word = caption ? 'same' : 'word';
          const before = caption ? '<table><caption>same</caption><tr><td>cell</td></tr>' : '<table><tr><td>word</td></tr>';
          fixture.updateRichText(host, fixture.renderHtml(before), { markdownSource: before });
          const part = host.querySelector(caption ? 'caption' : 'td');
          fixture.select(`#native-table-endpoints ${caption ? 'caption' : 'td'}`, word);
          const range = getSelection().getRangeAt(0);
          const first = [range.startContainer, range.startOffset];
          const last = caption ? [range.endContainer, range.endOffset] : [external.firstChild, 5];
          getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
          const initial = selected();
          const after = `${before}${word} outside</table>`;
          fixture.updateRichText(host, fixture.renderHtml(after), { markdownSource: after });
          const endpoint = backward ? getSelection().focusNode : getSelection().anchorNode;
          check(selected() === initial && part.contains(endpoint), 'Single table/caption endpoint moved to new prose');
        }
      }
    } finally { host.remove(); external.remove(); }
  }


  {
    const host = document.createElement('div');
    host.id = 'native-growing-table';
    document.body.append(host);
    try {
      for (const tag of ['td', 'th', 'caption']) {
        for (const formatting of ['', '\n\n**']) {
          const opening = tag === 'caption' ? '<table><caption>' : `<table><tr><${tag}>`;
          const closing = tag === 'caption' ? '</caption><tr><td>cell</td></tr>' : `</${tag}></tr>`;
          const before = opening + formatting + 'word';
          fixture.updateRichText(host, fixture.renderHtml(before), { markdownSource: before });
          const part = host.querySelector(tag);
          fixture.select(`#native-growing-table ${tag}`, 'word', true);
          const after = `${before}${formatting ? '**' : ''} retained${closing}word outside</table>`;
          fixture.updateRichText(host, fixture.renderHtml(after), { markdownSource: after });
          check(selected() === 'word' && part.contains(getSelection().anchorNode)
            && part.contains(getSelection().focusNode), 'Growing/formatted table part selected new prose');
        }
      }
    } finally { host.remove(); }
  }


  {
    const host = document.createElement('div');
    const external = document.createElement('p');
    host.id = 'native-growing-cross';
    external.textContent = 'Other message';
    document.body.append(host, external);
    try {
      for (const backward of [false, true]) {
        const before = '<table><tr><td>\n\n**word';
        fixture.updateRichText(host, fixture.renderHtml(before), { markdownSource: before });
        const part = host.querySelector('td');
        fixture.select('#native-growing-cross td', 'word');
        const range = getSelection().getRangeAt(0);
        const first = [range.startContainer, range.startOffset];
        const last = [external.firstChild, 5];
        getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
        const after = `${before}** retained</td></tr>word outside</table>`;
        fixture.updateRichText(host, fixture.renderHtml(after), { markdownSource: after });
        const internal = backward ? getSelection().focusNode : getSelection().anchorNode;
        const outside = backward ? getSelection().anchorNode : getSelection().focusNode;
        check(part.contains(internal) && outside === external.firstChild,
          'Cross-message ordering clipped away a retained formatted table endpoint');
      }
    } finally { host.remove(); external.remove(); }
  }

  {
    const { setStreamFadePreference, applyStreamFade } = await import('/src/features/conversation/stream-fade.js');
    const { ensureCodeCopyButtons, copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
    const host = document.createElement('div');
    document.body.append(host);
    const labels = { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' };
    const commit = source => {
      fixture.updateRichText(host, fixture.renderHtml(source), { markdownSource: source });
      ensureCodeCopyButtons(host, labels);
    };
    try {
      for (const wrapper of ['none', 'div', 'table', 'ul', 'blockquote']) {
        for (const same of [false, true]) {
          for (const index of [0, 1]) {
            for (const success of [false, true]) {
              getSelection().removeAllRanges();
              const block = value => {
                const pre = `<pre class=pinvou-code-block><code>${value}</code></pre>`;
                if (wrapper === 'none') return pre;
                if (wrapper === 'table') return `<table><tr><td>${pre}</td></tr></table>`;
                if (wrapper === 'ul') return `<ul><li>${pre}</li></ul>`;
                return `<${wrapper}>${pre}</${wrapper}>`;
              };
              const before = `![${block(same ? 'same' : 'gone')}][x]\n\n${block(same ? 'same' : 'stay')}`;
              const after = `${before}\n\n[x]: https://example.com/i.png${same ? `\n\n${block('same')}` : ''}`;
              commit(before);
              const buttons = [...host.querySelectorAll('.pinvou-code-copy-btn')];
              let finish;
              let snapshot;
              const pending = copyCodeBlockFromButton(buttons[index], { labels, writeText: value => {
                snapshot = value;
                return new Promise(resolve => { finish = resolve; });
              } });
              commit(after);
              const current = [...host.querySelectorAll('.pinvou-code-copy-btn')];
              finish(success);
              await pending;
              check(!buttons[0].isConnected && current[0] === buttons[1], 'Markdown-retired PRE moved copy controls');
              check(snapshot === (same ? 'same' : index === 0 ? 'gone' : 'stay'), 'Code copy snapshot changed');
              check(current[0].title === (index === 0 ? labels.copyCode : success ? labels.copied : labels.failed),
                'Copy feedback moved across Markdown source blocks');
              check(!same || (!buttons.includes(current[1]) && current[1].title === labels.copyCode),
                'New equal code block borrowed old feedback');
            }
          }
        }
      }
      for (const partial of [false, true]) {
        for (const prefix of ['', 'Earlier paragraph\n\n']) {
          for (const success of [false, true]) {
            const host = document.createElement('div');
            host.id = 'adopt-code-probe';
            document.body.append(host);
            try {
              const before = `${prefix}<pre class=pinvou-code-block><code>stay</code></pre>\n\nFollowing paragraph`;
              fixture.updateRichText(host, fixture.renderHtml(before), { markdownSource: before });
              ensureCodeCopyButtons(host, labels);
              const button = host.querySelector('.pinvou-code-copy-btn');
              fixture.select('#adopt-code-probe code', 'stay', true);
              let finish;
              const pending = copyCodeBlockFromButton(button, { labels,
                writeText: () => new Promise(resolve => { finish = resolve; }) });
              const after = `${before}\n\nNew prose`;
              const wrap = html => partial ? html.replace(/(<pre[\s\S]*?<\/pre>)/, '<article>$1</article>') : `<article>${html}</article>`;
              let html = wrap(fixture.renderHtml(after));
              fixture.updateRichText(host, html, { markdownSource: after, sourceHtmlTransform: wrap });
              ensureCodeCopyButtons(host, labels);
              finish(success);
              await pending;
              if (host.querySelector('.pinvou-code-copy-btn') !== button
                || button.title !== (success ? labels.copied : labels.failed)) {
                throw new Error('A new wrapper lost its retained code control');
              }
              html = wrap(fixture.renderHtml(`${after} continues`));
              fixture.updateRichText(host, html, { markdownSource: `${after} continues`, sourceHtmlTransform: wrap });
              ensureCodeCopyButtons(host, labels);
              if (host.querySelector('.pinvou-code-copy-btn') !== button
                || button.title !== (success ? labels.copied : labels.failed)) {
                throw new Error('The next frame lost a code control after wrapper adoption');
              }
              const content = host.cloneNode(true);
              for (const control of content.querySelectorAll('.pinvou-code-copy-btn, .pinvou-code-copy-live')) control.remove();
              const expected = document.createElement('div');
              expected.innerHTML = html;
              if (content.innerHTML !== expected.innerHTML) throw new Error('Code adoption changed rendered DOM');
            } finally { getSelection().removeAllRanges(); host.remove(); }
          }
        }
      }
      for (const animation of [false, true]) {
        setStreamFadePreference(animation);
        for (const tag of ['td', 'th', 'caption']) {
          for (const code of [false, true]) {
            for (const backward of [false, true]) {
              for (const boundary of [false, true]) {
                const host = document.createElement('div');
                document.body.append(host);
                try {
                  const payload = code ? '<pre class=pinvou-code-block><code>same</code></pre>' : 'same';
                  const body = tag === 'caption' ? `<caption>${payload}</caption><tr><td>cell</td></tr>`
                    : `<tr><${tag}>${payload}</${tag}></tr>`;
                  const block = `<div><table>${body}</table></div>`;
                  const before = `![${block}][x]\n\n${block}`;
                  const commit = source => {
                    const html = fixture.renderHtml(source);
                    fixture.updateRichText(host, html, { markdownSource: source });
                    ensureCodeCopyButtons(host, labels);
                    applyStreamFade(host, { streaming: true, html });
                  };
                  commit(before);
                  const part = host.querySelectorAll(tag)[1];
                  const pre = part.querySelector('pre');
                  let first;
                  let last;
                  if (boundary) { first = [part, 0]; last = [part, part.childNodes.length]; }
                  else {
                    const walker = document.createTreeWalker(part, NodeFilter.SHOW_TEXT);
                    let text = walker.nextNode();
                    while (text && !text.data.includes('same')) text = walker.nextNode();
                    const start = text.data.indexOf('same');
                    first = [text, start]; last = [text, start + 4];
                  }
                  getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
                  commit(`${before}\n\n[x]: /x\n\n${block}`);
                  if (host.querySelectorAll(tag)[0] !== part || getSelection().toString() !== 'same'
                    || !part.contains(getSelection().anchorNode) || !part.contains(getSelection().focusNode)
                    || (pre && !boundary && (!pre.contains(getSelection().anchorNode) || !pre.contains(getSelection().focusNode)))) {
                    throw new Error('Retained table selection moved to appended same-source-looking content');
                  }
                } finally { getSelection().removeAllRanges(); host.remove(); }
              }
            }
          }
        }
      }
      for (const animation of [false, true]) {
        setStreamFadePreference(animation);
        for (const partName of ['table', 'tbody', 'tr', 'td']) {
          for (const backward of [false, true]) {
            const host = document.createElement('div');
            document.body.append(host);
            try {
              const before = '<table><tr><td>word</td></tr>';
              const commit = source => {
                const html = fixture.renderHtml(source);
                fixture.updateRichText(host, html, { markdownSource: source });
                applyStreamFade(host, { streaming: true, html });
              };
              commit(before);
              const part = host.querySelector(partName);
              const first = [part, 0];
              const last = [part, part.childNodes.length];
              getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
              commit(`${before}word outside</table>`);
              if (getSelection().toString() !== 'word' || !part.contains(getSelection().anchorNode)
                || !part.contains(getSelection().focusNode)) throw new Error('Table ancestor endpoints selected new fostered prose');
            } finally { getSelection().removeAllRanges(); host.remove(); }
          }
        }
        for (const backward of [false, true, null]) {
          const host = document.createElement('div');
          document.body.append(host);
          try {
            const before = '- item\n  sentinel\n\n  ';
            const commit = source => {
              const html = fixture.renderHtml(source);
              fixture.updateRichText(host, html, { markdownSource: source });
              applyStreamFade(host, { streaming: true, html });
            };
            commit(before);
            const li = host.querySelector('li');
            const walker = document.createTreeWalker(li, NodeFilter.SHOW_TEXT);
            let text = walker.nextNode();
            while (text && !text.data.includes('sentinel')) text = walker.nextNode();
            const first = [li, 2];
            const last = backward === null ? first : [text, text.data.indexOf('sentinel') + 8];
            getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
            commit(`${before}c`);
            if (backward === null ? !getSelection().isCollapsed : getSelection().toString() !== 'sentinel') {
              throw new Error('Loose-list element boundary changed its selected break or collapsed caret');
            }
          } finally { getSelection().removeAllRanges(); host.remove(); }
        }
      }

      {
        const host = document.createElement('div');
        document.body.append(host);
        try {
          const commit = (source, transform, filtered = false) => {
            let html = fixture.renderHtml(source);
            if (transform) html = transform(html);
            fixture.updateRichText(host, html, { markdownSource: source, sourceHtmlTransform: transform, sourceHtmlFiltered: filtered });
            ensureCodeCopyButtons(host, labels);
          };
          for (const [before, suffix] of [
            ['<table><tr><td>one</td></tr></table><table><tr><td>two</td></tr>', 'new prose</table>'],
            ['<pre>one</pre><table><tr><td>two</td></tr>', 'new prose</table>'],
            ['<table><tr><td>one</td></tr></table><table><tr><td>two</td></tr>', '<b>new prose</b></table>'],
          ]) {
            for (const element of [false, true]) {
              for (const backward of [false, true]) {
                getSelection().removeAllRanges();
                commit('');
                commit(before);
                const [first, last] = host.querySelectorAll('td,pre');
                const firstText = first.querySelector('code')?.firstChild || first.firstChild;
                const lastText = last.firstChild;
                const from = element ? [first, 1] : [firstText, firstText.length];
                const to = element ? [last, 0] : [lastText, 0];
                getSelection().setBaseAndExtent(...(backward ? to : from), ...(backward ? from : to));
                commit(before + suffix);
                const selection = getSelection();
                const actual = selection.toString();
                if (!first.isConnected || !last.isConnected || selection.isCollapsed
                  || !first.contains(backward ? selection.focusNode : selection.anchorNode)
                  || !last.contains(backward ? selection.anchorNode : selection.focusNode)) {
                  throw new Error('Shared text coordinates lost independent native endpoint ownership');
                }
                selection.setBaseAndExtent(firstText, firstText.length, lastText, 0);
                if (actual !== selection.toString()) throw new Error('Native break endpoints lost intervening fostered prose');
              }
            }
          }
          getSelection().removeAllRanges();
          const code = '```js\nsame\n```';
          for (const success of [false, true]) {
            for (const kind of ['opening', 'position', 'policy', 'append', 'unchanged']) {
              commit('');
              const source = kind === 'policy' ? code + '\n\n' + code : code;
              const take = keep => html => html.replace(/<pre\b[\s\S]*?<\/pre>/g, (pre, offset) => {
                const first = offset === 0;
                return first === keep ? pre : '';
              });
              const oldPolicy = kind === 'policy' ? take(true) : undefined;
              const newPolicy = kind === 'policy' ? take(false) : undefined;
              commit(source, oldPolicy, kind === 'policy');
              const old = host.querySelector('.pinvou-code-copy-btn');
              let finish;
              const pending = copyCodeBlockFromButton(old, { labels,
                writeText: () => new Promise(resolve => { finish = resolve; }) });
              commit(source, oldPolicy, kind === 'policy');
              if (host.querySelector('.pinvou-code-copy-btn') !== old) throw new Error('An unchanged render tuple discarded pending copy');
              const after = kind === 'opening' ? '~~~js\nsame\n~~~'
                : kind === 'position' ? '[unused]: /x\n\n' + source
                  : kind === 'append' ? source + '\n\n[unused]: /x' : source;
              commit(after, newPolicy, kind === 'policy');
              const current = host.querySelector('.pinvou-code-copy-btn');
              const retained = kind === 'append' || kind === 'unchanged';
              if ((current === old) !== retained || old.isConnected !== retained) {
                throw new Error('Equal HTML ignored changed source or presentation ownership: ' + kind);
              }
              finish(success);
              await pending;
              if (current.title !== (retained ? success ? labels.copied : labels.failed : labels.copyCode)) {
                throw new Error('Equal HTML borrowed asynchronous copy feedback: ' + kind);
              }
            }
          }
        } finally { getSelection().removeAllRanges(); host.remove(); }
      }
      {
        // Count detached provenance parses, not elapsed wall time, so the
        // performance contract is deterministic on slower CI/native hosts.
        const descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'innerHTML');
        let probes = 0;
        const host = document.createElement('div');
        document.body.append(host);
        Object.defineProperty(Element.prototype, 'innerHTML', { ...descriptor, set(value) {
          if (typeof value === 'string' && value.includes('data-pinvou-source-block=')) probes += 1;
          descriptor.set.call(this, value);
        } });
        try {
          const code = '```js\n' + 'const alpha = 1;\n'.repeat(700) + '```\n\n';
          let source = code;
          const commit = (value, transform) => {
            let html = fixture.renderHtml(value);
            if (transform) html = transform(html);
            probes = 0;
            fixture.updateRichText(host, html, { markdownSource: value, sourceHtmlTransform: transform });
            ensureCodeCopyButtons(host, labels);
            return probes;
          };
          commit(source);
          source += 'warm ';
          commit(source);
          const button = host.querySelector('.pinvou-code-copy-btn');
          let finish;
          const pending = copyCodeBlockFromButton(button, { labels,
            writeText: () => new Promise(resolve => { finish = resolve; }) });
          for (let index = 0; index < 6; index += 1) {
            source += 'more ';
            if (commit(source) !== 0 || host.querySelector('.pinvou-code-copy-btn') !== button) {
              throw new Error('Unchanged closed code rebuilt detached provenance on a steady frame');
            }
          }
          finish(true);
          await pending;
          source = source.replaceAll('alpha = 1', 'alpha = 2');
          if (commit(source) === 0 || host.querySelector('.pinvou-code-copy-btn') !== button) {
            throw new Error('A source correction failed to revalidate or retain its unchanged opening');
          }
          source += '\n\n```text\nnew block\n```';
          if (commit(source) === 0 || host.querySelector('.pinvou-code-copy-btn') !== button) {
            throw new Error('A new code source bypassed provenance validation');
          }
          source += '\n\n<div>Raw HTML disables the simple-code cache</div>';
          if (commit(source) === 0) throw new Error('Raw HTML bypassed provenance validation');
          const wrap = html => `<article>${html}</article>`;
          if (commit(source, wrap) === 0 || host.querySelector('.pinvou-code-copy-btn') !== button
            || button.title !== labels.copied) throw new Error('A presentation change reused stale provenance or lost feedback');
        } finally {
          Object.defineProperty(Element.prototype, 'innerHTML', descriptor);
          host.remove();
        }
      }
    } finally { host.remove(); }
  }

  const templateHost = document.createElement('div');
  fixture.updateRichText(templateHost, '<template>first</template>');
  const templateNode = templateHost.querySelector('template');
  fixture.updateRichText(templateHost, '<template><b>second</b><template>nested</template></template>');
  check(templateHost.innerHTML === '<template><b>second</b><template>nested</template></template>'
    && templateHost.querySelector('template') === templateNode, 'Inert template content was stale');
  fixture.updateRichText(templateHost, '<div><p>old</p></div>');
  fixture.updateRichText(templateHost, '<div><template><b>new</b></template></div>');
  check(templateHost.innerHTML === '<div><template><b>new</b></template></div>'
    && templateHost.querySelector('template').childNodes.length === 0, 'Template promotion lost inert content');
  const folded = document.createElement('div');
  folded.id = 'native-presentation-fold';
  document.body.append(folded);
  const fold = label => html => html.replace(/<pre[\s\S]*$/, `<div>${label}</div>`);
  for (const backward of [false, true]) {
    const before = '![**gone**] retained gone\n\n```persona-card\n{"name":';
    const after = before + '"A"}\n```\n\n[**gone**]: /x';
    getSelection().removeAllRanges();
    fixture.updateRichText(folded, fold('old placeholder')(fixture.renderHtml(before)), {
      markdownSource: before, sourceHtmlTransform: fold('old placeholder'),
    });
    fixture.select('#native-presentation-fold', 'gone', backward);
    fixture.updateRichText(folded, fold('new placeholder')(fixture.renderHtml(after)), {
      markdownSource: after, sourceHtmlTransform: fold('new placeholder'),
    });
    check(selected() === '' && getSelection().isCollapsed, 'Previous presentation fold lost retired source ownership');
  }
  const recognize = html => html.includes('name') ? fold('Designing')(html) : html;
  for (const opening of ['```json\n', '<pre><code>']) {
    for (const backward of [false, true]) {
      getSelection().removeAllRanges();
      const before = opening + '{"description":"Designing",';
      const after = before + '"name":"A"}';
      fixture.updateRichText(folded, recognize(fixture.renderHtml(before)), {
        markdownSource: before, sourceHtmlTransform: recognize,
      });
      fixture.select('#native-presentation-fold', 'Designing', backward);
      fixture.updateRichText(folded, recognize(fixture.renderHtml(after)), {
        markdownSource: after, sourceHtmlTransform: recognize, sourceHtmlFiltered: true,
      });
      check(selected() === '' && getSelection().isCollapsed, 'Newly recognized code selected placeholder text');
    }
  }
  {
    getSelection().removeAllRanges();
    const extract = html => html.includes('name') ? html.replace(/<pre[\s\S]*?<\/pre>\n?/, '') : html;
    const before = '```json\n{"description":"old",';
    const after = before + '"name":"A","body":"B"}\n```\n\n```text\nnew code\n```';
    fixture.updateRichText(folded, extract(fixture.renderHtml(before)), {
      markdownSource: before, sourceHtmlTransform: extract,
    });
    const oldPre = folded.querySelector('pre');
    fixture.updateRichText(folded, extract(fixture.renderHtml(after)), {
      markdownSource: after, sourceHtmlTransform: extract, sourceHtmlFiltered: true,
    });
    check(oldPre !== folded.querySelector('pre') && !oldPre.isConnected
      && folded.querySelector('code').textContent === 'new code', 'Filtered code identity moved to a different source block');
  }
  folded.remove();
  await render('Prefix **word word word');
  fixture.select('#root .conversation-markdown', 'word', true, 2);
  await render('Prefix **word word word** trailing');
  let current = getSelection().getRangeAt(0);
  let preceding = document.createRange();
  preceding.selectNodeContents(document.querySelector('#root .conversation-markdown'));
  preceding.setEnd(current.startContainer, current.startOffset);
  check(selected() === 'word' && preceding.toString() === 'Prefix word word ', 'Third repeated word changed occurrence');
  await render('- word');
  fixture.select('#root .conversation-markdown', 'word');
  await render('- word\n- word\n\n  More output');
  check(getSelection().getRangeAt(0).startContainer.parentElement.closest('li') === document.querySelector('#root li'), 'Loose list selected a new item');
  await render('Title');
  fixture.select('#root .conversation-markdown', 'Title');
  await render('Title\n=====\n\nTitle');
  check(getSelection().getRangeAt(0).startContainer.parentElement.closest('h1') === document.querySelector('#root h1'), 'Heading conversion selected a new paragraph');
  for (const [before, after, word, prefixText] of [
    ['word **word ', 'word **word word** word', 'word', 'word '],
    ['[cherry cherry', '[cherry cherry](https://example.com) cherry', 'cherry', 'cherry '],
    ['**cherry *cherry*', '**cherry *cherry**', 'cherry', '*cherry '],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word, true, 1);
    await render(after);
    const range = getSelection().getRangeAt(0);
    const prefix = document.createRange();
    prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
    prefix.setEnd(range.startContainer, range.startOffset);
    check(selected() === word && prefix.toString() === prefixText, 'Reused node changed the logical selected occurrence');
  }
  for (const id of ['ref', 'word']) {
    await render(`[A][${id}] word word`);
    fixture.select('#root .conversation-markdown', 'word', true, id === 'word' ? 1 : 0);
    await render(`[A][${id}] word word word\n\n[${id}]: https://example.com`);
    const range = getSelection().getRangeAt(0);
    const prefix = document.createRange();
    prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
    prefix.setEnd(range.startContainer, range.startOffset);
    check(selected() === 'word' && prefix.toString() === 'A ', 'Reference resolution moved selection onto a repeated word');
  }
  await render('![word][ref] word word');
  fixture.select('#root .conversation-markdown', 'word', true, 1);
  await render('![word][ref] word word word\n\n[ref]: https://example.com/image.png');
  const imageRange = getSelection().getRangeAt(0);
  const imagePrefix = document.createRange();
  imagePrefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
  imagePrefix.setEnd(imageRange.startContainer, imageRange.startOffset);
  check(selected() === 'word' && imagePrefix.toString() === ' ', 'Image alt text retargeted selected prose');
  for (const [before, after, word, occurrence, expectedPrefix] of [
    ['[A][ref] [x][word] word word', '[A][ref] [x][word] word word word\n\n[ref]: https://example.com', 'word', 1, 'A [x][word] '],
    ['\\![word][ref] ![alt][ref] word word', '\\![word][ref] ![alt][ref] word word word\n\n[ref]: https://example.com/image.png', 'word', 1, '!word  '],
    ['![word ![alt](https://example.com/image.png)][ref] word word', '![word ![alt](https://example.com/image.png)][ref] word word word\n\n[ref]: https://example.com/image.png', 'word', 1, ' '],
    ['**[] []', '**[] []** []', '[]', 1, '[] '],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word, true, occurrence);
    await render(after);
    const range = getSelection().getRangeAt(0);
    const prefix = document.createRange();
    prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
    prefix.setEnd(range.startContainer, range.startOffset);
    check(selected() === word && prefix.toString() === expectedPrefix, 'Retained syntax or punctuation changed selected occurrence');
  }
  await render('[![alt][image]][word] word word');
  fixture.select('#root .conversation-markdown', 'word', true, 1);
  await render('[![alt][image]][word] word word word\n\n[image]: https://example.com/image.png\n[word]: https://example.com');
  const nestedRange = getSelection().getRangeAt(0);
  const nestedPrefix = document.createRange();
  nestedPrefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
  nestedPrefix.setEnd(nestedRange.startContainer, nestedRange.startOffset);
  check(selected() === 'word' && nestedPrefix.toString() === ' ', 'Nested roles shifted selected prose');
  const largeList = Array.from({ length: 40 }, (_, index) => `- Task ${String(index + 1).padStart(2, '0')}: pending`).join('\n');
  await render(largeList);
  fixture.select('#root .conversation-markdown', 'pending', true, 30);
  await render(`${largeList}\n\n  More output`);
  const listRange = getSelection().getRangeAt(0);
  const listPrefix = document.createRange();
  listPrefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
  listPrefix.setEnd(listRange.startContainer, listRange.startOffset);
  check(selected() === 'pending' && listPrefix.toString().endsWith('Task 31: '), 'Large list conversion changed selected item');
  for (const [before, after, word, occurrence] of [
    ['word ![word word]', 'word ![word word](https://example.com/image.png) word word', 'word', 1],
    ['Prefix &copy', 'Prefix &copy; copy tail', 'copy', 0],
    ['Prefix\n\n[word]:', 'Prefix\n\n[word]: https://example.com\n\nword', 'word', 0],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word, true, occurrence);
    await render(after);
    check(selected() === '' && getSelection().isCollapsed, `Removed syntax selected unrelated new output: ${JSON.stringify({ before, after, selected: selected() })}`);
  }
  const symbolList = Array.from({ length: 40 }, (_, index) => `- Task ${String(index + 1).padStart(2, '0')}: \`*\` pending`).join('\n');
  await render(symbolList);
  fixture.select('#root .conversation-markdown', '*', true, 30);
  await render(`${symbolList}\n\n  More output`);
  check(selected() === '*' && getSelection().getRangeAt(0).startContainer.parentElement.closest('li').textContent.includes('Task 31:'), 'Large mapping lost a code symbol');
  const literalRows = Array.from({ length: 100 }, (_, index) => `- Task ${index}: payload payload`);
  literalRows[50] = '- Task 50: *';
  const literalList = literalRows.join('\n');
  await render(literalList);
  fixture.select('#root li:nth-child(51)', '*', true);
  await render(`${literalList}\n\n  More output`);
  check(selected() === '*', 'Loose-list conversion lost isolated literal punctuation');
  await render('[x]: https://example.com\n  "tit');
  fixture.select('#root .conversation-markdown', 'tit');
  await render('[x]: https://example.com\n  "title"\n\nNew paragraph');
  check(selected() === '', 'Retired multiline title selected new prose');
  for (const [before, after, word] of [
    ['Prefix **', 'Prefix **word** then **', '**'],
    ['1', '1.\n\n1 tail', '1'],
    ['- [x', '- [x] \n\nx tail', 'x'],
    ['Prefix <span', 'Prefix <span>more</span> span tail', 'span'],
    ['> <span\n> title="word', '> <span\n> title="word">label</span> word tail', 'word'],
    ['Prefix <!--word', 'Prefix <!--word--> word tail', 'word'],
    ['Prefix &', 'Prefix &copy; word & tail', '&'],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word, true);
    await render(after);
    check(selected() === '' && getSelection().isCollapsed, `Retired source syntax selected new output: ${before}`);
  }
  for (const [before, after, word] of [
    ['Prefix &copy', 'Prefix &copycat and ©', 'copy'],
    ['Prefix ![word](', 'Prefix ![word](bad\n\n![word](https://example.com/i.png)', 'word'],
  ]) {
    await render(before);
    fixture.select('#root .conversation-markdown', word);
    await render(after);
    check(selected() === word, 'Growing literal syntax lost its selection');
  }
  await render('<svg>![gone]</svg> !\\[gone]');
  fixture.select('#root .conversation-markdown', 'gone', true);
  await render('<svg>![gone]</svg> !\\[gone]\n\n[gone]: https://example.com/i.png');
  check(selected() === 'gone', 'Hidden SVG source retired a retained equal literal');
  const hiddenPrefix = '<svg><text>Title</text></svg>\n\n![**gone**]\n\nMore paragraph';
  await render(hiddenPrefix);
  fixture.select('#root .conversation-markdown', 'gone', true);
  await render(`${hiddenPrefix}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
  check(selected() === '' && getSelection().isCollapsed, 'Sanitized prefix disabled source-local ownership');
  const headingPrefix = '# Title #\n\n![**gone**] more';
  await render(headingPrefix);
  fixture.select('#root .conversation-markdown', 'gone', true);
  await render(`${headingPrefix}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
  check(selected() === '' && getSelection().isCollapsed, 'Heading syntax disabled image source ownership');
  await render('![gone]');
  fixture.select('#root .conversation-markdown', 'gone');
  await render('![gone][missing]\n\n![gone](https://example.com/i.png)');
  check(selected() === 'gone', 'Growing closed shortcut discarded its retained label');
  await render('Prefix <script-x');
  fixture.select('#root .conversation-markdown', 'script-x');
  await render('Prefix <script-x> appended');
  check(selected() === 'script-x', 'Neutralized tag extension lost its selection');
  await render('![gone](<em');
  fixture.select('#root .conversation-markdown', 'gone');
  await render('![gone](<em>bold</em>\n\n![gone](https://example.com/i.png)');
  check(selected() === 'gone', 'Destination parsing discarded a retained image label');
  await render('![gone &co');
  fixture.select('#root .conversation-markdown', 'gone');
  await render('![gone &copy;\n\n![gone &co](https://example.com/i.png)');
  check(selected() === 'gone', 'Entity completion discarded a retained literal image label');
  await render('![gone **bold');
  fixture.select('#root .conversation-markdown', 'gone');
  await render('![gone **bold**\n\n![gone bold](https://example.com/i.png)');
  check(selected() === 'gone', 'Inner formatting completion discarded a retained literal label');
  await render('![gone **bold**');
  fixture.select('#root .conversation-markdown', 'gone');
  await render('![gone **bold**](https://example.com/i.png)\n\n![gone bold');
  check(selected() === '' && getSelection().isCollapsed, 'Formatted image label selected a new duplicate');
  await render('```js\nword\n`');
  fixture.select('#root .conversation-markdown', 'word\n`');
  await render('```js\nword\n```\n\nnew');
  check(selected() === 'word', 'Completed closing fence selected new prose');
  const history = '[q][literal] '.repeat(600);
  const nestedImage = `${history}![word **x** ![alt][image]][outer]`;
  await render(nestedImage);
  fixture.select('#root .conversation-markdown', 'word');
  await render(`${nestedImage}\n\n[image]: https://example.com/i.png\n[outer]: https://example.com/o.png\n\nword tail`);
  check(selected() === '' && getSelection().isCollapsed, 'Bounded nested-image match selected new output');
  await render('Heading');
  fixture.select('#root .conversation-markdown', 'Heading');
  await render('Heading\n=======\n\nMore');
  check(selected() === 'Heading', 'Markdown structure change lost selection');
  const source = '```javascript\nconst alpha = 1;\nlet growing';
  await render(source);
  fixture.select('#root code', 'const alpha = 1;');
  const button = document.querySelector('#root .pinvou-code-copy-btn');
  await render(source + ' = 2;');
  check(selected() === 'const alpha = 1;', 'Growing code lost selection');
  await render(source + ' = 2;\n```', false);
  check(selected() === 'const alpha = 1;', 'Completion/highlighting lost selection');
  check(button === document.querySelector('#root .pinvou-code-copy-btn'), 'Copy button was replaced');
  getSelection().removeAllRanges();
  fixture.mode('pending');
  button.click();
  await render(source + ' = 3;');
  fixture.finishWrite();
  await wait(50);
  check(button.title === 'Copied', 'Async copy feedback was lost');
  check(fixture.writes[0].includes('growing = 2;'), 'Copy did not snapshot the clicked code');
  const listCode = '- Step\n  ```js\n  const x = 1;';
  await render(listCode);
  fixture.mode('pending');
  const listButton = document.querySelector('#root .pinvou-code-copy-btn');
  listButton.click();
  await render(listCode + '\n  ```\n\n  More output');
  fixture.finishWrite();
  await wait(50);
  check(listButton === document.querySelector('#root .pinvou-code-copy-btn') && listButton.title === 'Copied',
    'Loose-list conversion replaced the pending copy button');
  fixture.legacy('<p>Legacy selected <strong>text</strong></p>');
  fixture.select('#legacy', 'selected text');
  fixture.legacy('<p>Legacy selected <strong>text</strong> grows</p>');
  check(selected() === 'selected text', 'Legacy selection changed');
  fixture.select('#legacy', 'grows');
  fixture.legacy('<p>Legacy selected <strong>text</strong> grows</p>', false);
  await wait(500);
  check(selected() === 'grows', 'Delayed animation cleanup lost selection inside a fade span');
  window.webkit.messageHandlers.result.postMessage(JSON.stringify({ ok: true }));
})().catch(error => window.webkit.messageHandlers.result.postMessage(JSON.stringify({ ok: false, error: String(error) })));
"""


manager = WebKit2.UserContentManager()
manager.register_script_message_handler("result")
window = Gtk.OffscreenWindow()
view = WebKit2.WebView(user_content_manager=manager)
window.add(view)
window.set_default_size(900, 800)
result = {"ok": False, "error": "Native WebKitGTK timed out"}


def receive(_manager, message):
    global result
    result = json.loads(message.get_js_value().to_string())
    Gtk.main_quit()


def loaded(webview, event):
    if event == WebKit2.LoadEvent.FINISHED:
        webview.evaluate_javascript(CHECKS, -1, None, None, None, None, None)


manager.connect("script-message-received::result", receive)
view.connect("load-changed", loaded)
view.load_uri(sys.argv[1])
window.show_all()
# The extended streaming matrix takes about 58s even without parallel jobs.
GLib.timeout_add_seconds(75, lambda: (Gtk.main_quit(), False)[1])
Gtk.main()
window.destroy()
version = f"{WebKit2.get_major_version()}.{WebKit2.get_minor_version()}.{WebKit2.get_micro_version()}"
print(json.dumps({"engine": f"WebKitGTK {version}", **result}))
sys.exit(0 if result["ok"] else 1)
