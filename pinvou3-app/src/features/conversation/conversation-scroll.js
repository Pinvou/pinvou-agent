import { useEffect } from 'react';
import {
  isNearConversationBottom,
  isShrinkClampedToBottom,
  restoreConversationScrollPosition,
} from './conversation-model.js';

// Consumers (ChatView / CodexAcpView onScroll handlers) read only the
// follow-state and the refreshed geometry; the intermediate movingUp /
// nearBottom / shrinkClamped signals stay internal to the transition.
export function transitionConversationScrollState({
  scrollElement,
  following,
  previousScrollTop,
  previousScrollHeight,
}) {
  if (!scrollElement) {
    return {
      following,
      scrollTop: Number(previousScrollTop) || 0,
      scrollHeight: Number(previousScrollHeight) || 0,
    };
  }

  const nearBottom = isNearConversationBottom(scrollElement);
  const shrinkClamped = isShrinkClampedToBottom(scrollElement, previousScrollHeight);
  const movingUp = scrollElement.scrollTop < (Number(previousScrollTop) || 0) - 1
    && !shrinkClamped;
  return {
    following: movingUp ? false : ((!shrinkClamped && nearBottom) || following),
    scrollTop: scrollElement.scrollTop,
    scrollHeight: scrollElement.scrollHeight,
  };
}

// Whether the chat auto-follow effect must force the container back to the
// bottom on this run. Following the stream is the normal case. A user-typed
// or mid-turn steered message parks a user bubble as the LAST item while the
// turn's output above it keeps changing the effect's follow traits; the snap
// for that bubble must fire once per appended item (count change) only, or
// every streaming delta would re-force the bottom and overwrite the scroll
// listener's "user scrolled up" state for the rest of the turn.
export function shouldForceScrollFollow({
  following,
  lastItemType,
  itemCount,
  lastSnapItemCount,
}) {
  if (following) return true;
  return lastItemType === 'user' && itemCount !== lastSnapItemCount;
}

export function measureConversationScrollGeometry({
  scrollElement,
  following,
  previousScrollTop,
  previousScrollHeight,
}) {
  const previous = {
    scrollTop: Number(previousScrollTop) || 0,
    scrollHeight: Number(previousScrollHeight) || 0,
  };
  if (!scrollElement) return previous;

  // Preserve the pre-shrink baseline when the browser has clamped a history
  // reader to the new bottom. A scroll event may still arrive after ResizeObserver;
  // retaining the old height lets the transition identify that event as layout-owned
  // instead of incorrectly resuming follow mode.
  if (!following && isShrinkClampedToBottom(scrollElement, previous.scrollHeight)) {
    return previous;
  }
  return {
    scrollTop: scrollElement.scrollTop,
    scrollHeight: scrollElement.scrollHeight,
  };
}

export function startConversationBottomFollower({
  scrollElement,
  contentElement,
  isFollowing,
  onMeasured,
  onRestored,
  windowObject = typeof window === 'undefined' ? null : window,
  documentObject = typeof document === 'undefined' ? null : document,
}) {
  if (!scrollElement || !contentElement || !windowObject || !documentObject) return () => {};

  let frame = null;
  const restoreBottomIfFollowing = () => {
    // ResizeObserver runs after layout has updated scrollTop/scrollHeight. Record that
    // baseline even while history browsing, so the next user scroll is not compared
    // with stale pre-reflow geometry and mistaken for a shrink-induced clamp.
    if (onMeasured) onMeasured(scrollElement);
    if (!isFollowing()) return;
    if (frame !== null) windowObject.cancelAnimationFrame(frame);
    frame = windowObject.requestAnimationFrame(() => {
      frame = null;
      if (!isFollowing()) return;
      restoreConversationScrollPosition(scrollElement, { stickToBottom: true, bottomGap: 0 });
      if (onRestored) onRestored(scrollElement.scrollTop);
    });
  };
  const onVisibilityChange = () => {
    if (documentObject.visibilityState === 'visible') restoreBottomIfFollowing();
  };
  const observer = windowObject.ResizeObserver
    ? new windowObject.ResizeObserver(restoreBottomIfFollowing)
    : null;

  if (observer) {
    observer.observe(scrollElement);
    if (contentElement !== scrollElement) observer.observe(contentElement);
  }
  windowObject.addEventListener('focus', restoreBottomIfFollowing);
  documentObject.addEventListener('visibilitychange', onVisibilityChange);
  restoreBottomIfFollowing();

  return () => {
    if (observer) observer.disconnect();
    windowObject.removeEventListener('focus', restoreBottomIfFollowing);
    documentObject.removeEventListener('visibilitychange', onVisibilityChange);
    if (frame !== null) windowObject.cancelAnimationFrame(frame);
  };
}

// React wiring around startConversationBottomFollower (ChatView's scroll
// effect, extracted so other hosts can reuse it). All refs/setters are
// caller-owned so the hook stays a thin effect wrapper; re-runs follow the
// ChatView dep-array semantics: the session identity plus whether the
// conversation content is mounted (contentElement only exists once messages
// render, so a bare session id would miss the first content attach).
export function useConversationBottomFollower({
  scrollRef,
  contentRef,
  autoScrollRef,
  lastScrollTopRef,
  lastScrollHeightRef,
  setShowScrollBottom,
  activeSessionId,
  hasMessages,
}) {
  useEffect(() => {
    const scrollElement = scrollRef.current;
    const contentElement = contentRef.current;
    if (!scrollElement || !contentElement) return;
    return startConversationBottomFollower({
      scrollElement,
      contentElement,
      isFollowing: () => autoScrollRef.current,
      onMeasured: () => {
        const measurement = measureConversationScrollGeometry({
          scrollElement,
          following: autoScrollRef.current,
          previousScrollTop: lastScrollTopRef.current,
          previousScrollHeight: lastScrollHeightRef.current,
        });
        lastScrollTopRef.current = measurement.scrollTop;
        lastScrollHeightRef.current = measurement.scrollHeight;
      },
      onRestored: (scrollTop) => {
        lastScrollTopRef.current = scrollTop;
        lastScrollHeightRef.current = scrollElement.scrollHeight;
        setShowScrollBottom(false);
      },
    });
  }, [activeSessionId, hasMessages, scrollRef, contentRef, autoScrollRef, lastScrollTopRef, lastScrollHeightRef, setShowScrollBottom]);
}
