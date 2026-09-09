import { useEffect } from 'react';

// Poll cadences for the persisted-transcript (ledger) fallback read. ACTIVE
// tracks in-flight children; IDLE is the always-on authoritative heartbeat:
// real-time spawn/progress events can be lost, so the only reliable way to
// discover a child that started after the last read is to keep reading the
// ledger even when nothing is known to be running (see the overlay component).
export const LEDGER_POLL_ACTIVE_MS = 3000;
export const LEDGER_POLL_IDLE_MS = 15000;

/**
 * Repeatedly read the authoritative subagent ledger for one session and hand
 * each batch of summaries to `onSummaries`.
 *
 * The loop never stops while mounted: with no active entries it only slows
 * down to LEDGER_POLL_IDLE_MS. Restarting the effect on `hasActive` flips is
 * declarative — a fresh generation re-reads immediately and adopts the matching
 * cadence. A single failed read (throw, or `null` instead of an array) is
 * transient: the loop keeps running and retries on the next tick.
 */
export function useSubagentLedgerPoll({ enabled, sessionId, hasActive, readLedger, onSummaries }) {
  useEffect(() => {
    if (!enabled || !sessionId) return;
    let stopped = false;
    let timer = null;
    const poll = async () => {
      timer = null;
      if (stopped) return;
      try {
        const summaries = await readLedger(sessionId);
        if (stopped) return;
        if (Array.isArray(summaries)) onSummaries(summaries);
      } catch {
        // A single failed read is not fatal; retry on the next tick.
      }
      if (!stopped) timer = setTimeout(poll, hasActive ? LEDGER_POLL_ACTIVE_MS : LEDGER_POLL_IDLE_MS);
    };
    timer = setTimeout(poll, 0);
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, [enabled, hasActive, onSummaries, readLedger, sessionId]);
}
