import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import react from '@vitejs/plugin-react';
import * as puppeteer from 'puppeteer-core';
import { createServer } from 'vite';

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const chrome = process.env.CHROME || [
  '/usr/bin/google-chrome', '/usr/bin/chromium', '/snap/bin/chromium', '/usr/bin/chromium-browser',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
].find(candidate => fs.existsSync(candidate));
if (!chrome) {
  console.error('SKIP: chromium/chrome not found; set CHROME to its executable');
  process.exit(2);
}

let server;
let browser;
try {
  server = await createServer({
    root: appRoot, configFile: false, appType: 'mpa', logLevel: 'error', plugins: [
      { name: 'test-conversation-commits', enforce: 'pre', transform(code, id) {
        if (id.endsWith('/src/features/chat/ChatView.jsx')) return `${code}\nexport { ChatBubble };\n`;
        if (id.endsWith('/src/features/conversation/rich-text-dom.js')) {
          assert.ok(code.includes('export function commitRichTextFrame('), 'commit instrumentation must remain connected');
          return `${code.replace('export function commitRichTextFrame(', 'function commitRichTextFrameImpl(')}
export function commitRichTextFrame(container, html, options) {
  const cleanup = commitRichTextFrameImpl(container, html, options);
  (globalThis.__pinvouTestCommits ||= new WeakMap()).set(container, options.markdownSource);
  return cleanup;
}`;
        }
        if (id.endsWith('/src/shared/markdown-renderer.js')) {
          const signature = 'export function renderMarkdownSourceProbe(tokens, decorate) {';
          assert.ok(code.includes(signature), 'diagnostic instrumentation must remain connected');
          return code.replace(signature, () => `${signature} globalThis.__pinvouSourceProbeCalls = (globalThis.__pinvouSourceProbeCalls || 0) + 1;`);
        }
      } }, react(),
    ],
    server: { host: '127.0.0.1', port: 0, watch: null },
  });
  await server.listen();
  browser = await puppeteer.launch({ executablePath: chrome, headless: true, args: ['--no-sandbox', '--disable-gpu'] });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/tests/fixtures/streaming_rich_text.html`, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => Boolean(window.streamingFixture));

  const render = async (text, streaming = true) => {
    await page.evaluate((value, active) => window.streamingFixture.render(value, active), text, streaming);
    const expectedText = await page.evaluate(async value => {
      const renderMarkdown = window.streamingFixture.renderHtml;
      const parsed = document.createElement('div');
      parsed.innerHTML = renderMarkdown(value);
      return parsed.textContent;
    }, text);
    await page.waitForFunction(value => {
      const content = document.querySelector('#root .conversation-markdown').cloneNode(true);
      for (const control of content.querySelectorAll('.pinvou-code-copy-btn, .pinvou-code-copy-live')) control.remove();
      return content.textContent === value;
    }, {}, expectedText);
    // Rendering is deliberately throttled at 200ms; await the actual commit,
    // including the completion pass's deferred animation cleanup.
    await new Promise(resolve => { setTimeout(resolve, 250); });
  };
  const selected = () => page.evaluate(() => window.getSelection().toString());
  const select = (selector, text, backward = false, occurrence = 0) => page.evaluate(
    (scope, value, reverse, index) => window.streamingFixture.select(scope, value, reverse, index), selector, text, backward, occurrence,
  );


  // Count actual detached parser/sanitizer calls rather than timing-sensitive
  // wall-clock limits. Each append still validates the current presentation.
  await page.evaluate(async () => {
    // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL.
    const { updateRichText } = await import('/src/features/conversation/rich-text-dom.js');
    // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL.
    const { ensureCodeCopyButtons, copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
    const check = (value, message) => { if (!value) throw new Error(message); };
    const labels = { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' };
    const renderHtml = window.streamingFixture.renderHtml;
    const payload = 'abcdefghijklmnopqrstuvwxyz0123456789 '.repeat(2703).slice(0, 100000);
    for (const wrapper of ['', '- Step\n  ', '> ']) {
      const host = document.createElement('div');
      document.body.append(host);
      try {
        let source = `${wrapper}\`\`\`text\n${wrapper ? wrapper === '> ' ? '> ' : '  ' : ''}${wrapper ? 'code' : payload}`;
        const options = wrapper ? { sourceHtmlTransform: html => html } : {};
        const commit = value => {
          updateRichText(host, renderHtml(value), { ...options, markdownSource: value });
          ensureCodeCopyButtons(host, labels);
        };
        commit(source);
        const pre = host.querySelector('pre');
        const button = host.querySelector('button');
        source += ' warm';
        commit(source);
        for (const success of [true, false]) {
          let finish;
          let copied;
          const pending = copyCodeBlockFromButton(button, { labels, writeText: value => {
            copied = value;
            return new Promise(resolve => { finish = resolve; });
          } });
          const snapshot = host.querySelector('code').textContent;
          for (let frame = 0; frame < 3; frame += 1) {
            source += ' more';
            const calls = globalThis.__pinvouSourceProbeCalls || 0;
            commit(source);
            check(globalThis.__pinvouSourceProbeCalls - calls === 1, 'Open code append must probe only the current frame');
            check(host.querySelector('pre') === pre && host.querySelector('button') === button, 'Open code must keep its own controls');
          }
          finish(success);
          await pending;
          check(copied === snapshot && button.title === (success ? labels.copied : labels.failed), 'Pending copy must retain its snapshot and feedback');
        }
        const code = host.querySelector('code');
        const text = code.firstChild;
        getSelection().setBaseAndExtent(text, 0, text, 4);
        source += `\n${wrapper === '> ' ? '> ' : wrapper ? '  ' : ''}\`\`\`\n\nMore prose`;
        commit(source);
        check(getSelection().toString() === snapshotPrefix(code.textContent), 'Closing a nested/open code must preserve selection');
        check(host.querySelector('pre') === pre && host.querySelector('button') === button, 'Closure must keep the source-owned block');
        const clean = host.cloneNode(true);
        for (const control of clean.querySelectorAll('.pinvou-code-copy-btn, .pinvou-code-copy-live')) control.remove();
        check(clean.innerHTML === renderHtml(source), 'Cached code closure must equal a fresh render');
      } finally { getSelection().removeAllRanges(); host.remove(); }
    }
    function snapshotPrefix(text) { return text.slice(0, 4); }

    for (const table of [false, true]) {
      const host = document.createElement('div');
      document.body.append(host);
      try {
        const code = '<pre class="pinvou-code-block"><code>same</code></pre>';
        const block = table ? `<table><tr><td>${code}</td></tr></table>` : code;
        const before = `![${block}][x]\n\n${block}`;
        const commit = source => {
          updateRichText(host, renderHtml(source), { markdownSource: source });
          ensureCodeCopyButtons(host, labels);
        };
        commit(before); commit(`${before}\n`);
        const buttons = [...host.querySelectorAll('button')];
        check(buttons.length === 2, 'Retirement probe needs equal old code blocks');
        commit(`${before}\n\n[x]: https://example.com/i.png\n\n${block}`);
        const after = [...host.querySelectorAll('button')];
        check(!buttons[0].isConnected && after[0] === buttons[1] && !buttons.includes(after[1]),
          'Warm provenance cache must retire image-label code without lending feedback to appended equal code');
      } finally { host.remove(); }
    }

    {
      const host = document.createElement('div');
      document.body.append(host);
      try {
        let source = '<pre class="pinvou-code-block"><code>same</code></pre>\n\n<pre class="pinvou-code-block"><code>same</code></pre>';
        const commit = (value, transform) => {
          const html = renderHtml(value);
          updateRichText(host, transform ? transform(html) : html, { markdownSource: value,
            sourceHtmlTransform: transform, sourceHtmlFiltered: Boolean(transform) });
          ensureCodeCopyButtons(host, labels);
        };
        commit(source); source += '\n'; commit(source);
        const buttons = [...host.querySelectorAll('button')];
        source += '\n';
        commit(source, html => html.replace(/<pre[\s\S]*?<\/pre>/, ''));
        check(!buttons[0].isConnected && host.querySelector('button') === buttons[1], 'New presentation policy must retain the surviving source block');
      } finally { host.remove(); }
    }

    {
      const host = document.createElement('div');
      document.body.append(host);
      try {
        let source = '```text\nretained';
        const commit = value => updateRichText(host, renderHtml(value), { markdownSource: value });
        commit(source); source += ' warm'; commit(source);
        const text = host.querySelector('code').firstChild;
        const original = text.replaceData;
        let failed = false;
        text.replaceData = () => { throw new Error('Injected commit failure'); };
        source += ' retry';
        try { commit(source); } catch (error) { failed = error.message === 'Injected commit failure'; }
        finally { text.replaceData = original; }
        check(failed, 'Failure control must interrupt the real DOM commit');
        const calls = globalThis.__pinvouSourceProbeCalls;
        commit(source);
        check(globalThis.__pinvouSourceProbeCalls - calls === 1 && host.querySelector('code').textContent.includes('retry'),
          'A failed commit must not publish its incoming diagnostic snapshot');
      } finally { host.remove(); }
    }

    const host = document.createElement('div');
    document.body.append(host);
    try {
      let source = '```text\nsame';
      const commit = (value, options = {}) => {
        updateRichText(host, renderHtml(value), { markdownSource: value, ...options });
        ensureCodeCopyButtons(host, labels);
      };
      commit(source); source += ' warm'; commit(source);
      const original = host.querySelector('button');
      source += ' more';
      // Actual DOM drift invalidates the previous snapshot instead of lending
      // its source identity to an unrelated visible shape.
      host.querySelector('code').firstChild.appendData(' drift');
      let calls = globalThis.__pinvouSourceProbeCalls;
      commit(source);
      check(globalThis.__pinvouSourceProbeCalls - calls === 2, 'DOM drift must revalidate both frames');
      check(host.querySelector('button') !== original, 'Invalid diagnostics must discard stale controls');
      source += ' warm'; commit(source); source += ' again'; commit(source);
      const retained = host.querySelector('button');
      const replacement = source.replace('```', '~~~');
      check(renderHtml(replacement) === renderHtml(source), 'Opening-replacement control must render equal HTML');
      commit(replacement);
      check(host.querySelector('button') !== retained, 'Equal HTML cannot reuse a replaced opening');
      source = `${replacement}\n~~~\n\nTail`; commit(source);
      source += ' warm'; commit(source);
      calls = globalThis.__pinvouSourceProbeCalls;
      source += ' more'; commit(source);
      check(globalThis.__pinvouSourceProbeCalls === calls, 'Unchanged closed code must retain the zero-probe path');
    } finally { host.remove(); }
  });

  const verifyTableEndpoints = async legacy => {
    const results = await page.evaluate(async isLegacy => {
      const fixture = isLegacy ? window.legacySelectionFixture : window.streamingFixture;
      // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
      const { setStreamFadePreference } = await import('/src/features/conversation/stream-fade.js');
      const selector = isLegacy ? '#root .msg-md' : '#root .conversation-markdown';
      const external = document.createElement('p');
      external.textContent = 'Other message';
      document.querySelector('#root').after(external);
      const results = [];
      const commit = async text => {
        const host = document.querySelector(selector);
        globalThis.__pinvouTestCommits?.delete(host);
        if (isLegacy) fixture.render(text, fixture.renderHtml(text), text);
        else fixture.render(text);
        const started = performance.now();
        while (globalThis.__pinvouTestCommits?.get(document.querySelector(selector)) !== text) {
          if (performance.now() - started > 10000) throw new Error('Stream frame did not commit');
          await new Promise(resolve => { setTimeout(resolve, 20); });
        }
      };
      try {
        for (const animation of [false, true]) {
          setStreamFadePreference(animation);
          for (const kind of ['cross-message', 'paragraph', 'caption']) {
            for (const variant of ['static', 'growing', 'formatting']) {
              for (const backward of [false, true]) {
                getSelection().removeAllRanges();
                const caption = kind === 'caption';
                const word = caption ? 'same' : 'word';
                const prefix = kind === 'paragraph' ? 'Earlier paragraph\n\n' : '';
                const opening = caption ? '<table><caption>' : '<table><tr><td>';
                const closePart = caption ? '</caption><tr><td>cell</td></tr>' : '</td></tr>';
                const formatting = variant === 'formatting' ? '\n\n**' : '';
                const before = prefix + opening + formatting + word + (variant === 'static' ? closePart : '');
                const continuation = variant === 'static' ? '' : `${formatting ? '**' : ''} retained${closePart}`;
                await commit(before);
                const host = document.querySelector(selector);
                const part = host.querySelector(caption ? 'caption' : 'td');
                if (isLegacy) fixture.select(word, false, `${selector} ${caption ? 'caption' : 'td'}`);
                else fixture.select(`${selector} ${caption ? 'caption' : 'td'}`, word);
                const range = getSelection().getRangeAt(0);
                let first = [range.startContainer, range.startOffset];
                let last = [range.endContainer, range.endOffset];
                if (kind === 'cross-message') last = [external.firstChild, 5];
                if (kind === 'paragraph') first = [host.querySelector('p').firstChild, 0];
                getSelection().setBaseAndExtent(...(backward ? last : first), ...(backward ? first : last));
                const initial = getSelection().toString();
                await commit(`${before}${continuation}${word} outside</table>`);
                const selection = getSelection();
                const endpoint = kind === 'paragraph'
                  ? (backward ? selection.anchorNode : selection.focusNode) : (backward ? selection.focusNode : selection.anchorNode);
                results.push({ kind, variant, initial, selected: selection.toString(), retained: part === host.querySelector(caption ? 'caption' : 'td'),
                  owned: part.contains(endpoint) });
              }
            }
          }
        }
        return results;
      } finally { external.remove(); }
    }, legacy);
    for (const result of results) {
      assert.equal(result.retained, true, 'the original table part must survive');
      assert.equal(result.owned, true, 'each table endpoint must retain its own source, including captions and cross-message ranges');
      if (result.kind === 'caption' || (result.kind === 'cross-message' && result.variant === 'static')) {
        assert.equal(result.selected, result.initial);
      }
    }
  };

  const verifyMarkdownCodeRetirement = async legacy => {
    const results = await page.evaluate(async isLegacy => {
      const fixture = isLegacy ? window.legacySelectionFixture : window.streamingFixture;
      // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
      const { ensureCodeCopyButtons, copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
      // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
      const { setStreamFadePreference, applyStreamFade } = await import('/src/features/conversation/stream-fade.js');
      const selector = isLegacy ? '#root .msg-md' : '#root .conversation-markdown';
      // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
      const { updateRichText } = await import('/src/features/conversation/rich-text-dom.js');
      const labels = { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' };
      const commit = async text => {
        const host = document.querySelector(selector);
        globalThis.__pinvouTestCommits?.delete(host);
        if (isLegacy) fixture.render(text, fixture.renderHtml(text), text);
        else fixture.render(text);
        const started = performance.now();
        while (globalThis.__pinvouTestCommits?.get(document.querySelector(selector)) !== text) {
          if (performance.now() - started > 10000) throw new Error('Stream frame did not commit');
          await new Promise(resolve => { setTimeout(resolve, 20); });
        }
      };
      const results = [];
      for (const animation of [false, true]) {
        setStreamFadePreference(animation);
        for (const wrapper of ['none', 'div', 'table', 'ul', 'blockquote']) {
          for (const same of [false, true]) {
            for (const index of [0, 1]) {
              for (const success of [false, true]) {
                if (wrapper !== 'none' && index === 0) continue;
                getSelection().removeAllRanges();
                const block = value => {
                  const pre = `<pre class=pinvou-code-block><code>${value}</code></pre>`;
                  if (wrapper === 'none') return pre;
                  if (wrapper === 'table') return `<table><tr><td>${pre}</td></tr></table>`;
                  if (wrapper === 'ul') return `<ul><li>${pre}</li></ul>`;
                  return `<${wrapper}>${pre}</${wrapper}>`;
                };
                const before = `![${block(same ? 'same' : 'gone')}][x]\n\n${block(same ? 'same' : 'stay')}`;
                // Also keep the PRE count unchanged and all payloads equal: neither
                // sibling count nor code text establishes source ownership.
                const after = `${before}\n\n[x]: https://example.com/i.png${same ? `\n\n${block('same')}` : ''}`;
                await commit(before);
                await commit(`${before}\n`);
                const buttons = [...document.querySelectorAll(`${selector} .pinvou-code-copy-btn`)];
                let finish;
                let snapshot;
                const pending = copyCodeBlockFromButton(buttons[index], { labels, writeText: value => {
                  snapshot = value;
                  return new Promise(resolve => { finish = resolve; });
                } });
                await commit(after);
                const current = [...document.querySelectorAll(`${selector} .pinvou-code-copy-btn`)];
                finish(success);
                await pending;
                results.push({ same, index, success, snapshot,
                  retired: !buttons[0].isConnected, kept: current[0] === buttons[1],
                  title: current[0].title,
                  fresh: !same || (!buttons.includes(current[1]) && current[1].title === labels.copyCode) });
              }
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
              updateRichText(host, fixture.renderHtml(before), { markdownSource: before });
              ensureCodeCopyButtons(host, labels);
              const button = host.querySelector('.pinvou-code-copy-btn');
              if (isLegacy) fixture.select('stay', true, '#adopt-code-probe code');
              else fixture.select('#adopt-code-probe code', 'stay', true);
              let finish;
              const pending = copyCodeBlockFromButton(button, { labels,
                writeText: () => new Promise(resolve => { finish = resolve; }) });
              const after = `${before}\n\nNew prose`;
              const wrap = html => partial ? html.replace(/(<pre[\s\S]*?<\/pre>)/, '<article>$1</article>') : `<article>${html}</article>`;
              let html = wrap(fixture.renderHtml(after));
              updateRichText(host, html, { markdownSource: after, sourceHtmlTransform: wrap });
              ensureCodeCopyButtons(host, labels);
              finish(success);
              await pending;
              if (getSelection().toString() !== 'stay' || host.querySelector('.pinvou-code-copy-btn') !== button
                || button.title !== (success ? labels.copied : labels.failed)) {
                throw new Error('A new wrapper lost its retained code control');
              }
              html = wrap(fixture.renderHtml(`${after} continues`));
              updateRichText(host, html, { markdownSource: `${after} continues`, sourceHtmlTransform: wrap });
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
                    updateRichText(host, html, { markdownSource: source });
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
                updateRichText(host, html, { markdownSource: source });
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
              updateRichText(host, html, { markdownSource: source });
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
            updateRichText(host, html, { markdownSource: source, sourceHtmlTransform: transform, sourceHtmlFiltered: filtered });
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
            updateRichText(host, html, { markdownSource: value, sourceHtmlTransform: transform });
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
      return results;
    }, legacy);
    for (const result of results) {
      assert.equal(result.retired, true, 'Markdown-retired code must discard its own control');
      assert.equal(result.kept, true, 'retained code must keep its own pending copy control');
      assert.equal(result.fresh, true, 'a newly appended equal code block needs fresh feedback');
      assert.equal(result.snapshot, result.same ? 'same' : result.index === 0 ? 'gone' : 'stay');
      assert.equal(result.title, result.index === 0 ? 'Copy code' : result.success ? 'Copied' : 'Copy failed',
        'only the copied surviving source may show asynchronous feedback');
    }
  };

  await verifyMarkdownCodeRetirement(false);

  await render('First paragraph.\n\nSecond growing paragraph');
  const crossParagraph = await select('#root .conversation-markdown', 'paragraph.\nSecond');
  await render('First paragraph.\n\nSecond growing paragraph continues');
  assert.equal(await selected(), crossParagraph, 'cross-paragraph selection must survive new output');
  const tail = await select('#root .conversation-markdown', 'growing paragraph', true);
  for (let index = 0; index < 3; index += 1) {
    await render(`First paragraph.\n\nSecond growing paragraph continues ${'more '.repeat(index + 1)}`);
    assert.equal(await selected(), tail, 'selection inside the growing paragraph must survive every frame');
  }
  const backward = await page.evaluate(() => {
    const selection = window.getSelection();
    const range = selection.getRangeAt(0);
    return selection.anchorNode !== range.startContainer || selection.anchorOffset !== range.startOffset;
  });
  assert.equal(backward, true, 'backward selection direction must survive');

  await render('First paragraph.\n\nSecond growing paragraph continues');
  const selectionBox = await page.evaluate(() => {
    window.streamingFixture.select('#root .conversation-markdown', 'growing paragraph');
    const rect = window.getSelection().getRangeAt(0).getBoundingClientRect();
    const box = { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
    window.getSelection().removeAllRanges();
    return box;
  });
  await page.mouse.move(selectionBox.x + 0.5, selectionBox.y + selectionBox.height / 2);
  await page.mouse.down();
  await page.mouse.move(selectionBox.x + selectionBox.width / 2, selectionBox.y + selectionBox.height / 2, { steps: 4 });
  await render('First paragraph.\n\nSecond growing paragraph continues while dragging');
  await page.mouse.move(selectionBox.x + selectionBox.width - 0.5, selectionBox.y + selectionBox.height / 2, { steps: 4 });
  await page.mouse.up();
  assert.equal(await selected(), 'growing paragraph', 'a native drag must remain usable across streaming commits');

  await render('正在生成😀，选择这段文字');
  await select('#root .conversation-markdown', '😀，选择');
  await render('正在生成😀，选择这段文字，后续内容继续出现');
  assert.equal(await selected(), '😀，选择', 'UTF-16 selection offsets must survive CJK animation spans');

  for (const padding of ['', ' earlier'.repeat(1000)]) {
    for (const whitespace of ['  ', '\t']) {
      for (const value of [`hello${whitespace}`, whitespace]) {
        for (const backward of [false, true]) {
          const before = `[A][ref]\n\n\`\`\`\n${padding}hello${whitespace}`;
          await render(before);
          await select('#root code', value, backward);
          assert.equal(await selected(), value, 'fixture must select source-owned code whitespace');
          await render(`${before}world\n\`\`\`\n\n[ref]: /x`);
          assert.equal(await selected(), value, 'code whitespace must survive a preceding reference rewrite');
          assert.equal(await page.evaluate(() => getSelection().anchorNode.parentElement.closest('code') !== null),
            true, 'code whitespace selection must stay inside its code block');
        }
      }
    }
  }

  for (const history of ['', 'context '.repeat(600)]) {
    for (const [opening, closing, expected] of [
      ['<code>', '</code>', ''], ['<pre>', '</pre>', ''], ['<pre><code>', '</code></pre>', ''],
      ['`', '`', 'cop'], ['```text\n', '\n```', 'cop'],
    ]) {
      for (const backward of [false, true]) {
        const before = `${history}${opening}Prefix &cop`;
        await render(before);
        await select('#root .conversation-markdown', 'cop', backward);
        await render(`${before}y;${closing} cop tail`);
        assert.equal(await selected(), expected, 'HTML entities must retire while Markdown code keeps its literal spelling');
        if (!expected) assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
      }
    }
  }

  assert.equal(await page.evaluate(() => {
    const host = document.createElement('div');
    const { updateRichText } = window.streamingFixture;
    updateRichText(host, '<p>Visible</p><template><p>first</p></template>');
    const template = host.querySelector('template');
    for (const html of [
      '<p>Visible tail</p><template><h2>second</h2><template>nested</template></template>',
      '<p>Visible tail</p><template><p>third</p><template><b>changed</b></template></template>',
    ]) {
      updateRichText(host, html);
      const fresh = document.createElement('div');
      fresh.innerHTML = html;
      if (host.innerHTML !== fresh.innerHTML || template !== host.querySelector('template')) return false;
    }
    updateRichText(host, '<div><p>old</p></div>');
    updateRichText(host, '<div><template><b>new</b></template></div>');
    if (host.innerHTML !== '<div><template><b>new</b></template></div>'
      || host.querySelector('template').childNodes.length) return false;
    return true;
  }), true, 'inert template contents must match fresh HTML while retaining their outer node');


  for (const history of ['', 'context '.repeat(1200)]) {
    for (const [opening, closing, entity, suffix, word, expected] of [
      ['<pre>', '</pre>', '&#65', '0;', 'A', ''],
      ['<code>', '</code>', '&#65', '0', 'A', ''],
      ['<code>', '</code>', '&not', 'in;', '¬', ''],
      ['<pre><code>', '</code></pre>', '&not', 'in;', '¬', ''],
      ['<code>', '</code>', '&copy', 'cat;', '©', '©'],
      ['<pre>', '</pre>', '&#9', '7;', '\t', ''],
    ]) {
      for (const backward of [false, true]) {
        const before = `${history}${opening}Prefix ${entity}`;
        await render(before);
        await select('#root .conversation-markdown', word, backward);
        assert.equal(await selected(), word, 'fixture must select the already decoded entity');
        await render(`${before}${suffix}${closing} ${word} tail`);
        assert.equal(await selected(), expected, 'growing decoded entities must retain only their original surviving value');
        assert.equal(await page.evaluate(() => getSelection().isCollapsed), !expected);
      }
    }
  }

  assert.equal(await page.evaluate(async () => {
    const { updateRichText, renderHtml } = window.streamingFixture;
    // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
    const { ensureCodeCopyButtons, copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
    const host = document.createElement('div');
    document.body.append(host);
    const labels = { copyCode: 'Copy code', copied: 'Copied' };
    const options = { sourceHtmlFiltered: true, sourceHtmlTransform: () => '<p>diagnostic mismatch</p>' };
    const before = '```text\nold code';
    const after = `${before}\n\`\`\`\n\n\`\`\`text\nnew code\n\`\`\``;
    try {
      getSelection().removeAllRanges();
      updateRichText(host, renderHtml(before), { ...options, markdownSource: before });
      ensureCodeCopyButtons(host, labels);
      const oldButton = host.querySelector('.pinvou-code-copy-btn');
      let finish;
      const pending = copyCodeBlockFromButton(oldButton, { labels, writeText: () => new Promise(resolve => { finish = resolve; }) });
      const filtered = renderHtml(after).replace(/<pre[\s\S]*?<\/pre>\n?/, '');
      updateRichText(host, filtered, { ...options, markdownSource: after });
      ensureCodeCopyButtons(host, labels);
      const newButton = host.querySelector('.pinvou-code-copy-btn');
      finish(true);
      await pending;
      return !oldButton.isConnected && newButton !== oldButton && newButton.title === 'Copy code'
        && newButton.closest('pre').querySelector('code').textContent === 'new code';
    } finally { host.remove(); }
  }), true, 'failed provenance diagnostics must discard old controls instead of borrowing their feedback');


  const hiddenEntityResults = await page.evaluate(() => {
    const { updateRichText, renderHtml, select } = window.streamingFixture;
    const results = [];
    const host = document.createElement('div');
    host.id = 'hidden-entity-selection';
    document.body.append(host);
    try {
      for (const history of ['', 'context '.repeat(1200)]) {
        for (const [before, suffix, word] of [
          ['<div>A<span title="&#65', '0;"></span></div> A tail', 'A'],
          ['<div>¬<span title="&not', 'in;"></span></div> ¬ tail', '¬'],
          ['<div>cop<span title="&cop', 'y;"></span></div> cop tail', 'cop'],
          ['<div>A<!--&#65', '0;--></div> A tail', 'A'],
          ['<div>A<template>&#65', '0;</template></div> A tail', 'A'],
        ]) {
          for (const backward of [false, true]) {
            getSelection().removeAllRanges();
            const previous = history + before;
            updateRichText(host, renderHtml(previous), { markdownSource: previous });
            select('#hidden-entity-selection', word, backward);
            const initial = getSelection().toString();
            const after = previous + suffix;
            updateRichText(host, renderHtml(after), { markdownSource: after });
            const prefix = document.createRange();
            prefix.selectNodeContents(host);
            const range = getSelection().getRangeAt(0);
            prefix.setEnd(range.startContainer, range.startOffset);
            results.push({ initial, word, selected: getSelection().toString(), prefix: prefix.toString(), history });
          }
        }
      }
      return results;
    } finally { host.remove(); }
  });
  for (const result of hiddenEntityResults) {
    assert.equal(result.initial, result.word);
    assert.equal(result.selected, result.word, 'hidden attributes/comments/template entities must not claim visible text');
    assert.equal(result.prefix, result.history, 'retained visible text must stay at its original occurrence');
  }


  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const history of ['', 'context '.repeat(1200)]) {
      for (const suffix of ['word outside</table>', '<b>word</b></table>']) {
        for (const backward of [false, true]) {
          const before = `${history}<table><tr><td>word</td></tr>`;
          await render(before);
          await select('#root td', 'word', backward);
          await render(before + suffix);
          assert.equal(await selected(), 'word');
          assert.equal(await page.evaluate(() => Boolean(getSelection().anchorNode.parentElement.closest('td'))),
            true, 'HTML foster parenting must keep the selection in its original table cell');
        }
      }
    }
  }

  await verifyTableEndpoints(false);

  const prefixChecks = await page.evaluate(async () => {
    const { updateRichText, renderHtml: renderMarkdown } = window.streamingFixture;
    const sources = [
      'Alpha paragraph with **bravo selected** and trailing text.',
      'Alpha paragraph with *bravo selected* and trailing text.',
      'Alpha paragraph with [bravo selected](https://example.com) and trailing text.',
      'Alpha paragraph with ~~bravo selected~~ and trailing text.',
      'Alpha paragraph with `bravo selected` and trailing text.',
      '- Alpha item\n- Bravo selected\n\n  Charlie paragraph\n- Delta item',
      'Alpha paragraph\n\n| Bravo | Charlie |\n| --- | --- |\n| delta | selected |',
      '[Alpha reference][ref]\n\nBravo selected\n\n[ref]: https://example.com',
    ];
    const host = document.createElement('div');
    document.body.append(host);
    let checks = 0;
    try {
      for (const source of sources) {
        for (let end = 1; end < source.length; end += 1) {
          const before = renderMarkdown(source.slice(0, end));
          const after = renderMarkdown(source.slice(0, end + 1));
          if (before === after) continue;
          const parsed = document.createElement('div');
          parsed.innerHTML = after;
          updateRichText(host, before);
          const words = [...host.textContent.matchAll(/[A-Za-z]{4,}/g)];
          for (const word of words) {
            if (!parsed.textContent.includes(word[0])) continue;
            for (const backward of [false, true]) {
              updateRichText(host, before);
              host.id = 'prefix-selection';
              window.streamingFixture.select('#prefix-selection', word[0], backward);
              const expected = getSelection().toString();
              updateRichText(host, after);
              if (getSelection().toString() !== expected) {
                throw new Error(`Selection changed at prefix ${end}: ${source}, ${word[0]}, backward=${backward}`);
              }
              checks += 1;
            }
          }
        }
      }
    } finally { host.remove(); }
    return checks;
  });
  assert.ok(prefixChecks > 2_000, 'prefix checks must include real forward/backward selections');

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const [before, after] of [
      ['Prefix **selected', 'Prefix **selected** trailing'],
      ['Prefix **selected', 'Prefix **selected** and'],
      ['Prefix **selected', 'Prefix **selected** then **selecte'],
      ['Prefix **selected', 'Prefix **selected** then **selected'],
      ['Prefix **selected\n', `Prefix **selected**${' selected'.repeat(4000)}\n`],
      ['Prefix **selected', 'Prefix **selected** selected'],
      ['Prefix `selected', 'Prefix `selected` trailing'],
      ['Prefix [selected', 'Prefix [selected](https://example.com) trailing'],
      ['Prefix ~~selected', 'Prefix ~~selected~~ trailing'],
    ]) {
      await render(before);
      await select('#root .conversation-markdown', 'selected', true);
      await render(after);
      assert.equal(await selected(), 'selected', `batched Markdown closure must preserve selection, animation=${animation}`);
      assert.equal(await page.evaluate(() => {
        const range = getSelection().getRangeAt(0);
        const prefix = document.createRange();
        prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
        prefix.setEnd(range.startContainer, range.startOffset);
        return prefix.toString() === 'Prefix ';
      }), true, 'selection endpoints must remain in the original formatted word, not a repeated appended word');
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const word of ['word', '选择', '😀', 'a']) {
      for (const [before, after, prefixText] of [
        [`Prefix ***${word}`, `Prefix ***${word}*** ${word}`, 'Prefix '],
        [`Prefix __${word}`, `Prefix __${word}__ __${word}`, 'Prefix '],
        [`[A][long_reference] ${word} ${word}`, `[A][long_reference] ${word} ${word}\n\n[long_reference]: https://example.com`, 'A '],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, true);
        await render(after);
        assert.equal(await selected(), word);
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), prefixText, 'transformed prefix must keep the original occurrence selected');
      }
    }
  }

  for (const word of ['word', '选择', '😀']) {
    for (const backward of [false, true]) {
      for (const occurrence of [0, 1, 2]) {
        const before = `Prefix **${word} ${word} ${word}`;
        await render(before);
        await select('#root .conversation-markdown', word, backward, occurrence);
        await render(`${before}** trailing`);
        assert.equal(await selected(), word);
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), `Prefix ${`${word} `.repeat(occurrence)}`, 'each repeated word must keep its occurrence');
      }
      await render('- word');
      await select('#root .conversation-markdown', 'word', backward);
      await render('- word\n- word\n\n  More output');
      assert.equal(await selected(), 'word');
      assert.equal(await page.evaluate(() => getSelection().getRangeAt(0).startContainer.parentElement.closest('li')
        === document.querySelector('#root li')), true, 'loose-list conversion must retain the original first item');
      await render(word);
      await select('#root .conversation-markdown', word, backward);
      await render(`${word}\n=====\n\n${word}`);
      assert.equal(await selected(), word);
      assert.equal(await page.evaluate(() => getSelection().getRangeAt(0).startContainer.parentElement.closest('h1')
        === document.querySelector('#root h1')), true, 'paragraph promotion must retain selection in the heading');
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const [before, after, word, prefixText] of [
        ['word **word ', 'word **word word** word', 'word', 'word '],
        ['[cherry cherry', '[cherry cherry](https://example.com) cherry', 'cherry', 'cherry '],
        ['**cherry *cherry*', '**cherry *cherry**', 'cherry', '*cherry '],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, backward, 1);
        await render(after);
        assert.equal(await selected(), word);
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), prefixText, 'nested Markdown or a new link tail must not retarget a reused node');
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const id of ['ref', 'word']) {
        await render(`[A][${id}] word word`);
        await select('#root .conversation-markdown', 'word', backward, id === 'word' ? 1 : 0);
        await render(`[A][${id}] word word word\n\n[${id}]: https://example.com`);
        assert.equal(await selected(), 'word');
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), 'A ', 'a disappearing reference identifier must not match retained or appended prose');
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const [before, after, prefixText] of [
        ['![word][ref] word word', '![word][ref] word word word\n\n[ref]: https://example.com/image.png', ' '],
        ['![word][] word word', '![word][] word word word\n\n[word]: https://example.com/image.png', ' '],
        ['![word] word word', '![word] word word word\n\n[word]: https://example.com/image.png', ' '],
        ['![word [word]][ref] word word', '![word [word]][ref] word word word\n\n[ref]: https://example.com/image.png', ' '],
        ['\\![word][ref] word word', '\\![word][ref] word word word\n\n[ref]: https://example.com', '!word '],
      ]) {
        await render(before);
        const occurrence = before.includes('word [word]') ? 2 : 1;
        await select('#root .conversation-markdown', 'word', backward, occurrence);
        await render(after);
        assert.equal(await selected(), 'word');
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), prefixText, 'materialized image alt text must not retarget selected prose');
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const [before, after, word, occurrence, prefixText] of [
        ['[A][ref] [x][word] word word', '[A][ref] [x][word] word word word\n\n[ref]: https://example.com', 'word', 1, 'A [x][word] '],
        ['[A][ref] `foo][word]` word word', '[A][ref] `foo][word]` word word word\n\n[ref]: https://example.com', 'word', 1, 'A foo][word] '],
        ['**[A][word] word word', '**[A][word] word word** word', 'word', 1, '[A][word] '],
        ['\\![word][ref] ![alt][ref] word word', '\\![word][ref] ![alt][ref] word word word\n\n[ref]: https://example.com/image.png', 'word', 1, '!word  '],
        ['![word ![alt](https://example.com/image.png)][ref] word word', '![word ![alt](https://example.com/image.png)][ref] word word word\n\n[ref]: https://example.com/image.png', 'word', 1, ' '],
        ['**[] []', '**[] []** []', '[]', 1, '[] '],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, backward, occurrence);
        await render(after);
        assert.equal(await selected(), word);
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), prefixText, 'only actual disappearing syntax may stop anchoring the original occurrence');
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const [before, after, occurrence, prefixText] of [
        ['[![alt][image]][word] word word', '[![alt][image]][word] word word word\n\n[image]: https://example.com/image.png\n[word]: https://example.com', 1, ' '],
        ['![word ![alt][image]][outer] word word', '![word ![alt][image]][outer] word word word\n\n[image]: https://example.com/image.png\n[outer]: https://example.com/outer.png', 1, ' '],
        ['[word ![alt](https://example.com/image.png)][ref] word word', '[word ![alt](https://example.com/image.png)][ref] word word word\n\n[ref]: https://example.com', 1, 'word  '],
        ['![<em>word</em>][ref] word word', '![<em>word</em>][ref] word word word\n\n[ref]: https://example.com/image.png', 1, ' '],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', 'word', backward, occurrence);
        await render(after);
        assert.equal(await selected(), 'word');
        assert.equal(await page.evaluate(() => {
          const range = getSelection().getRangeAt(0);
          const prefix = document.createRange();
          prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
          prefix.setEnd(range.startContainer, range.startOffset);
          return prefix.toString();
        }), prefixText, 'nested role labels must preserve prose occurrence through reference resolution');
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      const list = Array.from({ length: 40 }, (_, index) => `- Task ${String(index + 1).padStart(2, '0')}: pending`).join('\n');
      await render(list);
      await select('#root .conversation-markdown', 'pending', backward, 30);
      await render(`${list}\n\n  More output`);
      assert.equal(await selected(), 'pending');
      assert.equal(await page.evaluate(() => {
        const range = getSelection().getRangeAt(0);
        const prefix = document.createRange();
        prefix.selectNodeContents(document.querySelector('#root .conversation-markdown'));
        prefix.setEnd(range.startContainer, range.startOffset);
        return prefix.toString().endsWith('Task 31: ');
      }), true, 'large loose-list conversion must retain the numbered selected item');
      for (const [before, after, word, occurrence] of [
        ['word ![word word]', 'word ![word word](https://example.com/image.png) word word', 'word', 1],
        ['Prefix &copy', 'Prefix &copy; copy tail', 'copy', 0],
        ['Prefix\n\n[word]:', 'Prefix\n\n[word]: https://example.com\n\nword', 'word', 0],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, backward, occurrence);
        await render(after);
        assert.equal(await selected(), '', 'entirely removed syntax must collapse instead of selecting unrelated output');
        assert.equal(await page.evaluate(() => window.getSelection().isCollapsed), true);
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      const list = Array.from({ length: 40 }, (_, index) => `- Task ${String(index + 1).padStart(2, '0')}: \`*\` pending`).join('\n');
      await render(list);
      await select('#root .conversation-markdown', '*', backward, 30);
      await render(`${list}\n\n  More output`);
      assert.equal(await selected(), '*', 'large mapping must retain a literal code symbol');
      assert.equal(await page.evaluate(() => getSelection().getRangeAt(0).startContainer.parentElement.closest('li').textContent.includes('Task 31:')), true);
      for (const [before, after, word, occurrence] of [
        ['[x]: https://example.com\n  "tit', '[x]: https://example.com\n  "title"\n\nNew paragraph', 'tit', 0],
        ['[x]: https://example.com\n  (tit', '[x]: https://example.com\n  (title)\n\nNew paragraph', 'tit', 0],
        ['word ![word word', 'word ![word word](https://example.com/image.png) word word', 'word', 1],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, backward, occurrence);
        await render(after);
        assert.equal(await selected(), '', 'retired parser syntax must not select new prose');
        assert.equal(await page.evaluate(() => window.getSelection().isCollapsed), true);
      }
    }
  }

  for (const animation of [false, true]) {
    await page.evaluate(enabled => window.streamingFixture.setAnimation(enabled), animation);
    for (const backward of [false, true]) {
      for (const [before, after, word] of [
        ['Prefix **', 'Prefix **word** then **', '**'],
        ['1', '1. alpha\n\n1 tail', '1'],
        ['42', '42)\n\n42 tail', '42'],
        ['- [x', '- [x] \n\nx tail', 'x'],
        ['- [', '- [ ] \n\n[ tail', '['],
        ['Prefix `', 'Prefix `word` then `', '`'],
        ['Prefix \\', 'Prefix \\* word tail', '\\'],
        ['Prefix &', 'Prefix &copy; word & tail', '&'],
        ['Prefix [', 'Prefix [word](https://example.com) [ tail', '['],
        ['Prefix ![', 'Prefix ![word](https://example.com/a.png) ![ tail', '!['],
        ['Prefix <span', 'Prefix <span>more</span> span tail', 'span'],
        ['Prefix <!--word', 'Prefix <!--word--> word tail', 'word'],
        ['> <span\n> title="word', '> <span\n> title="word">label</span> word tail', 'word'],
        ['- <span\n  title="word', '- <span\n  title="word">label</span> word tail', 'word'],
        ['Prefix <span\r\n title="word', 'Prefix <span\r\n title="word">label</span> word tail', 'word'],
        ['- <span\n\ttitle="word', '- <span\n\ttitle="word">label</span> word tail', 'word'],
      ]) {
        await render(before);
        await select('#root .conversation-markdown', word, backward);
        await render(after);
        assert.equal(await selected(), '', `retired source syntax: ${before}`);
        assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
      }
    }
  }

  for (const backward of [false, true]) {
    for (const marker of ['*', '`', '~~']) {
      const rows = Array.from({ length: 100 }, (_, index) => `- Task ${index}: payload payload`);
      rows[50] = `- Task 50: ${marker}`;
      const before = rows.join('\n');
      await render(before);
      await select('#root li:nth-child(51)', marker, backward);
      await render(`${before}\n\n  More output`);
      assert.equal(await selected(), marker, 'large loose-list conversion must retain isolated literal punctuation');
      assert.equal(await page.evaluate(() => getSelection().getRangeAt(0).startContainer.parentElement.closest('li').textContent.includes('Task 50:')), true);
    }
  }

  for (const [before, after, word] of [
    ['Prefix `**', 'Prefix `**word**`', '**'],
    ['- [x', '- [x]', 'x'],
    ['Prefix &', 'Prefix &copy word & tail', '&'],
    ['Prefix <script', 'Prefix <script>word</script> script tail', 'script'],
    ['Prefix <span>word', 'Prefix <span>word</span> word tail', 'word'],
  ]) {
    await render(before);
    await select('#root .conversation-markdown', word);
    await render(after);
    assert.equal(await selected(), word, 'retained code, literal tags and HTML body text must stay selected');
  }

  for (const [before, after, word] of [
    ['Prefix &copy', 'Prefix &copycat and ©', 'copy'],
    ['Prefix ![word](', 'Prefix ![word](bad\n\n![word](https://example.com/i.png)', 'word'],
    ['Prefix [word](http', 'Prefix [word](httpbad\n\n[word](https://example.com)', 'http'],
  ]) {
    await render(before);
    await select('#root .conversation-markdown', word);
    const prefix = await page.evaluate(() => {
      const range = document.createRange();
      range.selectNodeContents(document.querySelector('#root .conversation-markdown'));
      range.setEnd(getSelection().anchorNode, getSelection().anchorOffset);
      return range.toString();
    });
    await render(after);
    assert.equal(await selected(), word, 'growing literal syntax must retain its original selection');
    assert.equal(await page.evaluate(() => {
      const range = document.createRange();
      range.selectNodeContents(document.querySelector('#root .conversation-markdown'));
      range.setEnd(getSelection().anchorNode, getSelection().anchorOffset);
      return range.toString();
    }), prefix, 'a later rendered role must not consume an earlier literal candidate');
  }

  await render('Prefix &copy');
  await select('#root .conversation-markdown', 'copy');
  await render('Prefix &copycat; and ©');
  assert.equal(await selected(), '', 'browser partial entity decoding retires the old spelling');
  assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);

  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const marker of ['**', '`']) {
      const before = `${history}![gone ${marker}word`;
      await render(before);
      await select('#root .conversation-markdown', 'gone');
      await render(`${before}${marker}\n\n![gone word](https://example.com/i.png)`);
      assert.equal(await selected(), 'gone', 'inner formatting completion must retain the old literal label');
    }
  }

  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const [tail, suffix] of [['&co', 'py;'], ['<em', '>bold</em>']]) {
      const before = `${history}![gone ${tail}`;
      await render(before);
      await select('#root .conversation-markdown', 'gone');
      await render(`${before}${suffix}\n\n![gone ${tail}](https://example.com/i.png)`);
      assert.equal(await selected(), 'gone', 'entity/HTML completion must preserve the original literal image label');
    }
  }

  for (const history of ['', '[q][literal] '.repeat(600)]) {
    for (const [tail, suffix] of [['<em', '>bold</em>'], ['a &co', 'py;']]) {
      const before = `${history}![gone](${tail}`;
      await render(before);
      await select('#root .conversation-markdown', 'gone');
      await render(`${before}${suffix}\n\n![gone](https://example.com/i.png)`);
      assert.equal(await selected(), 'gone', 'inline parsing in an unfinished destination must retain its original label');
    }
  }

  for (const [before, suffix] of [
    ['![gone](https://example.com', '/i.png)'],
    ['![**gone**][re', 'f]\n\n[ref]: https://example.com/i.png'],
  ]) {
    await render(before);
    await select('#root .conversation-markdown', 'gone');
    await render(`${before}${suffix}\n\ngone tail`);
    assert.equal(await selected(), '', 'actual image completion must retire the former label even with a duplicate');
    assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
  }

  for (const backward of [false, true]) {
    for (const suffix of ['[missing]', '(bad']) {
      await render('![gone]');
      await select('#root .conversation-markdown', 'gone', backward);
      await render(`![gone]${suffix}\n\n![gone](https://example.com/i.png)`);
      assert.equal(await selected(), 'gone', 'growing closed literal shortcuts must retain their original selection');
    }
  }
  for (const name of ['script', 'style', 'iframe', 'object', 'embed', 'link', 'meta']) {
    await render(`Prefix <${name}-x`);
    await select('#root .conversation-markdown', `${name}-x`);
    await render(`Prefix <${name}-x> appended`);
    assert.equal(await selected(), `${name}-x`, 'neutralized tag-name extensions remain literal selected text');
  }

  await render('![**gone**]');
  await select('#root .conversation-markdown', 'gone');
  await render('![**gone**]\n\n[**gone**]: https://example.com/i.png\n\n![gone]');
  assert.equal(await selected(), '', 'a resolved formatted shortcut must not retarget a new literal duplicate');
  assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);

  for (const tail of [' more', '\n\nMore paragraph']) {
    const before = `![**gone**]${tail}`;
    await render(before);
    await select('#root .conversation-markdown', 'gone');
    await render(`${before}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
    assert.equal(await selected(), '', 'earlier formatted shortcuts must not retarget a new literal label');
    assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
  }

  for (const backward of [false, true]) {
    for (const prefix of ['Title\n=====\n\n', '# Title #\n\n', '<p>&amp;</p>\n\n', '<div>&copy;</div>\n\n']) {
      const before = `${prefix}![**gone**] more`;
      await render(before);
      await select('#root .conversation-markdown', 'gone', backward);
      await render(`${before}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
      assert.equal(await selected(), '', 'unrelated headings/HTML entities must not disable image source ownership');
      assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
    }
  }

  for (const backward of [false, true]) {
    for (const prefix of ['hello\\\nworld\n\n', 'a | b\\|c\n---|---\nx | y\n\n', '<svg><text>Title</text></svg>\n\n', '<template>hidden</template>\n\n']) {
      const before = `${prefix}![**gone**]\n\nMore paragraph`;
      await render(before);
      await select('#root .conversation-markdown', 'gone', backward);
      await render(`${before}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
      assert.equal(await selected(), '', 'unrelated parser/sanitizer changes must not disable source-local image ownership');
      assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
    }
  }

  for (const backward of [false, true]) {
    await render('<svg>![gone]</svg> !\\[gone]');
    await select('#root .conversation-markdown', 'gone', backward);
    await render('<svg>![gone]</svg> !\\[gone]\n\n[gone]: https://example.com/i.png');
    assert.equal(await selected(), 'gone', 'a sanitizer-hidden source must not retire a retained equal literal');
    const before = '<svg><text>Title</text></svg>\n\n![**gone**] ![gone] ![gone]\n\n<svg><text>End</text></svg>';
    await render(before);
    await select('#root .conversation-markdown', 'gone', backward);
    await render(`${before}\n\n[**gone**]: https://example.com/i.png\n\n![gone]`);
    assert.equal(await selected(), '', 'retired source must collapse despite repeated labels and unrelated sanitizer transforms');
    assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
  }

  await render('![gone **bold]');
  await select('#root .conversation-markdown', 'gone');
  await render('![gone **bold]**\n\n![gone bold](https://example.com/i.png)');
  assert.equal(await selected(), 'gone', 'source-owned images must not retire a closed literal whose internal formatting completes');

  for (const tail of ['**bold**', '`code`', '<b>html</b>']) {
    const before = `![gone ${tail}`;
    await render(before);
    await select('#root .conversation-markdown', 'gone');
    await render(`${before}](https://example.com/i.png)\n\n![gone ${tail.replaceAll(/[*`]|<[^>]*>/g, '')}`);
    assert.equal(await selected(), '', 'formatted image labels must not retarget new literal duplicates');
    assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
  }

  for (const [before, after, word, retained] of [
    ['```js\nword\n`', '```js\nword\n```\n\nnew', '`', ''],
    ['```js\nword\n`', '```js\nword\n```\n\n`tail', '`', ''],
    ['```js\nword\n`', '```js\nword\n```\n\nnew', 'word\n`', 'word'],
    ['~~~js\nword\n~~', '~~~js\nword\n~~~\n\n~~tail', '~~', ''],
  ]) {
    await render(before);
    await select('#root .conversation-markdown', word);
    await render(after);
    assert.equal(await selected(), retained, 'a completed code fence must retire only its former source characters');
  }

  for (const backward of [false, true]) {
    const history = '[q][literal] '.repeat(600);
    const source = `${history}![word **x** ![alt][image]][outer]`;
    await render(source);
    await select('#root .conversation-markdown', 'word', backward);
    await render(`${source}\n\n[image]: https://example.com/i.png\n[outer]: https://example.com/o.png\n\nword tail`);
    assert.equal(await selected(), '', 'bounded nested-image correspondence must retire the original alt');
    assert.equal(await page.evaluate(() => getSelection().isCollapsed), true);
  }

  await render('Title');
  await select('#root .conversation-markdown', 'Title');
  await render('Title\n=====\n\nMore output');
  assert.equal(await selected(), 'Title', 'Markdown paragraph-to-heading conversion must restore the range');

  const code = '```javascript\nconst alpha = 1;\nlet growing';
  await render(code);
  await select('#root code', 'const alpha = 1;');
  await page.evaluate(() => { window.originalCopyButton = document.querySelector('#root .pinvou-code-copy-btn'); });
  await render(`${code} = 2;\nconst omega = 3;`);
  assert.equal(await selected(), 'const alpha = 1;', 'selection in an open code fence must survive growth');
  await render(`${code} = 2;\nconst omega = 3;\n\`\`\``, false);
  assert.equal(await selected(), 'const alpha = 1;', 'completion and syntax highlighting must retain code selection');
  assert.equal(await page.evaluate(() => window.originalCopyButton === document.querySelector('#root .pinvou-code-copy-btn')), true);

  // A real pointer gesture straddles a stream commit, reproducing the lost
  // click from #663's wholesale replacement rather than invoking button.click.
  await page.evaluate(() => { window.getSelection().removeAllRanges(); window.streamingFixture.mode('pending'); });
  const button = await page.$('#root .pinvou-code-copy-btn');
  const box = await button.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await render(`${code} = 22;`);
  await page.mouse.up();
  await page.waitForFunction(() => window.streamingFixture.writes.length === 1);
  const copiedSnapshot = await page.evaluate(() => window.streamingFixture.writes[0]);
  assert.ok(copiedSnapshot.includes('growing = 22;'));
  await render(`${code} = 222;`);
  await page.evaluate(() => window.streamingFixture.finishWrite());
  await page.waitForFunction(() => document.querySelector('#root .pinvou-code-copy-btn').title === 'Copied');
  assert.equal(await page.evaluate(() => window.streamingFixture.writes[0]), copiedSnapshot, 'later output must not alter an in-flight copy');
  await render(`${code} = 2222;`);
  assert.equal(await page.$eval('#root .pinvou-code-copy-btn', node => node.title), 'Copied', 'feedback must stay visible across later frames');

  await page.evaluate(() => window.streamingFixture.mode('failure'));
  await page.click('#root .pinvou-code-copy-btn');
  await page.waitForFunction(() => document.querySelector('#root .pinvou-code-copy-btn').title === 'Copy failed');
  await render(`${code} = 22222;`);
  assert.equal(await page.$eval('#root .pinvou-code-copy-btn', node => node.title), 'Copy failed');

  const listCode = '- Step\n  ```js\n  const x = 1;';
  const looseListCode = `${listCode}\n  \`\`\`\n\n  More output`;
  await render(listCode);
  await page.evaluate(() => {
    window.streamingFixture.mode('pending');
    window.listCopyButton = document.querySelector('#root .pinvou-code-copy-btn');
  });
  await page.click('#root .pinvou-code-copy-btn');
  await render(looseListCode);
  await page.evaluate(() => window.streamingFixture.finishWrite());
  await page.waitForFunction(() => document.querySelector('#root .pinvou-code-copy-btn').title === 'Copied');
  assert.equal(await page.evaluate(() => window.listCopyButton === document.querySelector('#root .pinvou-code-copy-btn')), true,
    'loose-list wrappers must retain a pending copy button and its feedback');
  await render(listCode);
  const listButton = await page.$('#root .pinvou-code-copy-btn');
  const listBox = await listButton.boundingBox();
  const writesBeforeListClick = await page.evaluate(() => window.streamingFixture.writes.length);
  await page.mouse.move(listBox.x + listBox.width / 2, listBox.y + listBox.height / 2);
  await page.mouse.down();
  await render(looseListCode);
  await page.mouse.up();
  await page.waitForFunction(count => window.streamingFixture.writes.length === count + 1, {}, writesBeforeListClick);
  await page.evaluate(() => window.streamingFixture.finishWrite());

  // The same commit path serves legacy HTML bubbles, including selected
  // inline formatting and delayed fade-wrapper cleanup after completion.
  await page.evaluate(() => window.streamingFixture.legacy('<p>Legacy selected <strong>text</strong></p>'));
  await select('#legacy', 'selected text');
  await page.evaluate(() => window.streamingFixture.legacy('<p>Legacy selected <strong>text</strong> keeps streaming</p>'));
  assert.equal(await selected(), 'selected text');
  await select('#legacy', 'keeps streaming');
  await page.evaluate(() => window.streamingFixture.legacy('<p>Legacy selected <strong>text</strong> keeps streaming</p>', false));
  await new Promise(resolve => { setTimeout(resolve, 450); });
  assert.equal(await selected(), 'keeps streaming', 'a selection inside new fade spans must survive their delayed unwrap');

  await render('Inside message');
  await select('#root .conversation-markdown', 'Inside');
  await page.evaluate(() => {
    const selection = window.getSelection();
    selection.setBaseAndExtent(document.querySelector('#outside').firstChild, 3,
      selection.focusNode, selection.focusOffset);
  });
  const crossMessage = await selected();
  await render('Inside message continues');
  assert.equal(await selected(), crossMessage, 'updating one message must preserve an endpoint in another');
  // A separate document loads all legacy dependencies before rendering, so
  // Vite dependency discovery cannot replace React during this component test.
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/tests/fixtures/streaming_legacy_selection.html`, { waitUntil: 'networkidle0' });
  await page.waitForFunction(() => Boolean(window.legacySelectionFixture));
  await verifyMarkdownCodeRetirement(true);
  const legacyThrottle = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const [before, hidden, after, word] of [
      ['Prefix <span', 'Prefix <span>', 'Prefix <span>word</span> span tail', 'span'],
      ['Prefix <!--word', 'Prefix <!--word-->', 'Prefix <!--word--> word tail', 'word'],
      ['Prefix **', 'Prefix **word', 'Prefix **word** then **', '**'],
      ['Prefix [', 'Prefix [word', 'Prefix [word](https://example.com) [ tail', '['],
    ]) {
      getSelection().removeAllRanges();
      const html = fixture.renderHtml(before);
      fixture.render(before, html, before);
      fixture.select(word);
      fixture.render(hidden, html, before);
      const duringThrottle = getSelection().toString();
      fixture.render(after, fixture.renderHtml(after), after);
      results.push({ word, duringThrottle, selected: getSelection().toString(), collapsed: getSelection().isCollapsed });
    }
    return results;
  });
  for (const result of legacyThrottle) {
    assert.equal(result.duringThrottle, result.word, 'text-only legacy deltas must retain visible source ownership');
    assert.equal(result.selected, '');
    assert.equal(result.collapsed, true, 'the real legacy bubble must collapse retired syntax when HTML catches up');
  }

  const legacyCopyOwnership = await page.evaluate(async () => {
    const fixture = window.legacySelectionFixture;
    // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
    const { copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
    const labels = { copyCode: 'Copy code', copied: 'Copied', failed: 'Copy failed' };
    const results = [];
    for (const attribute of [null, '', ' data-x="<pre>"', ' title="<pre>"', " data-x='<pre>'", ' data-x="<pre title=\'x\'>"']) {
      const raw = attribute !== null;
      getSelection().removeAllRanges();
      const opening = raw ? `<pre class="pinvou-code-block"${attribute}><code>` : '```json\n';
      const closing = raw ? '</code></pre>' : '\n```';
      const before = `${opening}{"description":"old",`;
      const after = `${before}"name":"A","body":"B"}${closing}\n\n\`\`\`text\nnew unrelated code\n\`\`\``;
      fixture.render(before, fixture.renderHtml(before), before);
      const oldButton = document.querySelector('#root .msg-md .pinvou-code-copy-btn');
      let finish;
      let snapshot;
      const pending = copyCodeBlockFromButton(oldButton, { labels, writeText: value => {
        snapshot = value;
        return new Promise(resolve => { finish = resolve; });
      } });
      fixture.render(after, fixture.renderHtml(after), after, false);
      const newButton = document.querySelector('#root .msg-md .pinvou-code-copy-btn');
      finish(true);
      await pending;
      results.push({ raw, attribute, replaced: newButton !== oldButton && !oldButton.isConnected,
        snapshot, title: newButton.title, code: newButton.closest('pre').querySelector('code').textContent });
    }
    // A surviving block must keep its own pending feedback when a preceding
    // structured draft is extracted without changing the Markdown source.
    const source = '```json\n{"name":"A","body":"B"}\n```\n\n```text\nretained code\n```';
    fixture.render(source, fixture.renderHtml(source), source);
    const buttons = [...document.querySelectorAll('#root .msg-md .pinvou-code-copy-btn')];
    const retained = buttons.at(-1);
    let finish;
    const pending = copyCodeBlockFromButton(retained, { labels, writeText: () => new Promise(resolve => { finish = resolve; }) });
    fixture.render(source, fixture.renderHtml(source), source, false);
    const visible = document.querySelector('#root .msg-md .pinvou-code-copy-btn');
    finish(true);
    await pending;
    return { results, kept: visible === retained && retained.isConnected, keptTitle: visible.title };
  });
  for (const result of legacyCopyOwnership.results) {
    assert.equal(result.replaced, true, 'discarded source code must discard its copy control');
    assert.equal(result.snapshot, '{"description":"old",');
    assert.equal(result.code, 'new unrelated code');
    assert.equal(result.title, 'Copy code', 'old copy feedback must not transfer to a new source block');
  }
  assert.equal(legacyCopyOwnership.kept, true, 'a surviving source block must keep its control through earlier extraction');
  assert.equal(legacyCopyOwnership.keptTitle, 'Copied');

  const legacyHtmlEntities = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const [opening, closing, expected] of [
      ['<code>', '</code>', ''], ['<pre>', '</pre>', ''], ['<pre><code>', '</code></pre>', ''],
      ['`', '`', 'cop'], ['```text\n', '\n```', 'cop'],
    ]) {
      for (const backward of [false, true]) {
        getSelection().removeAllRanges();
        const before = `${opening}Prefix &cop`;
        const after = `${before}y;${closing} cop tail`;
        fixture.render(before, fixture.renderHtml(before), before);
        fixture.select('cop', backward);
        fixture.render(after, fixture.renderHtml(after), after);
        results.push({ expected, selected: getSelection().toString(), collapsed: getSelection().isCollapsed });
      }
    }
    return results;
  });
  for (const result of legacyHtmlEntities) {
    assert.equal(result.selected, result.expected, 'legacy HTML entities must not override Markdown code literal ownership');
    if (!result.expected) assert.equal(result.collapsed, true);
  }


  const legacyDecodedEntities = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const history of ['', 'context '.repeat(1200)]) {
      for (const [opening, closing, entity, suffix, word, expected] of [
        ['<pre>', '</pre>', '&#65', '0;', 'A', ''],
        ['<code>', '</code>', '&#65', '0', 'A', ''],
        ['<code>', '</code>', '&not', 'in;', '¬', ''],
        ['<pre><code>', '</code></pre>', '&not', 'in;', '¬', ''],
        ['<code>', '</code>', '&copy', 'cat;', '©', '©'],
        ['<pre>', '</pre>', '&#9', '7;', '\t', ''],
      ]) {
        for (const backward of [false, true]) {
          getSelection().removeAllRanges();
          const before = `${history}${opening}Prefix ${entity}`;
          const after = `${before}${suffix}${closing} ${word} tail`;
          fixture.render(before, fixture.renderHtml(before), before);
          fixture.select(word, backward);
          const initial = getSelection().toString();
          fixture.render(after, fixture.renderHtml(after), after);
          results.push({ initial, word, expected, selected: getSelection().toString(), collapsed: getSelection().isCollapsed });
        }
      }
    }
    return results;
  });
  for (const result of legacyDecodedEntities) {
    assert.equal(result.initial, result.word, 'legacy fixture must select the already decoded entity');
    assert.equal(result.selected, result.expected, 'legacy decoded entities must retain only their surviving value');
    assert.equal(result.collapsed, !result.expected);
  }


  const legacyHtmlOwnership = await page.evaluate(async () => {
    const fixture = window.legacySelectionFixture;
    // eslint-disable-next-line import-x/no-unresolved, n/no-missing-import -- Browser-side Vite URL, not a Node filesystem import.
    const { copyCodeBlockFromButton } = await import('/src/features/conversation/code-block-copy.js');
    const copies = [];
    for (const prefix of ['', '<?x "\'>\n', '<div data-x=a"b>', '<!x "\'>\n', '<textarea>raw</textarea/>\n\n',
      '<div ="x>', "<div ='x>", '<div /="x>', '<div a=\u00a0"x>', '<div a=\v"x>', '<div a=x\u00a0="y>', '<![CDATA[>']) {
      getSelection().removeAllRanges();
      const source = `${prefix}<pre class="pinvou-code-block"><code>retained code</code></pre>\n\n`
        + '<pre class="pinvou-code-block"><code>{"name":"A","body":"B"}</code></pre>';
      fixture.render(source, fixture.renderHtml(source), source);
      const oldButton = document.querySelector('#root .msg-md .pinvou-code-copy-btn');
      let finish;
      const pending = copyCodeBlockFromButton(oldButton, { writeText: () => new Promise(resolve => { finish = resolve; }) });
      fixture.render(source, fixture.renderHtml(source), source, false);
      const newButton = document.querySelector('#root .msg-md .pinvou-code-copy-btn');
      finish(true);
      await pending;
      copies.push({ prefix, same: oldButton === newButton && oldButton.isConnected, title: newButton.title });
    }
    const selections = [];
    for (const [before, suffix, word] of [
      ['<div>A<span title="&#65', '0;"></span></div> A tail', 'A'],
      ['<div>¬<span title="&not', 'in;"></span></div> ¬ tail', '¬'],
      ['<div>cop<span title="&cop', 'y;"></span></div> cop tail', 'cop'],
      ['<div>A<!--&#65', '0;--></div> A tail', 'A'],
      ['<div>A<template>&#65', '0;</template></div> A tail', 'A'],
    ]) {
      for (const backward of [false, true]) {
        getSelection().removeAllRanges();
        fixture.render(before, fixture.renderHtml(before), before);
        fixture.select(word, backward);
        const after = before + suffix;
        fixture.render(after, fixture.renderHtml(after), after);
        const prefix = document.createRange();
        prefix.selectNodeContents(document.querySelector('#root .msg-md'));
        const range = getSelection().getRangeAt(0);
        prefix.setEnd(range.startContainer, range.startOffset);
        selections.push({ word, selected: getSelection().toString(), prefix: prefix.toString() });
      }
    }
    const entities = [];
    for (const active of [false, true]) {
      for (const backward of [false, true]) {
        getSelection().removeAllRanges();
        const before = '```json\n{"name":"X","body":"B"}\n```\n\n<code>Prefix &#65';
        const after = `${before}0;</code> A tail`;
        fixture.render(before, fixture.renderHtml(before), before, active);
        fixture.select('A', backward);
        const initial = getSelection().toString();
        fixture.render(after, fixture.renderHtml(after), after, active);
        entities.push({ initial, selected: getSelection().toString(), collapsed: getSelection().isCollapsed });
      }
    }
    return { copies, selections, entities };
  });
  for (const result of legacyHtmlOwnership.copies) {
    assert.equal(result.same, true, `HTML boundaries must retain surviving source controls: ${result.prefix}`);
    assert.equal(result.title, 'Copied');
  }
  for (const result of legacyHtmlOwnership.selections) {
    assert.equal(result.selected, result.word, 'legacy hidden HTML must not retire original visible text');
    assert.equal(result.prefix, '');
  }
  for (const result of legacyHtmlOwnership.entities) {
    assert.equal(result.initial, 'A');
    assert.equal(result.selected, '', 'filters elsewhere cannot hide ownership of a visible entity');
    assert.equal(result.collapsed, true);
  }


  const legacyFoster = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const history of ['', 'context '.repeat(1200)]) {
      for (const backward of [false, true]) {
        getSelection().removeAllRanges();
        const before = `${history}<table><tr><td>word</td></tr>`;
        const after = `${before}word outside</table>`;
        fixture.render(before, fixture.renderHtml(before), before);
        fixture.select('word', backward, '#root td');
        fixture.render(after, fixture.renderHtml(after), after);
        results.push({ selected: getSelection().toString(), cell: Boolean(getSelection().anchorNode.parentElement.closest('td')) });
      }
    }
    return results;
  });
  for (const result of legacyFoster) {
    assert.equal(result.selected, 'word');
    assert.equal(result.cell, true, 'legacy HTML foster parenting must retain the original cell selection');
  }

  await verifyTableEndpoints(true);

  const legacyDrafts = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const protocol of ['persona-card', 'card-question', 'scheduled-task-draft']) {
      for (const field of ['name', 'question', 'rrule']) {
        for (const label of ['gone', 'Designing']) {
          for (const backward of [false, true]) {
            for (const complete of [false, true]) {
              for (const retained of [false, true]) {
                getSelection().removeAllRanges();
                const before = `![**${label}**] retained ${label}\n\n\`\`\`${protocol}\n{"${field}":`;
                const after = `${before}"A"}\n\`\`\`\n\n[**${label}**]: /x`;
                fixture.render(before, fixture.renderHtml(before), before);
                const selectedText = retained ? `retained ${label}` : label;
                fixture.select(selectedText, backward);
                fixture.render(after, fixture.renderHtml(after), after, !complete);
                results.push({ protocol, field, label, backward, complete, retained, selectedText, selected: getSelection().toString(),
                  collapsed: getSelection().isCollapsed });
              }
            }
          }
        }
      }
    }
    return results;
  });
  for (const result of legacyDrafts) {
    assert.equal(result.selected, result.retained ? result.selectedText : '', `folded legacy draft must retire its old image label: ${JSON.stringify(result)}`);
    assert.equal(result.collapsed, !result.retained, 'draft folding or completion must not retarget repeated prose or placeholders');
  }

  const recognizedDrafts = await page.evaluate(() => {
    const fixture = window.legacySelectionFixture;
    const results = [];
    for (const raw of [false, true]) {
      for (const prefix of ['', '[A][ref] retained Designing\n\n', '```text\nDesigning\n```\n\n']) {
        for (const word of ['Designing', '  ', '\t']) {
          for (const backward of [false, true]) {
            for (const complete of [false, true]) {
              getSelection().removeAllRanges();
              const opening = raw ? '<pre><code>' : '```json\n';
              const closing = raw ? '</code></pre>' : '\n```';
              const before = `${prefix}${opening}{"description":"${word}",`;
              const after = `${before}"name":"A","body":"B"}${complete || raw ? closing : ''}\n\n[ref]: /x`;
              fixture.render(before, fixture.renderHtml(before), before);
              fixture.select(word, backward, '#root .msg-md pre:last-of-type code');
              const initial = getSelection().toString();
              fixture.render(after, fixture.renderHtml(after), after, !complete);
              const removed = ![...document.querySelectorAll('#root .msg-md pre code')]
                .some(code => code.textContent.includes('"description"'));
              results.push({ raw, prefix, word, backward, complete, initial, removed, selected: getSelection().toString(),
                collapsed: getSelection().isCollapsed });
            }
          }
        }
      }
    }
    for (const backward of [false, true]) {
      getSelection().removeAllRanges();
      const before = '```text\nDesigning\n```\n\n```json\n{"description":"Designing",';
      const after = before + '"name":"A"}';
      fixture.render(before, fixture.renderHtml(before), before);
      fixture.select('Designing', backward, '#root .msg-md pre:first-of-type code');
      const initial = getSelection().toString();
      fixture.render(after, fixture.renderHtml(after), after);
      results.push({ backward, word: 'Designing', initial, removed: false,
        selected: getSelection().toString(), collapsed: getSelection().isCollapsed });
    }
    return results;
  });
  for (const result of recognizedDrafts) {
    assert.equal(result.initial, result.word, 'the fixture must select the code payload before recognition');
    assert.equal(result.selected, result.removed ? '' : result.word, `newly recognized drafts must not retarget code selection: ${JSON.stringify(result)}`);
    assert.equal(result.collapsed, result.removed, 'presentation-removed code selection must collapse');
  }

  // Exercise the actual auxiliary panel's parent effect as well as the shared
  // timeline: the parent must not override selection-aware scroll following.
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/tests/fixtures/streaming_aux_selection.html`, { waitUntil: 'networkidle0' });
  await page.waitForSelector('.conversation-markdown');
  await new Promise(resolve => { setTimeout(resolve, 350); });
  const auxBefore = await page.evaluate(async () => {
    const host = document.querySelector('.conversation-markdown');
    const scroller = document.querySelector('.overflow-y-auto');
    scroller.scrollTop = scroller.scrollHeight;
    await new Promise(resolve => { setTimeout(resolve, 50); });
    const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (!node.data.includes('Line 19')) continue;
      const start = node.data.indexOf('selected');
      window.getSelection().setBaseAndExtent(node, start, node, start + 8);
      break;
    }
    return { scrollTop: scroller.scrollTop, height: scroller.scrollHeight,
      selectionTop: window.getSelection().getRangeAt(0).getBoundingClientRect().top };
  });
  for (let index = 0; index < 2; index += 1) {
    await page.evaluate(() => window.auxSelectionFixture.grow());
    await page.waitForFunction(height => document.querySelector('.overflow-y-auto').scrollHeight > height, {}, auxBefore.height);
    await new Promise(resolve => { setTimeout(resolve, 350); });
    const state = await page.evaluate(() => ({
      scrollTop: document.querySelector('.overflow-y-auto').scrollTop,
      selected: window.getSelection().toString(),
      selectionTop: window.getSelection().getRangeAt(0).getBoundingClientRect().top,
    }));
    assert.equal(state.selected, 'selected');
    assert.equal(state.scrollTop, auxBefore.scrollTop, 'auxiliary output must retain the selected reading position');
    assert.ok(Math.abs(state.selectionTop - auxBefore.selectionTop) < 1, 'auxiliary selection must stay in view');
  }
  assert.deepEqual(errors, []);
  console.log('Streaming rich text: selection, growing code, pointer copy, feedback, completion and legacy checks passed');
  if (process.env.PINVOU_NATIVE_WEBKIT === '1') {
    const result = await promisify(execFile)('python3', [
      path.join(appRoot, 'tests/streaming_rich_text_webkit.py'),
      `http://127.0.0.1:${server.httpServer.address().port}/tests/fixtures/streaming_rich_text.html`,
    ], { timeout: 80_000 });
    console.log(result.stdout.trim());
  }
} finally {
  if (browser) await browser.close();
  if (server) await server.close();
}
