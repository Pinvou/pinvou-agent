import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import { can } from '../../shared/platform.js';
import {
  RECENT_TERMINAL_MS,
  entryKey,
  isTerminal,
  overlayVisibleEntries,
  pruneOverlayEntries,
  statusPresentation,
} from './overlay-model.mjs';

/**
 * 蜂群运行小窗（右上角，ADR-0006 蜂群改造）：当前会话有未终态子智能体时
 * 显示。收起态是一枚计数胶囊，展开态列出每个运行中的子智能体（名称/角色 +
 * 状态点 + 简短状态），点击条目派发 `pinvou:open-subagent` 打开其只读执行
 * 记录面板；收起/展开选择记入 localStorage。
 *
 * 状态源两层（与行内专家卡同一哲学：实时事件可能丢，落盘投影是权威）：
 * - `pinvou:subagent-update`（bridge 转发的实时进展/完成事件）；
 * - 本组件自持的 `listSubagentTranscripts` 轮询兜底——只在存在未终态条目
 *   （或会话刚切换还没有数据）时轮询（3s 节流），全部终态即停；轮询读到的
 *   整份 ledger 快照另经 `pinvou:subagent-ledger-update` 广播：spawn 计数行
 *   取代行内专家卡后，行内卡原有的共享轮询不再有常驻 watcher，本组件是
 *   运行期唯一的常驻广播源，行内协调卡的后代树投影依赖它。
 *
 * 边框情绪价值：蜂群关闭 = 蓝边框，蜂群开启 = 紫边框（深浅色各配一档）；
 * 运行中状态点带呼吸动画，完成后短暂保持绿色成功态再淡出列表。
 */

const COLLAPSE_STORAGE_KEY = 'pinvou3.swarmOverlay.collapsed';
const POLL_INTERVAL_MS = 3000;

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
    return localStorage.getItem(COLLAPSE_STORAGE_KEY) !== '1';
  });
  const entriesRef = useRef(entries);
  useEffect(() => {
    entriesRef.current = entries;
  }, [entries]);

  const mergeEntry = useCallback((sessionIdIn, detail) => {
    if (!detail || !detail.agentId) return;
    const key = entryKey(sessionIdIn, detail.agentId);
    setEntries(previous => {
      const prev = previous[key];
      // 终态 ratchet：落盘（ledger）终态是权威；迟到的非终态实时事件不得把
      // 条目翻回运行中（落盘重唤醒场景由 ledger 快照本身负责翻回）。
      if (prev && prev.done && !detail.done && detail.source !== 'ledger') return previous;
      const next = { ...prev, ...detail, sessionId: sessionIdIn };
      if (detail.done && !(prev && prev.done)) next.completedAt = Date.now();
      if (!detail.done) delete next.completedAt;
      const merged = { ...previous, [key]: next };
      const pruned = pruneOverlayEntries(merged);
      return pruned || merged;
    });
  }, []);

  // 实时事件 + 共享 ledger 快照订阅。
  useEffect(() => {
    if (!enabled || typeof window === 'undefined') return;
    const onUpdate = event => {
      const detail = event && event.detail;
      if (!detail || (sessionId && detail.sessionId && detail.sessionId !== sessionId)) return;
      mergeEntry(detail.sessionId || sessionId, detail);
    };
    const onLedger = event => {
      const detail = event && event.detail;
      if (!detail || !Array.isArray(detail.agents)) return;
      if (sessionId && detail.sessionId && detail.sessionId !== sessionId) return;
      const ledgerSession = detail.sessionId || sessionId;
      for (const summary of detail.agents) {
        if (!summary || !summary.agent_id) continue;
        mergeEntry(ledgerSession, {
          sessionId: ledgerSession,
          agentId: summary.agent_id,
          role: summary.role || null,
          status: summary.status || null,
          done: !!summary.done,
          failed: !!summary.failed,
          blocked: !!summary.blocked,
          source: 'ledger',
        });
      }
    };
    window.addEventListener('pinvou:subagent-update', onUpdate);
    window.addEventListener('pinvou:subagent-ledger-update', onLedger);
    return () => {
      window.removeEventListener('pinvou:subagent-update', onUpdate);
      window.removeEventListener('pinvou:subagent-ledger-update', onLedger);
    };
  }, [enabled, mergeEntry, sessionId]);

  const sessionEntries = useMemo(
    () => Object.values(entries).filter(entry => entry.sessionId === sessionId),
    [entries, sessionId],
  );
  // 会话内是否存在未终态条目：驱动轮询兜底的启停（见下方 effect）。
  const sessionHasActive = useMemo(
    () => sessionEntries.some(entry => !isTerminal(entry)),
    [sessionEntries],
  );

  // 轮询兜底：有未终态条目（或会话刚切换还没有数据）时轮询落盘投影，全部
  // 终态即停。重启是声明式的：新条目进入运行态时 `sessionHasActive` 翻回
  // true，本 effect 重跑——不存在「停表后无法唤醒」的死状态。单次读数失败
  // （bridge 返回 null 而不是 []）是瞬时故障：不停表，下一轮照常重试（与
  // SubagentTranscriptPanel 同口径）。
  useEffect(() => {
    if (!enabled || !sessionId) return;
    let stopped = false;
    let timer = null;
    const poll = async () => {
      timer = null;
      if (stopped) return;
      try {
        const list = await bridge.multiAgent.listSubagentTranscripts(sessionId);
        if (stopped) return;
        if (Array.isArray(list)) {
          for (const summary of list) {
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
          // 整份快照广播一次，供行内协调卡投影自己的后代树。
          window.dispatchEvent(new CustomEvent('pinvou:subagent-ledger-update', {
            detail: { sessionId, agents: list },
          }));
        }
      } catch {
        // 单次轮询失败不致命，下一轮重试。
      }
      if (!stopped && sessionHasActive) timer = setTimeout(poll, POLL_INTERVAL_MS);
    };
    timer = setTimeout(poll, 0);
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  }, [enabled, mergeEntry, sessionId, sessionHasActive]);

  // 会话切换时丢弃其他会话的条目，避免跨会话串台。
  useEffect(() => {
    if (!sessionId) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- 只在 sessionId 变化时清理一次旧会话缓存，与渲染级联无关
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

  // 成功态展示窗口到期后自醒一次，把已展示完的终态条目淡出列表。
  const [, setRecentTick] = useState(0);
  // eslint-disable-next-line react-hooks/purity -- 成功态展示窗口按真实时钟判定，tick 到点后重算一次
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
        // 存储不可写只影响记忆，不影响功能。
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

export default RunningAgentsOverlay;
