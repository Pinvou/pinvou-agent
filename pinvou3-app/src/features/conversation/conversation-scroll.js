import { restoreConversationScrollPosition } from './conversation-model.js';

export function startConversationBottomFollower({
  scrollElement,
  contentElement,
  isFollowing,
  onRestored,
  windowObject = typeof window === 'undefined' ? null : window,
  documentObject = typeof document === 'undefined' ? null : document,
}) {
  if (!scrollElement || !contentElement || !windowObject || !documentObject) return () => {};

  let frame = null;
  const restoreBottomIfFollowing = () => {
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
