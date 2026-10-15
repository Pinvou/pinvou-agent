#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import domino from '@mixmark-io/domino';

import {
  applyStreamFade,
  isStreamFadePreferenceEnabled,
  planFadePass,
  setStreamFadePreference,
  splitFadeSegments,
} from '../src/features/conversation/stream-fade.js';

const testDir = path.dirname(fileURLToPath(import.meta.url));
const appRoot = path.join(testDir, '..');
const read = (...parts) => fs.readFileSync(path.join(appRoot, ...parts), 'utf8');

const streamFadeSource = read('src', 'features', 'conversation', 'stream-fade.js');
const baseCss = read('src', 'styles', 'base.css');
const chatView = read('src', 'features', 'chat', 'ChatView.jsx');
const timeline = read('src', 'features', 'conversation', 'ConversationTimeline.jsx');
const codeCopy = read('src', 'features', 'conversation', 'code-block-copy.js');

// Deterministic stand-in for Intl.Segmenter: whitespace runs keep together,
// ASCII words keep together, every other code point (CJK ideographs) is its
// own piece — the same shape granularity:'word' produces for zh text.
function fakeWordSegmenter() {
  return {
    segment(text) {
      return (String(text).match(/\s+|[\dA-Za-z']+|./gu) || []).map((piece) => ({ segment: piece }));
    },
  };
}

// ---- splitFadeSegments (pure) ----

// English words: trailing whitespace attaches to the preceding segment.
assert.deepEqual(splitFadeSegments('Hello world', fakeWordSegmenter()), ['Hello ', 'world']);
// CJK: each ideograph fades on its own step.
assert.deepEqual(splitFadeSegments('你好世界测试', fakeWordSegmenter()), ['你', '好', '世', '界', '测', '试']);
// Mixed CJK/latin with inner whitespace, never a whitespace-only segment.
assert.deepEqual(
  splitFadeSegments('你好 world 测试', fakeWordSegmenter()),
  ['你', '好 ', 'world ', '测', '试'],
);
// Leading whitespace attaches forward; whitespace-only text fades nothing.
assert.deepEqual(splitFadeSegments('  indented', fakeWordSegmenter()), ['  indented']);
assert.deepEqual(splitFadeSegments('   ', fakeWordSegmenter()), []);
assert.deepEqual(splitFadeSegments('', fakeWordSegmenter()), []);

// Real Intl.Segmenter (when the runtime has one) must agree on the CJK
// per-ideograph contract; pinned with an explicit locale for determinism.
if (typeof Intl === 'object' && Intl !== null && typeof Intl.Segmenter === 'function') {
  const zhSegmenter = new Intl.Segmenter('zh', { granularity: 'word' });
  assert.deepEqual(splitFadeSegments('你好世界测试', zhSegmenter), ['你', '好', '世', '界', '测', '试']);
  assert.deepEqual(splitFadeSegments('Hello world', zhSegmenter), ['Hello ', 'world']);
}

// Fallback path (segmenter unavailable): fixed 4-char chunks, same
// whitespace rules.
assert.deepEqual(splitFadeSegments('hello world', null), ['hell', 'o wo', 'rld']);
assert.deepEqual(splitFadeSegments('abcdefg', null), ['abcd', 'efg']);
assert.deepEqual(splitFadeSegments('    x', null), ['    x']);
assert.deepEqual(splitFadeSegments('   ', null), []);
// Surrogate pairs stay intact under the u-flag fallback ({1,4} counts code
// points, so the emoji is never split in half).
assert.deepEqual(splitFadeSegments('ab😀cd', null), ['ab😀c', 'd']);

// ---- planFadePass (pure) ----

// First pass on a container: hydration baseline, nothing animates.
assert.deepEqual(planFadePass({ started: false, committed: 0, total: 0 }), { action: 'baseline', from: 0 });
assert.deepEqual(planFadePass({ started: false, committed: 0, total: 120 }), { action: 'baseline', from: 120 });
// Growth animates exactly the region past the committed offset.
assert.deepEqual(planFadePass({ started: true, committed: 11, total: 24 }), { action: 'animate', from: 11 });
// Large jumps stay a single animate region (the cap is applied later, in the
// DOM pass, not by re-planning).
assert.deepEqual(planFadePass({ started: true, committed: 5, total: 500 }), { action: 'animate', from: 5 });
// Equal length: nothing new, cheap no-op pass.
assert.deepEqual(planFadePass({ started: true, committed: 24, total: 24 }), { action: 'none', from: 24 });
// Shrink (markdown re-parse rendered shorter text): re-baseline, no fade.
assert.deepEqual(planFadePass({ started: true, committed: 30, total: 12 }), { action: 'baseline', from: 12 });

// ---- source shape pins (voice_pill_animation style) ----

// CSS: keyframes + class with `both` fill and the stagger variable, placed
// right after the streaming-cursor block it extends.
assert.match(baseCss, /@keyframes streamFadeIn \{ from \{ opacity:0; \} to \{ opacity:1; \} \}/);
assert.match(
  baseCss,
  /\.stream-fade-seg \{ animation: streamFadeIn 0\.15s ease-out both; animation-delay: var\(--stream-fade-delay, 0ms\); \}/,
);
assert.ok(
  baseCss.indexOf('@keyframes streamFadeIn') > baseCss.indexOf('.streaming-cursor::after'),
  'stream-fade CSS must come after the streaming-cursor block',
);
assert.ok(
  baseCss.indexOf('@keyframes streamFadeIn') < baseCss.indexOf('.no-scrollbar'),
  'stream-fade CSS must sit between the streaming-cursor and Tool Store blocks',
);
// Reduced motion disables the fade, inside its own prefers-reduced-motion block.
const fadeReduceOffset = baseCss.indexOf('.stream-fade-seg { animation: none; }');
assert.ok(fadeReduceOffset > baseCss.indexOf('@keyframes streamFadeIn'), 'reduced-motion override must exist');
const fadeMediaStart = baseCss.lastIndexOf('@media (prefers-reduced-motion: reduce)', fadeReduceOffset);
assert.ok(fadeMediaStart >= 0, 'stream-fade reduced-motion override must live in a prefers-reduced-motion block');
assert.match(
  baseCss.slice(fadeMediaStart, fadeReduceOffset),
  /@media \(prefers-reduced-motion: reduce\) \{\s*$/,
  'the fade override must be inside the reduced-motion block',
);

// Wiring: both surfaces call the fade pass from their React effects with a
// streaming flag, imported from the conversation module.
assert.match(chatView, /import \{ applyStreamFade \} from '\.\.\/conversation\/stream-fade\.js';/);
assert.match(chatView, /applyStreamFade\(target, \{ streaming: !!item\.streaming \}\);/);
assert.match(timeline, /import \{ applyStreamFade \} from '\.\/stream-fade\.js';/);
assert.match(timeline, /applyStreamFade\(containerRef\.current, \{ streaming \}\);/);

// The fade pass mutates the container, so it must never be wired from a
// MutationObserver callback (recursion): the observer module must not know
// about it, and the module itself must not install an observer.
assert.doesNotMatch(codeCopy, /applyStreamFade/, 'code-copy observer module must not reference the fade pass');
assert.doesNotMatch(streamFadeSource, /new MutationObserver/, 'stream-fade.js must not install observers');

// ---- DOM pass against a real (domino) tree ----
// The module guards degrade cleanly under Node: no window/matchMedia, and the
// ancestor re-check makes up for domino not skipping FILTER_REJECT subtrees.

function documentWithContainer(html) {
  const doc = domino.createDocument(`<div id="m">${html}</div>`);
  return doc.getElementById('m');
}

function wrappedText(container) {
  return Array.from(container.querySelectorAll('.stream-fade-seg'), (span) => span.textContent).join('');
}

setStreamFadePreference(true);
try {
  // First pass is a hydration baseline: nothing wrapped, state committed.
  {
    const container = documentWithContainer('<p>hello world</p>');
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 'baseline pass must not wrap');

    // Stream tick: innerHTML wholesale replacement, region past the committed
    // offset fades in segments while seen text stays plain.
    container.innerHTML = '<p>hello world of streaming</p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.textContent, 'hello world of streaming', 'wrap must preserve text content');
    assert.equal(container.querySelectorAll('p').length, 1, 'wrap must not leave stray spans outside paragraphs');
    assert.equal(wrappedText(container), ' of streaming', 'only text past the committed offset fades');
    const delays = Array.from(container.querySelectorAll('.stream-fade-seg'), (span) => span.getAttribute('style'));
    assert.ok(delays.every((style) => style === null || /^--stream-fade-delay:\d+ms$/.test(style)), 'stagger style shape');

    // Same DOM twice: idempotent, committed === total, no extra spans.
    applyStreamFade(container, { streaming: true });
    assert.equal(wrappedText(container), ' of streaming');

    // Completion: final render replaces innerHTML (spans gone) and the
    // streaming=false pass forgets the container.
    container.innerHTML = '<p>hello world of streaming</p>';
    applyStreamFade(container, { streaming: false });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0);

    // Completion with an UNCHANGED html string: React skips the
    // dangerouslySetInnerHTML write, so the streaming=false pass must unwrap
    // the leftover fade spans itself (text content intact).
    {
      const skipContainer = documentWithContainer('<p>grow</p>');
      applyStreamFade(skipContainer, { streaming: true }); // baseline
      skipContainer.innerHTML = '<p>grow tail</p>';
      applyStreamFade(skipContainer, { streaming: true });
      assert.ok(skipContainer.querySelectorAll('.stream-fade-seg').length > 0, 'precondition: spans exist');
      // html string unchanged since the last tick — no React rewrite.
      applyStreamFade(skipContainer, { streaming: false });
      assert.equal(skipContainer.querySelectorAll('.stream-fade-seg').length, 0, 'completion unwraps residual spans');
      assert.equal(skipContainer.textContent, 'grow tail', 'unwrap preserves text');
    }

    // A fresh stream after completion re-baselines instead of replaying.
    container.innerHTML = '<p>hello world of streaming plus more</p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 'fresh stream must re-baseline');
  }

  // Code block subtrees neither count nor fade: only surrounding text animates.
  {
    const container = documentWithContainer('<p>before</p><pre><code>code text</code></pre>');
    applyStreamFade(container, { streaming: true }); // baseline counts 'before' only
    container.innerHTML = '<p>before</p><pre><code>code text grown</code></pre><p>after</p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelector('pre code').textContent, 'code text grown', 'code block untouched');
    assert.equal(wrappedText(container), 'after', 'growing code must not shift the offset nor fade');
  }

  // A whitespace-only new region is never wrapped alone.
  {
    const container = documentWithContainer('<p>hello</p>');
    applyStreamFade(container, { streaming: true });
    container.innerHTML = '<p>hello   </p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 'whitespace-only region stays unwrapped');
    assert.equal(container.textContent, 'hello   ', 'whitespace preserved');
  }

  // Hard cap: at most 40 segments animate per pass; the rest appears instantly
  // with the text content still intact.
  {
    const words = Array.from({ length: 100 }, (_, i) => `w${i}`).join(' ');
    const container = documentWithContainer('');
    applyStreamFade(container, { streaming: true }); // empty baseline
    container.innerHTML = `<p>${words}</p>`;
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 40, 'per-pass segment cap');
    assert.equal(container.textContent, words, 'cap must not drop text');
  }

  // Preference: disabled passes forget the bookkeeping, so a mid-stream
  // re-enable re-baselines the container and text that streamed while the
  // animation was off never re-fades.
  {
    const container = documentWithContainer('<p>seen</p>');
    applyStreamFade(container, { streaming: true }); // baseline commits 'seen'
    setStreamFadePreference(false);
    assert.equal(isStreamFadePreferenceEnabled(), false);
    container.innerHTML = '<p>seen and more</p>';
    applyStreamFade(container, { streaming: true }); // disabled: no wrap, state forgotten
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0);
    setStreamFadePreference(true);
    container.innerHTML = '<p>seen and more text</p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 're-enable re-baselines; disabled-era text never re-fades');
  }
} finally {
  // Leave the module default (enabled) for any later blocks/tests.
  setStreamFadePreference(true);
}

console.log('stream_fade: ok');
