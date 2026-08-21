// 代码会话 checkpoint / 回退的视图侧状态与纯逻辑。
//
// Rust 侧（features/code_checkpoints + app/commands/checkpoints）在每个用户消息
// （turn）开始时对执行根打快照并记录 turn 序号（1-based）；本模块把快照对齐到
// 原生车道时间线的 turn 边界，提供「回退到第 N 轮」入口的变体判定与 diff 摘要
// 解析（确认弹窗预览）。
//
// 对齐规则：投影 turns 中带 userItem 的用户 turn 按出现顺序计序号，与 Rust
// count_user_turns（is_user_turn_prompt 同口径）一一对应；preamble/系统项不占
// 序号。turn N+1 有 Turn 快照 → 该边界入口为「回退到第 N 轮」（代码+对话）；
// 快照缺失（LRU 淘汰/当时快照失败）→ 「仅回退对话」变体（conversationOnly）。
// 快照创建失败的 turn 只是入口变体不同，不会错位——回退前的 diff 预览展示的
// 始终是真实差异。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invokeTauri as invoke } from '../../platform/tauri/client.js';

/**
 * checkpoint 列表 → Map<turnNumber, checkpoint>。
 * turn 缺失（计数失败兜底）的条目不参与对齐；同一 turn 取先创建者（turn 快照），
 * 回滚点（turn=None 的 preRestore）不占位。
 */
export function checkpointMapByTurn(checkpoints) {
  const map = new Map();
  for (const checkpoint of checkpoints || []) {
    const turn = Number.isInteger(checkpoint?.turn) && checkpoint.turn > 0
      ? checkpoint.turn
      : null;
    if (turn === null) continue;
    if (!map.has(turn)) map.set(turn, checkpoint);
  }
  // 全部缺序号时退化为按创建顺序对齐（老数据/计数失败场景）。
  if (!map.size && Array.isArray(checkpoints)) {
    checkpoints.forEach((checkpoint, index) => {
      if (checkpoint && checkpoint.id) map.set(index + 1, checkpoint);
    });
  }
  return map;
}

/** diff changes 清单 → 按状态计数的摘要（确认弹窗「将撤销的变更」）。 */
export function summarizeCheckpointChanges(changes) {
  const summary = { added: 0, modified: 0, deleted: 0, renamed: 0, copied: 0, other: 0, total: 0 };
  for (const change of changes || []) {
    if (!change || typeof change !== 'object') continue;
    const status = typeof change.status === 'string' ? change.status : 'other';
    if (Object.prototype.hasOwnProperty.call(summary, status) && status !== 'total') {
      summary[status] += 1;
    } else {
      summary.other += 1;
    }
    summary.total += 1;
  }
  return summary;
}

/**
 * 时间线 turn 边界 → 回退入口（Map<turnId, entry>）。
 * entry = { keepTurns, checkpoint, conversationOnly }：
 * - keepTurns = 用户 turn 序号 - 1（「回退到第 N 轮」= 恢复第 N+1 轮快照 + 对话
 *   截断到第 N 轮；第 1 个用户 turn 的边界 keepTurns=0，即清空全部）；
 * - 该边界对应的 Turn 快照存在 → conversationOnly=false（代码+对话一起回退）；
 *   快照缺失 → conversationOnly=true 的「仅回退对话」变体（设计 §5/§7）。
 * checkpoint 列表为空（系统无 git/快照全部失败）时整个会话不渲染入口（设计 §5）。
 */
export function rewindEntriesByTurnId(turns, checkpoints) {
  const entries = new Map();
  const list = Array.isArray(checkpoints) ? checkpoints : [];
  if (!list.length) return entries;
  const byTurn = checkpointMapByTurn(list);
  let ordinal = 0;
  for (const turn of turns || []) {
    if (!turn || !turn.userItem || turn.id == null) continue;
    ordinal += 1;
    const checkpoint = byTurn.get(ordinal) || null;
    entries.set(turn.id, {
      keepTurns: ordinal - 1,
      checkpoint,
      conversationOnly: !checkpoint,
    });
  }
  return entries;
}

/**
 * checkpoint 列表的刷新键（修复联调 Bug A：列表加载竞态）。
 * 只看 turnCount 不够：新 turn 的 Turn 快照由 Rust 在 turn 开始时写入，发送
 * 瞬间（turnCount 变化）触发的拉取可能拿到不含该快照的旧列表，而 turn 完成
 * 不再改变 turnCount——没有 busy 边沿的话列表会一直停在旧值，该 turn 边界
 * 入口被 rewindEntriesByTurnId 误判为「仅回退对话」变体。busy true→false
 * （chat:done，含失败）时快照已落盘，此时键变化触发重拉，入口变体才收敛。
 */
export function checkpointRefreshKey({ turnCount, busy }) {
  return `${Number(turnCount) || 0}:${busy ? 'busy' : 'idle'}`;
}

/**
 * 回退结果 → 时间线内联提示文案（纯函数，copy 取 uiCodex）。
 * restoredCheckpoint 非空 = 代码已恢复且自动打了 PreRestore 回滚点（可反悔）；
 * degraded = 快照不可用只截断了对话；兜底 = 仅对话回退（代码未动）。
 * hadCompaction：截断不清 system_prompt，回退后模型上下文可能仍带着描述被截
 * 轮次的压缩摘要——在基础提示后如实追加，不替换。
 */
export function rewindNoticeText(copy, result, keepTurns) {
  const base = result?.degraded
    ? copy.rewindNoticeDegraded
    : result?.restoredCheckpoint
      ? copy.rewindNoticeRestored(keepTurns)
      : copy.rewindNoticeConversationOnly;
  return result?.hadCompaction ? `${base} ${copy.rewindNoticeCompaction}` : base;
}

/**
 * 「撤销回退」入口可见性（纯函数）：后端 rewind_undo_state 的可反悔语义
 * （sidecar 有备份 + 回退后未发新轮次 + 有 PreRestore 快照）已收敛为
 * 「非 null 即渲染」，这里只做形状校验——无 PreRestore checkpointId 的
 * 条目视为不可反悔（如仅对话回退后后端返回的残缺状态）。
 */
export function rewindUndoAvailable(undoState) {
  return Boolean(undoState && typeof undoState === 'object' && undoState.checkpointId);
}

/**
 * 回退成功后的视图重载编排（修复联调 Bug B，依赖注入便于单测）。
 * 语义：engine 已被后端回收、磁盘对话已截断——调用方传入的 reload 走既有
 * 会话重载路径重注水（前端不自己维护截断）。无论 reload 成败都 bumpTick：
 * hydrate 可能已完成而重载尾部步骤失败/被并发守卫提前返回，lane 已是新内容
 * 时必须兜底触发重投影；reload 本身失败时错误返回给调用方如实上屏，不静默。
 * 「撤销回退」成功后复用同一编排（undo_last_rewind 同样重建 engine + 还原对话）。
 */
export async function reloadSessionAfterRewind({ reload, bumpTick }) {
  let error = null;
  try {
    await reload();
  } catch (err) {
    error = String(err && err.message ? err.message : err);
  } finally {
    bumpTick();
  }
  return { error };
}

/**
 * 会话级 checkpoint 状态：列表加载/刷新 + diff 预览缓存 + 可反悔状态。
 * `enabled` 由调用方门控（仅原生代码会话且已有 sessionId）；`refreshKey` 变化
 * 时重新拉取（调用方应使用 checkpointRefreshKey：turns 数 + busy 边沿），让新
 * turn 的回退入口及时出现并收敛为正确变体；undoState 与列表同节奏刷新（回退
 * 后/发新轮后可见性随之收敛）。回退编排（rewind_to_turn / undo_last_rewind）
 * 由视图层直接调用，成功后走既有 loadSession 重载，再调本 hook 的 refresh。
 */
export function useSessionCheckpoints({ sessionId, enabled, refreshKey }) {
  const [checkpoints, setCheckpoints] = useState([]);
  const [previews, setPreviews] = useState({});
  const [undoState, setUndoState] = useState(null);
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;

  const refresh = useCallback(async () => {
    const id = sessionRef.current;
    if (!id || !enabled) {
      setCheckpoints([]);
      setPreviews({});
      setUndoState(null);
      return;
    }
    try {
      const list = await invoke('list_checkpoints', { sessionId: id });
      if (sessionRef.current === id) {
        setCheckpoints(Array.isArray(list) ? list : []);
      }
    } catch {
      // 列表失败（会话被删/索引损坏/非代码会话）不打扰主流程：该会话没有回退入口。
      if (sessionRef.current === id) setCheckpoints([]);
    }
    try {
      const state = await invoke('rewind_undo_state', { sessionId: id });
      if (sessionRef.current === id) {
        setUndoState(rewindUndoAvailable(state) ? state : null);
      }
    } catch {
      // 可反悔状态查询失败同样静默：不渲染「撤销回退」入口即诚实降级。
      if (sessionRef.current === id) setUndoState(null);
    }
  }, [enabled]);

  useEffect(() => {
    setPreviews({});
    refresh();
    // refreshKey 由调用方给（turns.length），变化即重新拉取。
  }, [sessionId, enabled, refreshKey, refresh]);

  /** 懒加载某 checkpoint 的 diff 预览（缓存随 sessionId/refreshKey 失效）。 */
  const preview = useCallback(async (checkpointId) => {
    const id = sessionRef.current;
    if (!id) return;
    setPreviews(current => ({ ...current, [checkpointId]: { loading: true } }));
    try {
      const diff = await invoke('checkpoint_diff', { sessionId: id, checkpointId });
      if (sessionRef.current === id) {
        setPreviews(current => ({ ...current, [checkpointId]: { loading: false, diff } }));
      }
    } catch (error) {
      if (sessionRef.current === id) {
        setPreviews(current => ({ ...current, [checkpointId]: { loading: false, error: String(error) } }));
      }
    }
  }, []);

  return useMemo(
    () => ({ checkpoints, previews, preview, refresh, undoState }),
    [checkpoints, previews, preview, refresh, undoState],
  );
}
