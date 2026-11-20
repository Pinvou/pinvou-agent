import { useEffect, useRef } from 'react';

// Condition-gated poll interval (#817 B3): the shared skeleton for the
// hand-rolled "no timer while disabled, cleaned up on disable and unmount"
// setInterval effects. Four sites consume it (ProvidersSection install,
// CodexWorkspacePanel workspace, KnowledgeView index, ScheduledTasksView
// refresh); the other seven poll components keep bespoke effects for ref-managed
// timers, staleness guards, or deliberate reset semantics — do not force-fit.
// Deliberately narrow — per-site guards (document.visibilityState checks,
// staleness logic, immediate-first-tick syncs) stay with their components; this
// only owns the timer lifecycle. The callback is read through a ref re-synced
// after every commit, so polling closures that read fresh state do not need to
// (and must not be relied on to) reset the timer: the interval keeps its
// cadence across re-renders instead of restarting whenever a closure dep
// changes. A null/undefined delay disables the timer like `enabled: false`, as
// does any non-positive or non-finite cadence (browsers coerce 0/NaN/+Infinity
// to a ~4ms hot interval, so a degenerate delay must not reach setInterval).
/**
 * @param {() => void} callback - invoked every `delayMs` while `enabled`; always sees the latest closure
 * @param {number | null | undefined} delayMs - poll cadence in ms; must be a finite positive number — null/undefined/0/NaN/±Infinity/negative all disable the timer
 * @param {boolean} enabled - false creates no timer (and disabling clears the running one)
 */
export function useIntervalWhen(callback, delayMs, enabled) {
  const callbackRef = useRef(callback);
  useEffect(() => {
    callbackRef.current = callback;
  });
  useEffect(() => {
    if (!enabled || !Number.isFinite(delayMs) || delayMs <= 0) return;
    const timer = window.setInterval(() => callbackRef.current(), delayMs);
    return () => window.clearInterval(timer);
  }, [enabled, delayMs]);
}
