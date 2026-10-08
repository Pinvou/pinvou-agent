// 原生车道「回退到第 N 轮」的共享编排 hook（工作模式 ChatView 与原生 code
// CodexAcpView 复用，不复制）。
//
// 职责：checkpoint 列表/预览/可反悔状态（useSessionCheckpoints）+ turn 边界对齐
// （rewindEntriesByTurnId）+ 确认弹窗状态机 + rewind_to_turn / undo_last_rewind
// 单 flight 编排（忙碌门在后端）+ 成功后的重载/兜底重投影/内联提示。
// 视图差异全部经依赖注入收敛：
// - invoke：IPC 入口（两视图同为 invokeTauri）；
// - reload(sessionId)：按磁盘截断后内容重注水的既有会话重载路径；
// - ownsSession(sessionId)：UI 收口动作的归属检查（回退在途时用户切走会话，
//   重载/提示只认原会话）；
// - bumpTick：重载失败后仍兜底触发重投影的视图级 tick；
// - appendNotice(sessionId, text)：回退成功的内联提示落点。
// copy 取 uiCodex 回退键组（两车道共用，文案中性）。
//
// 工作模式的「回退范围」选择（scope）：确认弹窗以 { includeWorkspace } 回调
// onConfirm（RewindConfirmDialog scopeSelection 变体）；取消勾选映射为
// conversation_only=true 的既有语义（零后端改动），内联提示按显式仅对话
// 口径（rewindNoticeConversationOnly），不冒充「快照不可用」的降级。

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  checkpointRefreshKey,
  reloadSessionAfterRewind,
  rewindEntriesByTurnId,
  rewindNoticeText,
  rewindUndoAvailable,
  useSessionCheckpoints,
} from './checkpoints.js';

export function useRewindController({
  sessionId,
  enabled,
  turns,
  busy,
  copy,
  invoke,
  reload,
  ownsSession,
  bumpTick,
  appendNotice,
}) {
  // 视图注入的依赖在渲染期镜像进 ref：编排回调保持稳定标识（弹窗 props 不因
  // 视图每渲染重建），读取时永远拿到最新值（与原视图内函数每渲染重建的语义
  // 等价——闭包不再新鲜，ref 补读）。
  const depsRef = useRef({});
  // eslint-disable-next-line react-hooks/refs -- latest-ref sync: deps (copy/reload/ownsSession/…) are read only inside the stable confirm callbacks, never during render; the original in-view orchestration rebuilt fresh closures per render, this mirror is the extracted equivalent
  depsRef.current = { sessionId, enabled, turns, busy, copy, invoke, reload, ownsSession, bumpTick, appendNotice };

  const checkpoints = useSessionCheckpoints({
    sessionId,
    enabled,
    refreshKey: checkpointRefreshKey({ turnCount: turns?.length || 0, busy }),
  });
  const rewindEntries = useMemo(
    () => (enabled ? rewindEntriesByTurnId(turns, checkpoints.checkpoints) : new Map()),
    [enabled, turns, checkpoints.checkpoints],
  );

  // 待确认的回退目标（{ keepTurns, checkpoint, conversationOnly }）；非 null 时渲染确认弹窗。
  // ref 镜像：confirmRewind 为稳定回调，读取不依赖渲染闭包。
  const [rewindTarget, setRewindTargetState] = useState(null);
  const rewindTargetRef = useRef(null);
  const setRewindTarget = useCallback((next) => {
    rewindTargetRef.current = next;
    setRewindTargetState(next);
  }, []);
  const [rewindError, setRewindError] = useState('');
  const [rewinding, setRewinding] = useState(false);
  // 回退/撤销是全局单 flight：in-flight 按 sessionId 记账到 ref（state 只是
  // UI 镜像）。切会话的复位只清 UI 标志；旧 promise 的 finally 仅在 ref 仍
  // 指向本次调用时才清——否则 A 在途时切 B 发起回退，A settle 的 finally 会
  // 把 B 的守卫标志抹掉（评审 M2）。
  const rewindInFlightRef = useRef(null);
  const rewindUndoInFlightRef = useRef(null);
  // 「撤销回退」：入口可见性由 checkpoints.undoState（rewind_undo_state）驱动，
  // null 不渲染。确认弹窗由本地条目副本 rewindUndoEntry 驱动（打开时快照
  // undoState）——后端可反悔状态会因 refreshKey 边沿/记录消费随时变 null，若
  // 弹窗直接挂在其上，「撤销已生效但重载失败」的重试窗口会被边沿击穿（弹窗关、
  // 错误吞、重试通道丢）。reloadFailed 重试期内弹窗与 undoState 生命周期解耦。
  const rewindUndoState = checkpoints.undoState;
  const [rewindUndoEntry, setRewindUndoEntryState] = useState(null);
  const rewindUndoEntryRef = useRef(null);
  const setRewindUndoEntry = useCallback((next) => {
    rewindUndoEntryRef.current = next;
    setRewindUndoEntryState(next);
  }, []);
  const [rewindUndoError, setRewindUndoError] = useState('');
  const [rewindUndoing, setRewindUndoing] = useState(false);
  useEffect(() => {
    // 可反悔状态消失（回退后发了新轮/记录被消费）时收回弹窗；reloadFailed
    // 重试窗口豁免（弹窗由本地副本驱动，重试只补重载、不再发 undo）。
    if (!rewindUndoAvailable(rewindUndoState)) {
      // setRewindUndoEntry 是 ref 镜像包装（只接收值，不支持函数式更新）：从
      // ref 读当前值判定豁免条件，等价于原视图的 (current => …) 复位。
      const current = rewindUndoEntryRef.current;
      if (current && !current.reloadFailed) setRewindUndoEntry(null);
    }
  }, [rewindUndoState, setRewindUndoEntry]);

  // 打开回退确认弹窗；有快照的目标懒加载 diff 预览（「将撤销的变更」摘要）。
  const openRewindDialog = useCallback((entry) => {
    setRewindError('');
    setRewindTarget(entry);
    if (entry.checkpoint) checkpoints.preview(entry.checkpoint.id);
  }, [setRewindTarget, checkpoints]);

  const cancelRewind = useCallback(() => {
    if (!rewindInFlightRef.current) setRewindTarget(null);
  }, [setRewindTarget]);

  const openRewindUndoDialog = useCallback((state) => {
    setRewindUndoError('');
    setRewindUndoEntry({ ...state, reloadFailed: false });
  }, [setRewindUndoEntry]);

  const cancelRewindUndo = useCallback(() => {
    if (!rewindUndoInFlightRef.current) setRewindUndoEntry(null);
  }, [setRewindUndoEntry]);

  // 会话切换复位：收回弹窗与错误、复位 UI busy 镜像（原视图内 [activeId] 复位
  // effect 的共享化）。in-flight ref 故意不动：旧 promise 的 finally 仅在其记账
  // 仍指向本次调用时清理（评审 M2），提前抹掉会让新会话的守卫标志与旧 promise
  // 的清理交错错位；在途期间新会话发起回退/撤销会被单 flight 门如实拒绝。
  const resetForSessionSwitch = useCallback(() => {
    rewindTargetRef.current = null;
    setRewindTargetState(null);
    setRewindError('');
    rewindUndoEntryRef.current = null;
    setRewindUndoEntryState(null);
    setRewindUndoError('');
    setRewinding(false);
    setRewindUndoing(false);
  }, []);

  // 确认回退：rewind_to_turn 编排（恢复文件 + 截断对话 + engine 回收重注水，
  // 含本会话/跨会话忙碌门）。成功后先重载再收口（联调 Bug B）：reload 走视图
  // 注入的既有重载路径按磁盘截断后内容重注水，成败都强制 bumpTick 兜底重投影；
  // 重载失败留在弹窗如实上屏（弹窗不在重载前关闭，错误不再被吞进不可见状态）。
  // 重载失败时回退已在后端生效：刷新 checkpoint/undo 状态让「撤销回退」入口
  // 出现（用户自救通道），并把目标标记为 reloadFailed——重试只补重载，不会
  // 对已截断的对话再发一次 rewind_to_turn（那会必败且文案令人困惑）。
  // choice：工作模式确认弹窗（scopeSelection）传 { includeWorkspace }——取消
  // 勾选映射 conversation_only=true；code 车道与无快照变体不传。
  const confirmRewind = useCallback(async (choice) => {
    const target = rewindTargetRef.current;
    const { sessionId: currentSessionId, copy: currentCopy, invoke: currentInvoke, reload: currentReload, ownsSession: currentOwnsSession, bumpTick: currentBumpTick, appendNotice: currentAppendNotice } = depsRef.current;
    if (!target || !currentSessionId) return;
    // 单 flight（跨操作类型）：本会话/另一会话的回退或撤销任一在途时不静默
    // 吞点击，如实上屏（评审 M9）；两个 ref 分别记账回退与撤销，后端执行根
    // flag 对跨类型并发兜底拒绝（评审 finding：本地门也按跨类型检查，注释
    // 与行为口径一致）。
    if (rewindInFlightRef.current || rewindUndoInFlightRef.current) {
      setRewindError(currentCopy.rewindInFlightBusy);
      return;
    }
    const sessionId = currentSessionId;
    // 显式仅对话（工作模式取消勾选「同时回退工作区文件」）：即便快照存在也
    // 不动文件；无快照边界本就恒为仅对话（entry.conversationOnly）。
    const conversationOnlyRequested = Boolean(choice && choice.includeWorkspace === false);
    rewindInFlightRef.current = sessionId;
    setRewinding(true);
    setRewindError('');
    try {
      const result = target.reloadFailed
        ? null
        : await currentInvoke('rewind_to_turn', {
          sessionId,
          keepTurns: target.keepTurns,
          conversationOnly: target.conversationOnly || conversationOnlyRequested,
        });
      const { error: reloadError } = await reloadSessionAfterRewind({
        // reload 前置归属检查（评审 M4）：回退/撤销在途时用户切到其它会话，
        // reload(原会话) 会把视图的活动会话改回原会话、作废新会话的在途
        // 加载并冻结其流式输出。已切走则跳过重载——磁盘已是目标状态，切回时
        // 自然重注水；notice 补发走 pendingNotice 暂存。
        reload: () => (currentOwnsSession(sessionId)
          ? currentReload(sessionId)
          : Promise.resolve(null)),
        bumpTick: currentBumpTick,
      });
      // 跨会话竞态：await 期间会话被程序化切换（remote control）时，UI 收口
      // 动作只认原会话；重载/状态刷新已按 sessionId 定向完成。已知取舍：若
      // 重载失败且暂存了 pendingNotice，此早退会丢弃补发——回到该会话时时间线
      // 按磁盘重注水，仅少一条内联提示；undo 侧有 refresh 重查 rewind_undo_state
      // 的自愈兜底。而「重载失败+用户取消重试」路径不补发是有意的：彼时屏上
      // 仍是截断前的陈旧时间线，补发「已回退」反而误导。
      if (!currentOwnsSession(sessionId)) return;
      if (reloadError) {
        // 回退已在后端生效：把成功结果随目标暂存（pendingNotice），重试只补
        // 重载、成功后补发提示——避免整条流走完时间线却没有「已回退」确认项。
        // 重试再失败时保留既有暂存（result 为 null 的重试路径不得覆盖）。
        setRewindTarget({
          ...target,
          reloadFailed: true,
          pendingNotice: result
            ? rewindNoticeText(currentCopy, result, target.keepTurns, conversationOnlyRequested)
            : (target.pendingNotice ?? null),
        });
        setRewindError(reloadError);
        checkpoints.refresh();
        return;
      }
      setRewindTarget(null);
      const notice = result
        ? rewindNoticeText(currentCopy, result, target.keepTurns, conversationOnlyRequested)
        : target.pendingNotice;
      if (notice) {
        currentAppendNotice(sessionId, notice);
        currentBumpTick();
      }
      // 入口可用性随新时间线重算（refreshKey 的 turns/busy 变化通常已触发，此处兜底）。
      checkpoints.refresh();
    } catch (err) {
      setRewindError(String(err && err.message ? err.message : err));
    } finally {
      // 仅当 in-flight 记账仍指向本次调用才清除：切会话后旧 promise 的
      // finally 不得抹掉新会话（或新调用）的守卫标志（评审 M2）。
      if (rewindInFlightRef.current === sessionId) {
        rewindInFlightRef.current = null;
        setRewinding(false);
      }
    }
  }, [setRewindTarget, checkpoints]);

  // 撤销回退：undo_last_rewind（恢复文件到绑定回滚点（仅对话降级则跳过）+
  // 对话从备份还原 + engine 重建）；成功后复用 reloadSessionAfterRewind 重载编排
  // （与回退后同语义：先重载、成败都 bumpTick、成功才关弹窗），失败错误留在弹窗
  // 上屏。弹窗状态走本地副本 rewindUndoEntry：重载失败时撤销已在后端生效（记录
  // 已消费，undoState 随后收敛为 null），把 entry 标记 reloadFailed 留在屏上——
  // 重试只补重载，不会再发一次必败的 undo_last_rewind。
  const confirmRewindUndo = useCallback(async () => {
    const entry = rewindUndoEntryRef.current;
    const { sessionId: currentSessionId, copy: currentCopy, invoke: currentInvoke, reload: currentReload, ownsSession: currentOwnsSession, bumpTick: currentBumpTick, appendNotice: currentAppendNotice } = depsRef.current;
    if (!entry || !currentSessionId) return;
    // 与 confirmRewind 同款跨类型单 flight：回退/撤销任一在途时如实上屏（评审 M9）。
    if (rewindUndoInFlightRef.current || rewindInFlightRef.current) {
      setRewindUndoError(currentCopy.rewindInFlightBusy);
      return;
    }
    const sessionId = currentSessionId;
    const reloadOnly = Boolean(entry.reloadFailed);
    rewindUndoInFlightRef.current = sessionId;
    setRewindUndoing(true);
    setRewindUndoError('');
    try {
      if (!reloadOnly) {
        await currentInvoke('undo_last_rewind', { sessionId });
      }
      const { error: reloadError } = await reloadSessionAfterRewind({
        // reload 前置归属检查（评审 M4）：同 confirmRewind。
        reload: () => (currentOwnsSession(sessionId)
          ? currentReload(sessionId)
          : Promise.resolve(null)),
        bumpTick: currentBumpTick,
      });
      // 跨会话竞态：await 期间会话被程序化切换时不再写原会话的 UI 状态。
      if (!currentOwnsSession(sessionId)) return;
      if (reloadError) {
        setRewindUndoEntry({ ...entry, reloadFailed: true });
        setRewindUndoError(reloadError);
        // 与回退侧对齐：刷新让 undoState 收敛（记录已消费 → null），用户取消
        // 弹窗后「撤销回退」入口不会以陈旧状态残留。弹窗由本地 entry 驱动，
        // 不受 undoState 收敛影响（复位 effect 豁免 reloadFailed 条目）。
        checkpoints.refresh();
        return;
      }
      setRewindUndoEntry(null);
      currentAppendNotice(sessionId, currentCopy.rewindUndoDone);
      currentBumpTick();
      // refresh 连带重查 rewind_undo_state：撤销后不可再反悔，入口随之消失。
      checkpoints.refresh();
    } catch (err) {
      setRewindUndoError(String(err && err.message ? err.message : err));
    } finally {
      // 与 confirmRewind 同款：in-flight 记账仍指向本次调用才清除。
      if (rewindUndoInFlightRef.current === sessionId) {
        rewindUndoInFlightRef.current = null;
        setRewindUndoing(false);
      }
    }
  }, [setRewindUndoEntry, checkpoints]);

  return useMemo(
    () => ({
      checkpoints,
      rewindEntries,
      rewindTarget,
      rewindError,
      rewinding,
      openRewindDialog,
      confirmRewind,
      cancelRewind,
      rewindUndoState,
      rewindUndoEntry,
      rewindUndoError,
      rewindUndoing,
      openRewindUndoDialog,
      confirmRewindUndo,
      cancelRewindUndo,
      resetForSessionSwitch,
    }),
    [
      checkpoints,
      rewindEntries,
      rewindTarget,
      rewindError,
      rewinding,
      openRewindDialog,
      confirmRewind,
      cancelRewind,
      rewindUndoState,
      rewindUndoEntry,
      rewindUndoError,
      rewindUndoing,
      openRewindUndoDialog,
      confirmRewindUndo,
      cancelRewindUndo,
      resetForSessionSwitch,
    ],
  );
}
