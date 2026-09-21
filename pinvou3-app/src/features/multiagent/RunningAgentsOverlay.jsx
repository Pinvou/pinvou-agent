import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import { can } from '../../shared/platform.js';
import {
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  isUnknownLedgerRow,
  mergeOverlayEntry,
  overlayVisibleEntries,
  pruneOverlayEntries,
  statusPresentation,
} from './overlay-model.mjs';
import { useSubagentLedgerPoll } from './useSubagentLedgerPoll.js';
import { dispatchOpenSubagent } from './subagent-panel-event.mjs';
import { roleKeyOf } from './subagent-conversation.mjs';

/**
 * Swarm running overlay (top-right, ADR-0006 swarm rework): shown while the
 * current session has non-terminal subagents. Collapsed it is a count pill;
 * expanded it lists every running subagent (name/role + status dot + short
 * status). Clicking an entry dispatches `pinvou:open-subagent` to open its
 * read-only transcript panel. The collapsed/expanded choice is persisted in
 * localStorage.
 *
 * Two state sources (the persisted projection is authoritative; real-time
 * events can be lost):
 * - `pinvou:subagent-update` (real-time progress/completion events forwarded
 *   by the bridge);
 * - this component's own `listSubagentTranscripts` ledger poll — 3s while
 *   something is non-terminal, and never fully stopped: with nothing active it
 *   keeps an idle authoritative read (15s), so a child that starts later whose
 *   real-time event was lost is still discovered (a poll that stopped on an
 *   empty ledger could never learn about it). The persisted snapshot serves
 *   only this component: after the spawn count row replaced inline expert
 *   cards, `pinvou:subagent-ledger-update` has no live consumer left on the
 *   chat lane (the remaining expert row subscribes to no events), so it is no
 *   longer broadcast.
 *
 * Border mood: swarm off = blue border, swarm on = purple border (one shade
 * per light/dark theme); running status dots breathe, and completed entries
 * hold a green success state briefly before fading out of the list.
 */

const COLLAPSE_STORAGE_KEY = 'pinvou3.swarmOverlay.collapsed';

const swarmBorder = on => (on
  ? 'border-[#7C3AED]/50 dark:border-[#A78BFA]/45'
  : 'border-[#0B57D0]/45 dark:border-[#A8C7FA]/40');

const dotClass = {
  running: 'bg-[#0B57D0] dark:bg-[#A8C7FA] animate-pulse',
  // Blocked is a done worker awaiting user follow-up (the foundation counts
  // it Completed), so it holds a steady amber dot instead of the breathing
  // one reserved for live work.
  blocked: 'bg-[#F9AB00]',
  done: 'bg-[#137333] dark:bg-[#93D5A6]',
  failed: 'bg-[#C5221F] dark:bg-[#F28B82]',
  // Cancelled/interrupted: an operator- or session-level ending, neither a
  // failure (red) nor a success (green).
  stopped: 'bg-[#9AA0A6]',
};

// One row renderer shared by the active and recent lists; `recent` switches
// the test id, the faded name, and the terminal status colors (a failed entry
// keeps its red, a stopped one its gray, instead of the neutral active gray).
function OverlayAgentRow({ entry, recent, isDark, copy, onOpen }) {
  // Recent entries carry their real terminal status: a failed agent must not
  // borrow the success styling of the window.
  const status = statusPresentation(entry, copy);
  return (
    <li>
      <button
        type="button"
        data-testid={recent ? 'running-agents-entry-done' : 'running-agents-entry'}
        onClick={() => onOpen(entry.agentId, entry.sessionId)}
        className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12.5px] transition-colors ${
          isDark ? 'hover:bg-white/[0.06]' : 'hover:bg-black/[0.04]'
        }`}
      >
        <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${dotClass[status.dot]}`} />
        <span className={`min-w-0 flex-1 truncate font-medium ${recent ? 'opacity-70' : ''}`}>
          {/* 与转录面板同一套角色本地化:内置别名折回 roleCards 文案;自定义角色
              原样展示,无 role 时回退 agentId。 */}
          {(copy.roleCards && copy.roleCards[roleKeyOf(entry.role, entry.agentType)]) || entry.role || entry.agentId}
        </span>
        <span
          className={recent
            ? `shrink-0 text-[10.5px] ${
              status.dot === 'failed'
                ? (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]')
                : status.dot === 'stopped'
                  ? 'text-[#9AA0A6]'
                  : (isDark ? 'text-[#93D5A6]' : 'text-[#137333]')
            }`
            : `shrink-0 truncate text-[10.5px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#757575]'}`}
        >
          {status.text}
        </span>
      </button>
    </li>
  );
}

export const RunningAgentsOverlay = ({ sessionId, theme, t, swarmOn = false }) => {
  const copy = t.uiMultiAgent;
  const isDark = theme === 'dark';
  const enabled = can('multiAgent') && bridge.available && !!bridge.multiAgent;
  const [entries, setEntries] = useState({});
  // Mirror of `entries` for synchronous merge decisions (the terminal ratchet
  // must reject or accept a real-time event without waiting for a state
  // update). Every write goes through commitEntries.
  const entriesRef = useRef({});
  const kickPollRef = useRef(null);
  const commitEntries = useCallback(next => {
    entriesRef.current = next;
    setEntries(next);
  }, []);
  const [expanded, setExpanded] = useState(() => {
    if (typeof localStorage === 'undefined') return false;
    try {
      return localStorage.getItem(COLLAPSE_STORAGE_KEY) !== '1';
    } catch {
      // Restricted storage environments (e.g. cookie-blocked WebViews) throw on
      // read: fall back to expanded, mirroring the setItem defense below.
      return true;
    }
  });

  const mergeEntry = useCallback((sessionIdIn, detail) => {
    if (!detail || !detail.agentId) return false;
    const key = entryKey(sessionIdIn, detail.agentId);
    const previous = entriesRef.current;
    const next = mergeOverlayEntry(previous[key], detail, sessionIdIn, Date.now());
    if (!next) return false;
    const merged = { ...previous, [key]: next };
    commitEntries(pruneOverlayEntries(merged) || merged);
    return true;
  }, [commitEntries]);

  // Real-time event subscription.
  useEffect(() => {
    if (!enabled || typeof window === 'undefined') return;
    const onUpdate = event => {
      const detail = event && event.detail;
      if (!detail || (sessionId && detail.sessionId && detail.sessionId !== sessionId)) return;
      const applied = mergeEntry(detail.sessionId || sessionId, detail);
      // Revival: a live non-terminal event rejected by the terminal ratchet
      // means the persisted state likely moved (a parent follow-up re-awakened
      // the agent). Kick an immediate authoritative read instead of waiting
      // for the next heartbeat — same pattern the retired expert card used.
      if (!applied && !detail.done && detail.source !== 'ledger' && kickPollRef.current) {
        kickPollRef.current();
      }
    };
    window.addEventListener('pinvou:subagent-update', onUpdate);
    return () => {
      window.removeEventListener('pinvou:subagent-update', onUpdate);
    };
  }, [enabled, mergeEntry, sessionId]);

  const sessionEntries = useMemo(
    () => Object.values(entries).filter(entry => entry.sessionId === sessionId),
    [entries, sessionId],
  );
  // Whether any entry in this session is truly unfinished: drives the ledger
  // poll cadence (see the hook). Blocked entries are done in the authority
  // chain, so a stale blocked row never locks the poll at the active cadence.
  const sessionHasActive = useMemo(
    () => sessionEntries.some(entry => !isTerminal(entry)),
    [sessionEntries],
  );

  const readLedger = useCallback(
    id => bridge.multiAgent.listSubagentTranscripts(id),
    [],
  );

  const mergeLedgerSummaries = useCallback(summaries => {
    // One commit per batch: merging each summary with its own setEntries would
    // shallow-clone the whole cache per entry (O(n²) per tick at swarm scale).
    const previous = entriesRef.current;
    let changed = false;
    const merged = { ...previous };
    for (const summary of summaries) {
      if (!summary || !summary.agent_id) continue;
      // Orphan transcripts (done=false, no status token) are records the
      // foundation pruned from its 256-entry worker ledger while keeping the
      // transcript files: historical leftovers, not live agents. Merging them
      // would pin eternal "working" ghosts into the overlay and lock the
      // poll at the active cadence (see isUnknownLedgerRow).
      if (isUnknownLedgerRow(summary)) continue;
      const key = entryKey(sessionId, summary.agent_id);
      const next = mergeOverlayEntry(previous[key], {
        sessionId,
        agentId: summary.agent_id,
        role: summary.role || null,
        status: summary.status || null,
        done: !!summary.done,
        failed: !!summary.failed,
        blocked: !!summary.blocked,
        source: 'ledger',
      }, sessionId, Date.now());
      if (!next) continue;
      merged[key] = next;
      changed = true;
    }
    if (!changed) return;
    commitEntries(pruneOverlayEntries(merged) || merged);
  }, [commitEntries, sessionId]);

  // Ledger fallback poll. The loop never stops while mounted: it reads the
  // authoritative persisted projection at 3s while something is non-terminal
  // and keeps an idle 15s heartbeat otherwise, so a child that appears later
  // with its real-time event lost is still discovered (this effect restarting
  // on `hasActive` alone cannot cover that case — nothing flips `hasActive`
  // until some source observes the child). Restarts on session switches and
  // `hasActive` flips are declarative: each generation re-reads immediately.
  useSubagentLedgerPoll({
    enabled,
    sessionId,
    hasActive: sessionHasActive,
    readLedger,
    onSummaries: mergeLedgerSummaries,
    kickRef: kickPollRef,
  });

  // Drop entries of other sessions on a session switch to avoid cross-talk.
  useEffect(() => {
    if (!sessionId) return;
    const previous = entriesRef.current;
    let next = null;
    for (const [key, entry] of Object.entries(previous)) {
      if (entry.sessionId === sessionId) continue;
      if (!next) next = { ...previous };
      delete next[key];
    }
    if (!next) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-time cleanup of the previous session's cache on sessionId change, unrelated to the render cascade
    commitEntries(next);
  }, [commitEntries, sessionId]);

  // Wake once when the success-state display window expires so finished
  // terminal entries fade out of the list.
  const [, setRecentTick] = useState(0);
  // eslint-disable-next-line react-hooks/purity -- the success-state window is judged on the real clock; recompute once when the tick fires
  const { active, recent } = overlayVisibleEntries(sessionEntries, Date.now());
  // The wake-up is armed on the earliest pending expiry, not on the window's
  // size: a completion landing near another entry's expiry can leave
  // recent.length unchanged, and a length-keyed timer would miss the re-arm.
  // When the earliest entry fades, the recomputed minimum moves forward and
  // re-runs this effect for the next entry. A primitive dep keeps the effect
  // from re-arming on unrelated re-renders.
  let nextRecentExpiry = Infinity;
  for (const entry of recent) {
    if (entry.completedAt != null && entry.completedAt < nextRecentExpiry) nextRecentExpiry = entry.completedAt;
  }
  useEffect(() => {
    if (!Number.isFinite(nextRecentExpiry)) return;
    // eslint-disable-next-line react-hooks/purity -- the delay is measured on the real clock, matching the render-phase window judgment above
    const delay = Math.max(0, nextRecentExpiry + RECENT_TERMINAL_MS + 100 - Date.now());
    const timer = setTimeout(() => setRecentTick(value => value + 1), delay);
    return () => clearTimeout(timer);
  }, [nextRecentExpiry]);
  const visible = enabled && sessionId && (active.length > 0 || recent.length > 0);

  const toggleExpanded = useCallback(() => {
    setExpanded(value => {
      const next = !value;
      try {
        localStorage.setItem(COLLAPSE_STORAGE_KEY, next ? '0' : '1');
      } catch {
        // Unwritable storage only loses the preference, not functionality.
      }
      return next;
    });
  }, []);

  const openAgent = useCallback((agentId, agentSessionId) => {
    dispatchOpenSubagent(agentId, agentSessionId || sessionId || null);
  }, [sessionId]);

  if (!visible) return null;

  const surface = `rounded-2xl border shadow-sm ${swarmBorder(swarmOn)} ${
    isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'
  }`;

  return (
    <div className="pointer-events-auto relative" data-testid="running-agents-overlay">
      <button
        type="button"
        data-testid="running-agents-toggle"
        aria-expanded={expanded}
        aria-label={expanded ? copy.runningAgentsCollapse : copy.runningAgentsExpand}
        title={copy.runningAgentsCount(active.length)}
        onClick={toggleExpanded}
        className={`flex h-10 max-w-[220px] shrink-0 items-center gap-2 rounded-full border px-3 text-[14px] font-medium shadow-sm transition-colors ${swarmBorder(swarmOn)} ${
          isDark
            ? 'bg-[#1E1F20] text-[#E3E3E3] hover:bg-[#333537]'
            : 'bg-white text-[#1F1F1F] hover:bg-[#F0F4F9]'
        }`}
      >
        <span aria-hidden="true" className={`text-[13px] leading-none ${swarmOn ? 'text-[#7C3AED] dark:text-[#A78BFA]' : 'text-[#0B57D0] dark:text-[#A8C7FA]'}`}>✦</span>
        <span className="max-sm:hidden truncate">{copy.runningAgentsTitle}</span>
        <span className={`flex h-5 min-w-5 shrink-0 items-center justify-center rounded-full px-1.5 text-[11px] tabular-nums ${
          active.length > 0
            ? (swarmOn ? 'bg-[#7C3AED] text-white dark:bg-[#A78BFA] dark:text-[#2E1065]' : 'bg-[#0B57D0] text-white dark:bg-[#A8C7FA] dark:text-[#062E6F]')
            : (isDark ? 'bg-white/10 text-[#E3E3E3]' : 'bg-black/[0.06] text-[#1F1F1F]')
        }`}>
          {active.length}
        </span>
      </button>
      {expanded && (
        <div
          data-testid="running-agents-list"
          className={`absolute right-0 top-full mt-2 w-64 overflow-hidden ${surface}`}
        >
          <div className={`px-3 py-2 text-[11px] font-semibold ${isDark ? 'text-[#9AA0A6]' : 'text-[#757575]'}`}>
            {copy.runningAgentsTitle}
          </div>
          {/* `visible` above already requires a non-empty list, so no empty
              guard is needed on the <ul> itself. */}
          <ul className="max-h-64 overflow-y-auto pb-1 custom-scrollbar">
            {active.map(entry => (
              <OverlayAgentRow
                key={entryKey(entry.sessionId, entry.agentId)}
                entry={entry}
                recent={false}
                isDark={isDark}
                copy={copy}
                onOpen={openAgent}
              />
            ))}
            {recent.map(entry => (
              <OverlayAgentRow
                key={entryKey(entry.sessionId, entry.agentId)}
                entry={entry}
                recent
                isDark={isDark}
                copy={copy}
                onOpen={openAgent}
              />
            ))}
          </ul>
        </div>
      )}
    </div>
  );
};
