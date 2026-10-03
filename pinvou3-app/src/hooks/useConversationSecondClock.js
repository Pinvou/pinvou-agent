import { useEffect, useState } from 'react';

// The 1s clock is scoped to the smallest display subtree: the per-second tick used to live on
// ChatView top-level state, re-rendering the whole transcript every second while busy; now only
// the component showing elapsed time owns the tick. On mount/activation it first syncs a baseline,
// then starts the interval (same semantics as the old ChatView/CodexAcpView top-level ticker); no
// timer is created while inactive, and it is cleaned up on unmount.
// Promoted out of features/conversation/ConversationTimeline.jsx: the timeline remains the
// canonical consumer, but artifacts/monitor/scheduled/chat tickers were importing the hook
// across the feature boundary.
/**
 * @param {boolean} active - whether the displayed duration is currently advancing
 * @returns {number} a timestamp that advances once per second while active
 */
export function useConversationSecondClock(active) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- sync the clock baseline once on activation so elapsed time is correct immediately, before the first interval tick
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  return now;
}
