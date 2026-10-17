// 原生车道会话时间线上的「回退到第 N 轮」入口与确认弹窗（工作模式 + 原生
// code 共用）。
//
// 每个用户 turn 边界渲染一个 RewindChip（由视图用 rewindEntriesByTurnId 对齐）：
// 点击打开 RewindConfirmDialog，懒加载 checkpoint_diff 展示「将撤销的变更」
// 摘要（计数 + 文件清单），并明示对话将截断到的位置；确认后由视图层调
// rewind_to_turn 编排（恢复文件 + 截断对话 + engine 回收重注水）。无 Turn 快照
// 的边界是「仅回退对话」变体（conversationOnly），文案明示文件不回退。
//
// 工作模式变体（scopeSelection）：确认弹窗额外呈现「是否同时回退工作区文件」
// 选择项，默认勾选（勾选 = 双层回退，展示变更预览让代价可见；取消勾选 =
// conversation_only=true 仅截断对话，文件不动）。code 车道不传 scopeSelection，
// 弹窗维持「改动随对话回退」的双层默认，无此选择项。


import { RotateCcw } from '../../components/icons.jsx';


export function RewindChip({ entry, disabled, copy, onOpen }) {
  const label = entry.conversationOnly
    ? copy.rewindChipConversationOnly(entry.keepTurns)
    : copy.rewindChip(entry.keepTurns);
  return (
    // Idle state is a faint thin divider line; the whole row is the hover zone,
    // and hovering anywhere near the line fades the line out and the rewind
    // button in. focus-visible keeps the button reachable from the keyboard.
    // pointer-events follow visibility: an opacity-0 button would otherwise
    // still intercept clicks aimed at the timeline content beneath it.
    <div className="group relative my-1 flex h-7 items-center justify-center">
      <div
        aria-hidden="true"
        className="h-px w-24 bg-black/[0.08] transition-opacity group-hover:opacity-0 dark:bg-white/[0.12]"
      />
      <button
        type="button"
        data-testid="rewind-chip"
        disabled={disabled}
        onClick={() => onOpen(entry)}
        title={entry.conversationOnly ? copy.rewindConversationOnlyNote : copy.rewindPreRestoreNote}
        className="pointer-events-none absolute inline-flex max-w-full items-center gap-1.5 rounded-xl border border-black/[0.06] bg-white px-2.5 py-1 text-[11px] text-gray-500 opacity-0 shadow-sm transition-opacity focus:pointer-events-auto focus:opacity-100 group-hover:pointer-events-auto group-hover:opacity-100 focus-visible:pointer-events-auto focus-visible:opacity-100 hover:text-gray-700 disabled:cursor-not-allowed dark:border-white/10 dark:bg-[#2A2B2E] dark:text-gray-400 dark:hover:text-gray-200"
      >
        <RotateCcw size={11} className="shrink-0" />
        <span className="truncate">{label}</span>
      </button>
    </div>
  );
}

// 「撤销回退」入口：渲染在时间线末尾（回退成功的内联提示其后），可见性由
// rewind_undo_state 驱动（null 不渲染，见 checkpoints.js rewindUndoAvailable）。
//
// 撤销文案按 state.checkpointId 分流：有绑定回滚点 = 文件+对话一起恢复；
// null = 被撤销的那次回退是仅对话降级（文件未动过），撤销也只还原对话，
// 文案必须如实、不得承诺恢复文件。
function rewindUndoBodyText(copy, state) {
  return state?.checkpointId
    ? copy.rewindUndoBody(state.rewoundTurns)
    : copy.rewindUndoBodyConversationOnly(state.rewoundTurns);
}

export function RewindUndoChip({ state, disabled, copy, onOpen }) {
  return (
    <div className="my-1 flex justify-center">
      <button
        type="button"
        data-testid="rewind-undo-chip"
        disabled={disabled}
        onClick={() => onOpen(state)}
        title={rewindUndoBodyText(copy, state)}
        className="inline-flex max-w-full items-center gap-1.5 rounded-xl border border-blue-500/25 bg-blue-500/[0.06] px-2.5 py-1 text-[11px] text-blue-600 transition-colors hover:bg-blue-500/10 disabled:cursor-not-allowed disabled:opacity-40 dark:text-blue-300"
      >
        <RotateCcw size={11} className="shrink-0" />
        <span className="truncate">{copy.rewindUndo}</span>
      </button>
    </div>
  );
}
