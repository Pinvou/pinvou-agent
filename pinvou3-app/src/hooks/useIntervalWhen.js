import { useEffect, useRef } from 'react';

// Condition-gated poll interval: the shared skeleton behind the ~10 components
// that each hand-rolled "no timer while disabled, cleaned up on disable and
// unmount" setInterval effects (promoted per the 2026-10 reuse audit, #817 B3).
// Deliberately narrow — per-site guards (document.visibilityState checks,
// staleness logic, immediate-first-tick syncs) stay with their components; this
// only owns the timer lifecycle. The callback is read through a ref re-synced
// after every commit, so polling closures that read fresh state do not need to
// (and must not be relied on to) reset the timer: the interval keeps its
// cadence across re-renders instead of restarting whenever a closure dep
// changes. A null/undefined delay disables the timer like `enabled: false`, as
// does any non-positive or non-finite cadence (browsers coerce 0/NaN to a ~4ms
// hot interval, so a degenerate delay must not reach setInterval).
/**
 * @param {() => void} callback - invoked every `delayMs` while `enabled`; always sees the latest closure
 * @param {number | null | undefined} delayMs - poll cadence in ms; null/undefined/0/NaN/negative disables the timer
 * @param {boolean} enabled - false creates no timer (and disabling clears the running one)
 */
export function useIntervalWhen(callback, delayMs, enabled) {
  const callbackRef = useRef(callback);
  useEffect(() => {
    callbackRef.current = callback;
  });
  useEffect(() => {
    if (!enabled || !delayMs || delayMs < 0) return;
    const timer = window.setInterval(() => callbackRef.current(), delayMs);
    return () => window.clearInterval(timer);
  }, [enabled, delayMs]);
}
