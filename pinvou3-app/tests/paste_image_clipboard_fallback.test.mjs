import assert from 'node:assert/strict';
import test from 'node:test';

import {
  collectClipboardImages,
  pasteEventNeedsClipboardFallback,
  pasteImageClipboardFallbackAvailable,
} from '../src/features/attachments/paste-image.js';

function pasteEvent(clipboardData) {
  return { clipboardData, preventDefault: () => { pasteEvent.preventDefaultCalls += 1; } };
}

function item(type) {
  return {
    type,
    getAsFile: () => ({ name: 'pasted.png', type, size: 8 }),
  };
}

test('paste events that carry data keep the default path (no fallback)', () => {
  // Windows/macOS WebViews list the image among items — primary path handles it.
  const imageEvent = pasteEvent({ types: ['image/png'], items: [item('image/png')] });
  assert.equal(pasteEventNeedsClipboardFallback(imageEvent), false);
  assert.equal(collectClipboardImages(imageEvent).length, 1);

  // Plain text exposes text/plain — default native paste must stay untouched.
  assert.equal(
    pasteEventNeedsClipboardFallback(pasteEvent({ types: ['text/plain'], items: [item('text/plain')] })),
    false,
  );

  // Mixed image+text on WebView2/WKWebView: items carry both — image primary path.
  assert.equal(
    pasteEventNeedsClipboardFallback(pasteEvent({ types: ['text/plain', 'image/png'], items: [item('text/plain'), item('image/png')] })),
    false,
  );

  // No clipboardData at all (defensive) — nothing to fall back for.
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent(null)), false);
});

test('WebKitGTK image pastes surface an empty clipboardData and need the native fallback', () => {
  // Verified against WebKitGTK 2.52.6 (GTK clipboard set_image, incl. a
  // cross-process clipboard owner): image-only clipboards produce an empty
  // clipboardData — no items, empty types.
  const imageOnlyPaste = { types: [], items: [] };
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent(imageOnlyPaste)), true);
  // Text is swallowed too on image+text clipboards — the mixed clipboard still
  // surfaces as the same empty shape (observed in the same experiment).
  const mixedPaste = { types: [], items: [] };
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent(mixedPaste)), true);
  // Defensive: a types-less clipboardData still routes to the fallback probe
  // (the native command answers "no image" → no-op).
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent({ items: [] })), true);
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent({})), true);
  // Non-text non-image flavors (e.g. a lone text/html offer) are NOT the
  // WebKitGTK image signature — the default path must stay untouched.
  assert.equal(pasteEventNeedsClipboardFallback(pasteEvent({ types: ['text/html'], items: [] })), false);
});

test('clipboard fallback availability requires a loaded Linux capability flag', () => {
  assert.equal(pasteImageClipboardFallbackAvailable(null), false);
  assert.equal(pasteImageClipboardFallbackAvailable({}), false);
  // Loaded but not a Linux build — the paste event there already carries images.
  assert.equal(pasteImageClipboardFallbackAvailable({ loaded: true, pasteImageClipboardRead: false }), false);
  // Still loading — keep the previous "nothing pasted" behavior instead of a
  // command roundtrip that cannot help.
  assert.equal(pasteImageClipboardFallbackAvailable({ loaded: false, pasteImageClipboardRead: true }), false);
  assert.equal(pasteImageClipboardFallbackAvailable({ loaded: true, pasteImageClipboardRead: true }), true);
});
