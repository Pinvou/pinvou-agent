/**
 * 蜂群运行小窗的纯展示/筛选模型（无 React、无 bridge 依赖，可被 node:test
 * 直接覆盖）。RunningAgentsOverlay 是唯一生产消费者。
 */

/** 终态条目在列表里短暂停留的时长（成功态可见窗口）。 */
export const RECENT_TERMINAL_MS = 3500;

/**
 * entries 缓存的条目上限：超出后优先淘汰最老的终态条目。蜂群模式单树准入
 * 可达底座上限（1024），长会话不淘汰会无界积攒。
 */
export const MAX_OVERLAY_ENTRIES = 200;

export function entryKey(sessionId, agentId) {
  return `${sessionId || ''}\u0000${agentId || ''}`;
}

export function isTerminal(entry) {
  return !!entry && !!entry.done && !entry.blocked;
}

/**
 * 状态展示：终态优先；非终态把 ledger 的英文状态 token 映射到 i18n 文案
 * （queued/pending/starting → 等待，running → 运行中，与 tool-renderers 的
 * LEDGER_STATUS_TOKENS 同口径），其余视为实时进展短语原样展示。
 */
export function statusPresentation(entry, copy) {
  const statusToken = String(entry && entry.status || '').toLowerCase();
  if (entry && entry.done && entry.failed) return { text: copy.agentCard.failed, dot: 'failed' };
  if (entry && entry.done && entry.blocked) return { text: copy.blockedTag, dot: 'blocked' };
  if (entry && entry.done) return { text: copy.agentCard.completed, dot: 'done' };
  if (['queued', 'pending', 'starting'].includes(statusToken)) return { text: copy.pendingTag, dot: 'running' };
  if (statusToken === 'running') return { text: copy.agentCard.working, dot: 'running' };
  return { text: entry && entry.status && !/\s/.test(String(entry.status)) ? entry.status : copy.agentCard.working, dot: 'running' };
}

/**
 * 覆盖层可见性：有未终态条目，或刚结束的终态条目还在成功态展示窗口内。
 * @param {Array} entries 当前会话的条目列表
 * @param {number} now 时钟（测试注入）
 */
export function overlayVisibleEntries(entries, now) {
  const active = [];
  const recent = [];
  for (const entry of entries || []) {
    if (!isTerminal(entry)) active.push(entry);
    else if (entry.completedAt && now - entry.completedAt < RECENT_TERMINAL_MS) recent.push(entry);
  }
  return { active, recent };
}

/**
 * entries 缓存淘汰：超过上限时按 completedAt 升序淘汰最老的终态条目（无
 * completedAt 视为最老）。返回 null 表示无需变更（未超限，或终态条目不够
 * 淘汰——非终态条目由实际运行中的智能体数天然限定）。
 */
export function pruneOverlayEntries(entries, max = MAX_OVERLAY_ENTRIES) {
  const keys = Object.keys(entries);
  if (keys.length <= max) return null;
  const overflow = keys.length - max;
  const terminal = keys
    .filter(key => isTerminal(entries[key]))
    .sort((left, right) => (entries[left].completedAt || 0) - (entries[right].completedAt || 0));
  if (terminal.length <= overflow) return null;
  const evict = new Set(terminal.slice(0, overflow));
  const next = {};
  for (const key of keys) {
    if (!evict.has(key)) next[key] = entries[key];
  }
  return next;
}
