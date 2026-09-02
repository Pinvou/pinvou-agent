#!/usr/bin/env node
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const temp = mkdtempSync(path.join(tmpdir(), 'pinvou3-conversation-scroll-'));
const conversationDir = path.join(temp, 'features', 'conversation');
mkdirSync(conversationDir, { recursive: true });
writeFileSync(path.join(temp, 'package.json'), '{"type":"module"}\n');
for (const file of ['conversation-model.js', 'conversation-scroll.js']) {
  copyFileSync(
    path.join(here, '..', 'src', 'features', 'conversation', file),
    path.join(conversationDir, file),
  );
}
const {
  measureConversationScrollGeometry,
  shouldForceScrollFollow,
  startConversationBottomFollower,
  transitionConversationScrollState,
} = await import(
  `${pathToFileURL(path.join(conversationDir, 'conversation-scroll.js')).href}?t=${Date.now()}`
);
const { isShrinkClampedToBottom } = await import(
  `${pathToFileURL(path.join(conversationDir, 'conversation-model.js')).href}?t=${Date.now()}`
);

function eventTarget() {
  const listeners = new Map();
  return {
    addEventListener(name, listener) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
    },
    removeEventListener(name, listener) {
      listeners.get(name)?.delete(listener);
    },
    dispatch(name) {
      for (const listener of listeners.get(name) || []) listener();
    },
    listenerCount(name) {
      return listeners.get(name)?.size || 0;
    },
  };
}

const windowEvents = eventTarget();
const documentEvents = eventTarget();
const frames = new Map();
let nextFrame = 1;
const observers = [];
const fakeWindow = {
  ...windowEvents,
  requestAnimationFrame(callback) {
    const id = nextFrame++;
    frames.set(id, callback);
    return id;
  },
  cancelAnimationFrame(id) {
    frames.delete(id);
  },
  ResizeObserver: class {
    constructor(callback) {
      this.callback = callback;
      this.observeCalls = [];
      this.observedElements = new Set();
      this.disconnected = false;
      observers.push(this);
    }
    observe(element) {
      this.observeCalls.push(element);
      this.observedElements.add(element);
    }
    trigger(element) {
      if (this.observedElements.has(element)) this.callback([{ target: element }], this);
    }
    disconnect() {
      this.disconnected = true;
      this.observedElements.clear();
    }
  },
};
const fakeDocument = { ...documentEvents, visibilityState: 'visible' };
const scrollElement = { scrollHeight: 1000, scrollTop: 700, clientHeight: 200 };
const contentElement = {};
let following = true;
const restored = [];
const flushFrame = () => {
  const pending = [...frames.entries()];
  frames.clear();
  for (const [, callback] of pending) callback();
};

try {
  const stop = startConversationBottomFollower({
    scrollElement,
    contentElement,
    isFollowing: () => following,
    onRestored: (scrollTop) => restored.push(scrollTop),
    windowObject: fakeWindow,
    documentObject: fakeDocument,
  });
  const observer = observers[0];
  assert.deepEqual(observer.observeCalls, [scrollElement, contentElement],
    'the bottom follower must observe both the viewport and its content');

  flushFrame();
  assert.equal(scrollElement.scrollTop, 800, 'initial session activation must settle at the bottom');

  scrollElement.clientHeight = 300;
  observer.trigger(scrollElement);
  flushFrame();
  assert.equal(scrollElement.scrollTop, 700,
    'a clientHeight-only viewport resize must keep a following conversation at the bottom');

  scrollElement.scrollHeight = 1400;
  observer.trigger(contentElement);
  flushFrame();
  assert.equal(scrollElement.scrollTop, 1100, 'content reflow must keep a following conversation at the bottom');

  scrollElement.scrollHeight = 1600;
  fakeWindow.dispatch('focus');
  flushFrame();
  assert.equal(scrollElement.scrollTop, 1300, 'window focus must restore the bottom after background layout changes');

  scrollElement.scrollHeight = 1800;
  fakeDocument.visibilityState = 'hidden';
  fakeDocument.dispatch('visibilitychange');
  flushFrame();
  assert.equal(scrollElement.scrollTop, 1300, 'hidden visibility changes must not move the conversation');
  fakeDocument.visibilityState = 'visible';
  fakeDocument.dispatch('visibilitychange');
  flushFrame();
  assert.equal(scrollElement.scrollTop, 1500, 'visible conversations must resume bottom following');

  following = false;
  scrollElement.scrollTop = 900;
  scrollElement.scrollHeight = 2000;
  scrollElement.clientHeight = 400;
  observer.trigger(scrollElement);
  observer.trigger(contentElement);
  fakeWindow.dispatch('focus');
  flushFrame();
  assert.equal(scrollElement.scrollTop, 900,
    'a user browsing history must not be pulled back after viewport or content resize');

  following = true;
  scrollElement.scrollHeight = 2200;
  fakeWindow.dispatch('focus');
  stop();
  flushFrame();
  assert.equal(scrollElement.scrollTop, 900, 'cleanup must cancel a pending bottom restoration');
  assert.equal(observer.disconnected, true);
  assert.equal(observer.observedElements.size, 0);
  assert.equal(fakeWindow.listenerCount('focus'), 0);
  assert.equal(fakeDocument.listenerCount('visibilitychange'), 0);
  assert.deepEqual(restored, [800, 700, 1100, 1300, 1500]);

  const sharedElement = { scrollHeight: 1000, scrollTop: 800, clientHeight: 200 };
  const stopShared = startConversationBottomFollower({
    scrollElement: sharedElement,
    contentElement: sharedElement,
    isFollowing: () => true,
    windowObject: fakeWindow,
    documentObject: fakeDocument,
  });
  const sharedObserver = observers[1];
  assert.deepEqual(sharedObserver.observeCalls, [sharedElement],
    'one element serving as viewport and content must only be observed once');
  stopShared();
  flushFrame();
  assert.equal(sharedObserver.disconnected, true, 'cleanup must disconnect the shared-element observer');

  // A content shrink clamps scrollTop down to the new maximum; that scroll event must be
  // recognizable as programmatic so it neither disables nor re-enables auto-follow.
  const shrinkClamped = { scrollHeight: 900, scrollTop: 700, clientHeight: 200 };
  assert.equal(isShrinkClampedToBottom(shrinkClamped, 1000), true,
    'a shrink-induced clamp to the new bottom must be detected');
  assert.equal(isShrinkClampedToBottom({ ...shrinkClamped, scrollTop: 300 }, 1000), false,
    'a shrink with the viewport far above the bottom is user scrolling, not a clamp');
  assert.equal(isShrinkClampedToBottom({ scrollHeight: 1000, scrollTop: 800, clientHeight: 200 }, 1000), false,
    'no shrink means any upward move is user scrolling');
  assert.equal(isShrinkClampedToBottom({ scrollHeight: 1200, scrollTop: 1000, clientHeight: 200 }, 1000), false,
    'content growth must never be treated as a shrink clamp');
  assert.equal(isShrinkClampedToBottom(null, 1000), false,
    'a missing element must not be reported as clamped');

  // Exercise the state transition shared by Chat and Codex: the user first leaves
  // the bottom, then content shrinks without a scroll event, and finally the user
  // deliberately returns to the new bottom. The resize measurement must replace the
  // stale height before that last scroll so future output resumes bottom following.
  const recoveryElement = { scrollHeight: 1200, scrollTop: 1000, clientHeight: 200 };
  let recoveryState = { following: true, scrollTop: 1000, scrollHeight: 1200 };
  const applyRecoveryScroll = () => {
    const transition = transitionConversationScrollState({
      scrollElement: recoveryElement,
      following: recoveryState.following,
      previousScrollTop: recoveryState.scrollTop,
      previousScrollHeight: recoveryState.scrollHeight,
    });
    recoveryState = {
      following: transition.following,
      scrollTop: transition.scrollTop,
      scrollHeight: transition.scrollHeight,
    };
  };
  const stopRecovery = startConversationBottomFollower({
    scrollElement: recoveryElement,
    contentElement: recoveryElement,
    isFollowing: () => recoveryState.following,
    onMeasured: () => {
      const measurement = measureConversationScrollGeometry({
        scrollElement: recoveryElement,
        following: recoveryState.following,
        previousScrollTop: recoveryState.scrollTop,
        previousScrollHeight: recoveryState.scrollHeight,
      });
      recoveryState.scrollTop = measurement.scrollTop;
      recoveryState.scrollHeight = measurement.scrollHeight;
    },
    onRestored: (scrollTop) => {
      recoveryState.scrollTop = scrollTop;
      recoveryState.scrollHeight = recoveryElement.scrollHeight;
    },
    windowObject: fakeWindow,
    documentObject: fakeDocument,
  });
  const recoveryObserver = observers[2];
  flushFrame();

  recoveryElement.scrollTop = 600;
  applyRecoveryScroll();
  assert.equal(recoveryState.following, false, 'scrolling up must pause bottom following');

  recoveryElement.scrollHeight = 1000;
  recoveryObserver.trigger(recoveryElement);
  flushFrame();
  assert.deepEqual(recoveryState, { following: false, scrollTop: 600, scrollHeight: 1000 },
    'a resize without a scroll event must refresh geometry without pulling history browsing to the bottom');

  recoveryElement.scrollTop = 800;
  applyRecoveryScroll();
  assert.equal(recoveryState.following, true,
    'a deliberate downward scroll to the new bottom must resume following after content shrink');

  recoveryElement.scrollHeight = 1300;
  recoveryObserver.trigger(recoveryElement);
  flushFrame();
  assert.equal(recoveryElement.scrollTop, 1100,
    'later content growth must remain pinned after the user resumes following');
  stopRecovery();

  const clampedMeasurement = measureConversationScrollGeometry({
    scrollElement: shrinkClamped,
    following: false,
    previousScrollTop: 800,
    previousScrollHeight: 1000,
  });
  assert.deepEqual(clampedMeasurement, { scrollTop: 800, scrollHeight: 1000 },
    'resize measurement must preserve a shrink-clamp baseline until its possible scroll event arrives');
  const clampedTransition = transitionConversationScrollState({
    scrollElement: shrinkClamped,
    following: false,
    previousScrollTop: clampedMeasurement.scrollTop,
    previousScrollHeight: clampedMeasurement.scrollHeight,
  });
  assert.equal(clampedTransition.following, false,
    'a delayed shrink-clamp scroll event must not resume following for a history reader');
  const followedShrink = transitionConversationScrollState({
    scrollElement: shrinkClamped,
    following: true,
    previousScrollTop: clampedMeasurement.scrollTop,
    previousScrollHeight: clampedMeasurement.scrollHeight,
  });
  assert.equal(followedShrink.following, true,
    'a shrink-clamp scroll event while following must keep pinning instead of pausing the follower');

  // r13 review M1: a mid-turn steered bubble parks a user item LAST while the
  // turn's streaming output above it keeps changing the follow traits — the
  // snap must fire once per appended item, not on every delta, or it would
  // overwrite the scroll listener's "user scrolled up" state for the rest of
  // the turn.
  assert.equal(shouldForceScrollFollow({
    following: true, lastItemType: 'user', itemCount: 5, lastSnapItemCount: 5,
  }), true, 'following the stream must keep forcing the bottom');
  assert.equal(shouldForceScrollFollow({
    following: false, lastItemType: 'assistant', itemCount: 5, lastSnapItemCount: 4,
  }), false, 'a non-user last item must never force the snap while history reading');
  assert.equal(shouldForceScrollFollow({
    following: false, lastItemType: 'user', itemCount: 6, lastSnapItemCount: 5,
  }), true, 'a newly appended user item must snap once even while history reading');
  assert.equal(shouldForceScrollFollow({
    following: false, lastItemType: 'user', itemCount: 6, lastSnapItemCount: 6,
  }), false, 'the same parked user bubble must not re-snap on streaming deltas above it');

  console.log('conversation_scroll_follow: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
