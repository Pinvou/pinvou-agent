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
 *
 * `kickRef` (optional) receives a function that triggers an immediate read,
 * cancelling the pending tick. Callers use it when a real-time hint suggests
 * the ledger moved (e.g. a revival event the terminal ratchet rejected): an
 * authoritative read beats waiting for the next heartbeat. A kick during an
 * in-flight read queues exactly one re-read right after it: the in-flight
 * read was issued before the kick, so its snapshot may predate the very
 * change the kick is about — dropping the request could delay the correction
 * to the next heartbeat.
 */
export function useSubagentLedgerPoll({ enabled, sessionId, hasActive, readLedger, onSummaries, kickRef }) {
  useEffect(() => {
    if (!enabled || !sessionId) return;
    let stopped = false;
    let inFlight = false;
    let kickQueued = false;
    let timer = null;
    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      timer = null;
      try {
        const summaries = await readLedger(sessionId);
        if (stopped) return;
        if (Array.isArray(summaries)) onSummaries(summaries);
      } catch {
        // A single failed read is not fatal; retry on the next tick.
      } finally {
        inFlight = false;
      }
      if (kickQueued && !stopped) {
        // Deliver the queued kick: re-read immediately instead of waiting a
        // full cadence for a snapshot this already-finished read may predate.
        kickQueued = false;
        void poll();
        return;
      }
      if (!stopped) timer = setTimeout(poll, hasActive ? LEDGER_POLL_ACTIVE_MS : LEDGER_POLL_IDLE_MS);
    };
    timer = setTimeout(poll, 0);
    if (kickRef) {
      kickRef.current = () => {
        if (stopped) return;
        if (inFlight) {
          kickQueued = true;
          return;
        }
        if (timer != null) {
          clearTimeout(timer);
          timer = null;
        }
        void poll();
      };
    }
    return () => {
      stopped = true;
      if (timer != null) clearTimeout(timer);
      if (kickRef) kickRef.current = null;
    };
  }, [enabled, hasActive, kickRef, onSummaries, readLedger, sessionId]);
}
