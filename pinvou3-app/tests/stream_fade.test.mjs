#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { mock } from 'node:test';

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
// Chunk boundaries are repaired so grapheme clusters never split: a broken
// ZWJ sequence, half a flag, or a detached combining mark renders wrong until
// the completion unwrap re-joins the text.
assert.deepEqual(splitFadeSegments('a👨‍👩‍👧b', null), ['a👨‍👩‍👧b']);
assert.deepEqual(splitFadeSegments('👨‍👩‍👧‍👦xy', null), ['👨‍👩‍👧‍👦x', 'y']);
assert.deepEqual(splitFadeSegments('abc🇺🇸de', null), ['abc🇺🇸de']);
assert.deepEqual(splitFadeSegments('abcd\u0301efg', null), ['abcd\u0301efg']);

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
// streaming flag and the html string they inject, imported from the
// conversation module.
assert.match(chatView, /import \{ applyStreamFade \} from '\.\.\/conversation\/stream-fade\.js';/);
assert.match(chatView, /applyStreamFade\(target, \{ streaming: !!item\.streaming, html: assistantParsed\.cq\.html \|\| '' \}\);/);
assert.match(timeline, /import \{ applyStreamFade \} from '\.\/stream-fade\.js';/);
assert.match(timeline, /applyStreamFade\(containerRef\.current, \{ streaming, html \}\);/);
// The completion flush can coincide with no html change (final text already
// rendered), so the streaming=false pass must still run: `streaming` must stay
// a dependency of the install effect.
assert.match(timeline, /\}, \[html, copyLabels, streaming\]\);/, 'the install effect must keep streaming (and html) as dependencies so the completion pass always runs');

// The fade pass mutates the container, so it must never be wired from a
// MutationObserver callback (recursion): the observer module must not know
// about it, and the module itself must not install an observer.
assert.doesNotMatch(codeCopy, /applyStreamFade/, 'code-copy observer module must not reference the fade pass');
assert.doesNotMatch(streamFadeSource, /new MutationObserver/, 'stream-fade.js must not install observers');

// Preference sync: both shells must feed the module flag from the resolved
// chat preference, and the settings toggle must write the desktop-only chat
// key through the settings bridge — a dropped sync would strand the module on
// its compiled-in default while the UI shows the user's choice.
const mainShell = read('src', 'app', 'main.jsx');
const detachedShell = read('src', 'app', 'DetachedShell.jsx');
const settingsView = read('src', 'features', 'settings', 'SettingsView.jsx');
assert.match(mainShell, /setStreamFadePreference\(chatStreamingAnimation\);/);
assert.match(mainShell, /saveSettings\(\{ chat: \{ streaming_animation: !!enabled \} \}\)/);
assert.match(detachedShell, /setStreamFadePreference\(chatStreamingAnimation\);/);
assert.match(detachedShell, /streaming_animation !== false/, 'detached shell must resolve the same default-on polarity');
assert.match(
  settingsView,
  /\{!isWeb && \(\s*<IOSSection title=\{t\.chatSection\}>\s*<IOSRow label=\{t\.chatStreamingAnimation\}/,
  'the chat row must stay desktop-only',
);

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

const nowMs = () => Date.now();

// Backdate every pass-created span's creation marker so a completion pass sees
// it as older than the unwrap age floor (simulates a stream that paused before
// completing).
function ageFadeSpans(container, ms = 1000) {
  for (const span of container.querySelectorAll('.stream-fade-seg')) {
    if (span.hasAttribute('data-stream-fade')) span.setAttribute('data-stream-fade', String(nowMs() - ms));
  }
}

function spanDelays(container) {
  return Array.from(container.querySelectorAll('.stream-fade-seg'), (span) => {
    const style = span.getAttribute('style');
    return style === null ? 0 : Number(style.match(/--stream-fade-delay:(\d+)ms/)[1]);
  });
}

setStreamFadePreference(true);
// Keep callbacks and span ages on the same controlled clock without relying
// on experimental timer APIs on the project's supported Node 22 runtime: the
// exact UNWRAP_MIN_AGE_MS boundary (199 must not unwrap, +1 must) can only be
// pinned against a fake clock, not a real sleep.
function createFadeClock() {
  let now = 1_700_000_000_000;
  const timers = new Map();
  mock.method(Date, 'now', () => now);
  mock.method(globalThis, 'setTimeout', (callback, delay) => {
    const timer = {};
    timers.set(timer, { callback, due: now + delay });
    return timer;
  });
  mock.method(globalThis, 'clearTimeout', (timer) => { timers.delete(timer); });
  return {
    tick(milliseconds) {
      const end = now + milliseconds;
      for (;;) {
        const next = [...timers].filter(([, timer]) => timer.due <= end)
          .sort(([, left], [, right]) => left.due - right.due)[0];
        if (!next) break;
        const [id, timer] = next;
        now = timer.due;
        timers.delete(id);
        timer.callback();
      }
      now = end;
    },
  };
}
const clock = createFadeClock();
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
    // No span animates an invisible fragment: every fade span carries real
    // (non-whitespace-only) text, whitespace always riding with a neighbor.
    for (const span of container.querySelectorAll('.stream-fade-seg')) {
      assert.ok(!/^\s*$/.test(span.textContent), 'whitespace-only segments must never stand alone');
    }
    // Every pass-created span carries the creation marker the completion
    // unwrap keys on (and foreign class-reuse does not).
    for (const span of container.querySelectorAll('.stream-fade-seg')) {
      assert.match(span.getAttribute('data-stream-fade') || '', /^\d+$/, 'pass-created spans must carry the creation marker');
    }

    // Same DOM twice: idempotent, committed === total, no extra spans.
    applyStreamFade(container, { streaming: true });
    assert.equal(wrappedText(container), ' of streaming');

    // Completion: final render replaces innerHTML (spans gone) and the
    // streaming=false pass forgets the container.
    container.innerHTML = '<p>hello world of streaming</p>';
    applyStreamFade(container, { streaming: false });
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0);

    // Completion with an UNCHANGED html string: React skips the
    // dangerouslySetInnerHTML write, so the streaming=false pass must reap the
    // leftover fade spans itself. Spans created within the last tick are still
    // animating, so they are deferred past the age floor instead of being
    // unwrapped mid-flight; aged spans unwrap synchronously.
    {
      const skipContainer = documentWithContainer('<p>grow</p>');
      applyStreamFade(skipContainer, { streaming: true }); // baseline
      skipContainer.innerHTML = '<p>grow tail</p>';
      applyStreamFade(skipContainer, { streaming: true });
      const freshCount = skipContainer.querySelectorAll('.stream-fade-seg').length;
      assert.ok(freshCount > 0, 'precondition: spans exist');
      // html string unchanged since the last tick — no React rewrite. The
      // spans are a few ms old: completion must not snap them to full opacity.
      applyStreamFade(skipContainer, { streaming: false });
      assert.equal(skipContainer.querySelectorAll('.stream-fade-seg').length, freshCount, 'young spans survive completion unwrapped-in-place');
      assert.equal(skipContainer.textContent, 'grow tail', 'deferral preserves text');
      // Preserve the span until the exact age floor, then run its real callback.
      clock.tick(199);
      assert.equal(skipContainer.querySelectorAll('.stream-fade-seg').length, freshCount,
        'deferred reap must not unwrap before the age floor');
      clock.tick(1);
      assert.equal(skipContainer.querySelectorAll('.stream-fade-seg').length, 0, 'deferred reap unwraps spans past the age floor');
      assert.equal(skipContainer.textContent, 'grow tail', 'deferred reap preserves text');
    }
    {
      // Same completion, but the stream visibly paused before ending: aged
      // spans are past the floor and unwrap synchronously.
      const agedContainer = documentWithContainer('<p>grow</p>');
      applyStreamFade(agedContainer, { streaming: true }); // baseline
      agedContainer.innerHTML = '<p>grow tail</p>';
      applyStreamFade(agedContainer, { streaming: true });
      ageFadeSpans(agedContainer);
      applyStreamFade(agedContainer, { streaming: false });
      assert.equal(agedContainer.querySelectorAll('.stream-fade-seg').length, 0, 'aged spans unwrap immediately on completion');
      assert.equal(agedContainer.textContent, 'grow tail', 'unwrap preserves text');
    }
    {
      // A stamp that can never age — wall clock stepped back, or a forged
      // future marker — must terminate: after UNWRAP_MAX_DEFERRALS deferred
      // passes the reap force-unwraps instead of re-arming forever.
      const stuckContainer = documentWithContainer('<p>grow</p>');
      applyStreamFade(stuckContainer, { streaming: true }); // baseline
      stuckContainer.innerHTML = '<p>grow tail</p>';
      applyStreamFade(stuckContainer, { streaming: true });
      for (const span of stuckContainer.querySelectorAll('.stream-fade-seg')) {
        span.setAttribute('data-stream-fade', String(nowMs() + 3_600_000));
      }
      applyStreamFade(stuckContainer, { streaming: false });
      assert.ok(stuckContainer.querySelectorAll('.stream-fade-seg').length > 0, 'precondition: never-aging spans survive the first completion');
      clock.tick(200);
      assert.ok(stuckContainer.querySelectorAll('.stream-fade-seg').length > 0,
        'the first deferred pass must preserve never-aging spans');
      clock.tick(200);
      assert.equal(stuckContainer.querySelectorAll('.stream-fade-seg').length, 0, 'never-aging stamps are force-reaped, not looped');
      assert.equal(stuckContainer.textContent, 'grow tail', 'force reap preserves text');
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

  // A bare <pre> without a <code> child is skipped exactly like fenced code,
  // and inline <code> is skipped too (backtick-open/close transitions must not
  // re-fade or mis-offset the converted text).
  {
    const container = documentWithContainer('<pre>plain</pre><p>a <code>tok</code> b</p>');
    applyStreamFade(container, { streaming: true }); // baseline counts 'a  b' only
    container.innerHTML = '<pre>plain grown</pre><p>a <code>tok grown</code> b tail</p>';
    applyStreamFade(container, { streaming: true });
    assert.equal(container.querySelector('pre').textContent, 'plain grown', 'bare pre untouched');
    assert.equal(container.querySelectorAll('pre .stream-fade-seg').length, 0, 'nothing inside a bare pre fades');
    assert.equal(container.querySelectorAll('code .stream-fade-seg').length, 0, 'nothing inside inline code fades');
    assert.equal(wrappedText(container), ' tail', 'code growth must not shift the offset nor fade');
  }

  // Exact stagger values: 12ms per segment capped at 48ms. CJK fades one
  // ideograph per segment, so a six-character region pins the step AND the
  // cap (segments 5.. share the 48ms ceiling).
  {
    const container = documentWithContainer('<p>引</p>');
    applyStreamFade(container, { streaming: true }); // baseline commits 引
    container.innerHTML = '<p>引一二三四五六</p>';
    applyStreamFade(container, { streaming: true });
    assert.deepEqual(spanDelays(container), [0, 12, 24, 36, 48, 48], 'stagger step 12ms capped at 48ms');
  }

  // Model-emitted markup that merely reuses the class is not ours to remove:
  // completion strips the animation hook class so the content renders at once,
  // but the element, its own style, and its text are preserved. Pass-created
  // spans (with the creation marker) still unwrap fully.
  {
    const container = documentWithContainer('<p>seen <span class="stream-fade-seg" style="color:red">kept</span></p>');
    applyStreamFade(container, { streaming: true }); // baseline
    container.innerHTML = '<p>seen <span class="stream-fade-seg" style="color:red">kept</span> tail</p>';
    applyStreamFade(container, { streaming: true });
    assert.ok(container.querySelectorAll('.stream-fade-seg').length >= 2, 'precondition: foreign + fresh spans coexist');
    ageFadeSpans(container); // only affects marked spans; the foreign span stays young-looking forever
    applyStreamFade(container, { streaming: false });
    const foreign = container.querySelector('span[style="color:red"]');
    assert.ok(foreign, 'foreign element preserved');
    assert.equal(foreign.textContent, 'kept', 'foreign text preserved');
    assert.ok(!/(^|\s)stream-fade-seg(\s|$)/.test(foreign.getAttribute('class') || ''), 'fade class stripped from foreign element');
    assert.equal(foreign.getAttribute('style'), 'color:red', 'foreign own style untouched');
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 'pass-created spans unwrapped');
    assert.equal(container.textContent, 'seen kept tail', 'all text preserved');
  }

  // A forged creation marker must not upgrade model markup to pass-created:
  // DOMPurify passes data-* attributes through, so ownership keys on the
  // pass-created WeakSet instead — completion still neutralizes (element and
  // text preserved, animation hook stripped) and never unwraps it.
  {
    const container = documentWithContainer('<p>seen <span class="stream-fade-seg" data-stream-fade="123" style="color:blue">fake</span></p>');
    applyStreamFade(container, { streaming: true }); // baseline
    container.innerHTML = '<p>seen <span class="stream-fade-seg" data-stream-fade="123" style="color:blue">fake</span> tail</p>';
    applyStreamFade(container, { streaming: true });
    applyStreamFade(container, { streaming: false });
    const fake = container.querySelector('span[style="color:blue"]');
    assert.ok(fake, 'forged-marker element preserved');
    assert.equal(fake.textContent, 'fake', 'forged-marker text preserved');
    assert.ok(!/(^|\s)stream-fade-seg(\s|$)/.test(fake.getAttribute('class') || ''), 'forged marker still stripped of the fade class');
    assert.equal(fake.getAttribute('style'), 'color:blue', 'forged-marker own style untouched');
  }

  // Unchanged html since the last pass ⇒ the walk is skipped wholesale: DOM
  // changes React did not make are not observed. A changed html string (React
  // rewrote the container) resumes measuring.
  {
    const container = documentWithContainer('<p>hello</p>');
    applyStreamFade(container, { streaming: true, html: '<p>hello</p>' }); // baseline records lastHtml
    container.querySelector('p').appendChild(container.ownerDocument.createTextNode(' sneaky'));
    applyStreamFade(container, { streaming: true, html: '<p>hello</p>' }); // skipped
    assert.equal(container.querySelectorAll('.stream-fade-seg').length, 0, 'unchanged html skips the walk');
    applyStreamFade(container, { streaming: true, html: '<p>hello sneaky</p>' }); // changed: walk resumes
    assert.ok(container.querySelectorAll('.stream-fade-seg').length > 0, 'changed html resumes wrapping');
    assert.equal(container.textContent, 'hello sneaky', 'resume preserves text');
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
  mock.restoreAll();
  // Leave the module default (enabled) for any later blocks/tests.
  setStreamFadePreference(true);
}

console.log('stream_fade: ok');
