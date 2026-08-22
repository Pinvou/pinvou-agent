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
const { startConversationBottomFollower } = await import(
  `${pathToFileURL(path.join(conversationDir, 'conversation-scroll.js')).href}?t=${Date.now()}`
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

  console.log('conversation_scroll_follow: ok');
} finally {
  rmSync(temp, { recursive: true, force: true });
}
