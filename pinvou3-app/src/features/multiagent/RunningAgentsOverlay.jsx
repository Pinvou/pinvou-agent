import { useCallback, useEffect, useMemo, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import { can } from '../../shared/platform.js';
import {
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  mergeOverlayEntry,
  overlayVisibleEntries,
  pruneOverlayEntries,
  statusPresentation,
} from './overlay-model.mjs';
import { useSubagentLedgerPoll } from './useSubagentLedgerPoll.js';

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
  blocked: 'bg-[#F9AB00] animate-pulse',
  done: 'bg-[#137333] dark:bg-[#93D5A6]',
  failed: 'bg-[#C5221F] dark:bg-[#F28B82]',
};

export const RunningAgentsOverlay = ({ sessionId, theme, t, swarmOn = false }) => {
  const copy = t.uiMultiAgent;
  const isDark = theme === 'dark';
  const enabled = can('multiAgent') && bridge.available && !!bridge.multiAgent;
  const [entries, setEntries] = useState({});
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
    if (!detail || !detail.agentId) return;
    const key = entryKey(sessionIdIn, detail.agentId);
    setEntries(previous => {
      const next = mergeOverlayEntry(previous[key], detail, sessionIdIn, Date.now());
      if (!next) return previous;
      const merged = { ...previous, [key]: next };
      const pruned = pruneOverlayEntries(merged);
      return pruned || merged;
    });
  }, []);

  // Real-time event subscription.
  useEffect(() => {
    if (!enabled || typeof window === 'undefined') return;
    const onUpdate = event => {
      const detail = event && event.detail;
      if (!detail || (sessionId && detail.sessionId && detail.sessionId !== sessionId)) return;
      mergeEntry(detail.sessionId || sessionId, detail);
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
  // Whether any entry in this session is non-terminal: drives the ledger poll
  // cadence (see the hook).
  const sessionHasActive = useMemo(
    () => sessionEntries.some(entry => !isTerminal(entry)),
    [sessionEntries],
  );

  const readLedger = useCallback(
    id => bridge.multiAgent.listSubagentTranscripts(id),
    [],
  );

  const mergeLedgerSummaries = useCallback(summaries => {
    for (const summary of summaries) {
      if (!summary || !summary.agent_id) continue;
      mergeEntry(sessionId, {
        sessionId,
        agentId: summary.agent_id,
        role: summary.role || null,
        status: summary.status || null,
        done: !!summary.done,
        failed: !!summary.failed,
        blocked: !!summary.blocked,
        source: 'ledger',
      });
    }
  }, [mergeEntry, sessionId]);

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
  });

  // Drop entries of other sessions on a session switch to avoid cross-talk.
  useEffect(() => {
    if (!sessionId) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-time cleanup of the previous session's cache on sessionId change, unrelated to the render cascade
    setEntries(previous => {
      const next = {};
      let changed = false;
      for (const [key, entry] of Object.entries(previous)) {
        if (entry.sessionId === sessionId) next[key] = entry;
        else changed = true;
      }
      return changed ? next : previous;
    });
  }, [sessionId]);

  // Wake once when the success-state display window expires so finished
  // terminal entries fade out of the list.
  const [, setRecentTick] = useState(0);
  // eslint-disable-next-line react-hooks/purity -- the success-state window is judged on the real clock; recompute once when the tick fires
  const { active, recent } = overlayVisibleEntries(sessionEntries, Date.now());
  useEffect(() => {
    if (recent.length === 0) return;
    const timer = setTimeout(() => setRecentTick(value => value + 1), RECENT_TERMINAL_MS + 100);
    return () => clearTimeout(timer);
  }, [recent.length]);
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
    if (typeof window === 'undefined' || !agentId) return;
    window.dispatchEvent(new CustomEvent('pinvou:open-subagent', {
      detail: { agentId, sessionId: agentSessionId || sessionId || null },
    }));
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
          {active.length === 0 && recent.length === 0 ? null : (
            <ul className="max-h-64 overflow-y-auto pb-1 custom-scrollbar">
              {active.map(entry => {
                const status = statusPresentation(entry, copy);
                return (
                  <li key={entryKey(entry.sessionId, entry.agentId)}>
                    <button
                      type="button"
                      data-testid="running-agents-entry"
                      onClick={() => openAgent(entry.agentId, entry.sessionId)}
                      className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12.5px] transition-colors ${
                        isDark ? 'hover:bg-white/[0.06]' : 'hover:bg-black/[0.04]'
                      }`}
                    >
                      <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${dotClass[status.dot]}`} />
                      <span className="min-w-0 flex-1 truncate font-medium">
                        {entry.role || entry.agentId}
                      </span>
                      <span className={`shrink-0 truncate text-[10.5px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#757575]'}`}>
                        {status.text}
                      </span>
                    </button>
                  </li>
                );
              })}
              {recent.map(entry => (
                <li key={entryKey(entry.sessionId, entry.agentId)}>
                  <button
                    type="button"
                    data-testid="running-agents-entry-done"
                    onClick={() => openAgent(entry.agentId, entry.sessionId)}
                    className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12.5px] transition-colors ${
                      isDark ? 'hover:bg-white/[0.06]' : 'hover:bg-black/[0.04]'
                    }`}
                  >
                    <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${dotClass.done}`} />
                    <span className="min-w-0 flex-1 truncate font-medium opacity-70">
                      {entry.role || entry.agentId}
                    </span>
                    <span className={`shrink-0 text-[10.5px] ${isDark ? 'text-[#93D5A6]' : 'text-[#137333]'}`}>
                      {copy.agentCard.completed}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
};
