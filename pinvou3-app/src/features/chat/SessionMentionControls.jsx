import { MessageSquare, X } from '../../components/icons.jsx';

/**
 * 引用对话(Session Mention)的输入框 chip 条与 @ 面板列表。
 * 视觉对齐 AttachmentChips / BackgroundTasksIndicator 的既有 pill 语言;
 * 全部文案经 props.copy 注入(i18n 三语键见 shared/i18n 的 uiSessionMention)。
 */

const CHIP_CLS =
  'h-7 max-w-[220px] rounded-lg pl-2 pr-1 inline-flex items-center gap-1.5 text-[12px] ' +
  'bg-[#E8F0FE] text-[#1967D2] dark:bg-[#1F3A5F] dark:text-[#A8C7FA]';

/** 输入框上方的引用 chip 条(可逐个移除)。 */
export function SessionMentionChips({ refs, onRemove, copy }) {
  if (!refs || refs.length === 0) return null;
  return (
    <div data-testid="session-mention-chips" className="flex flex-wrap gap-1.5 mb-2 px-2">
      {refs.map((ref) => (
        <span key={ref.sessionId} className={CHIP_CLS} title={ref.title}>
          <MessageSquare size={13} className="shrink-0" />
          <span className="min-w-0 truncate">{ref.title || ref.sessionId}</span>
          <button
            type="button"
            data-testid={'session-mention-chip-remove-' + ref.sessionId}
            aria-label={copy.chipRemove(ref.title || ref.sessionId)}
            title={copy.chipRemove(ref.title || ref.sessionId)}
            onClick={() => onRemove(ref.sessionId)}
            className="w-5 h-5 shrink-0 rounded-full flex items-center justify-center hover:bg-black/10 dark:hover:bg-white/15"
          >
            <X size={12} />
          </button>
        </span>
      ))}
    </div>
  );
}

/** @ 面板的会话候选列表(键盘选中项由 selectedIndex 驱动,回车/点击选择)。 */
export function SessionMentionMenu({ candidates, selectedIndex, onSelect, onHover, copy }) {
  return (
    <div data-testid="session-mention-menu" role="listbox" aria-label={copy.menuTitle}>
      <div className="px-3 py-2 text-[12px] font-medium text-[#85888D] dark:text-[#9AA0A6]">
        {copy.menuTitle}
      </div>
      {candidates.length === 0 ? (
        <div className="px-3 pb-2 text-[12px] text-[#85888D] dark:text-[#9AA0A6]">
          {copy.menuEmpty}
        </div>
      ) : (
        candidates.map((candidate, index) => (
          <button
            key={candidate.sessionId}
            type="button"
            role="option"
            aria-selected={index === selectedIndex}
            data-testid={'session-mention-option-' + candidate.sessionId}
            onMouseEnter={() => onHover(index)}
            onClick={() => onSelect(candidate)}
            className={
              'w-full px-3 py-1.5 text-left text-[13px] rounded-lg flex items-center gap-2 ' +
              (index === selectedIndex
                ? 'bg-[#F0F4F9] dark:bg-[#333537]'
                : 'hover:bg-[#F0F4F9] dark:hover:bg-[#333537]')
            }
          >
            <MessageSquare size={13} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            <span className="min-w-0 truncate">{candidate.title || candidate.sessionId}</span>
          </button>
        ))
      )}
    </div>
  );
}

/**
 * 已发送消息里的引用卡片(点击跳转目标会话)。
 * knownSessionIds 提供存活判定:被引用会话已删除时卡片降级为失效态(不可点)。
 */
export function SessionMentionCards({ refs, knownSessionIds, onOpenSession, copy }) {
  if (!refs || refs.length === 0) return null;
  return (
    <div data-testid="session-mention-cards" className="flex max-w-full flex-wrap justify-end gap-1.5 mb-1.5">
      {refs.map((ref, index) => {
        const known = !knownSessionIds || knownSessionIds.has(ref.sessionId);
        const label = ref.title || ref.sessionId;
        const base =
          'max-w-[240px] rounded-xl px-3 py-1.5 inline-flex items-center gap-1.5 text-[12px] border ';
        const active =
          'border-black/[0.08] bg-black/[0.03] text-[#444746] hover:bg-black/[0.06] ' +
          'dark:border-white/10 dark:bg-white/[0.06] dark:text-[#C4C7C5] dark:hover:bg-white/10';
        const dead = 'border-black/[0.06] text-[#9AA0A6] dark:border-white/5 dark:text-[#80868B]';
        const inner = (
          <>
            <MessageSquare size={13} className="shrink-0" />
            <span className="min-w-0 truncate">{label}</span>
            {!known && <span className="shrink-0 text-[11px]">{copy.cardUnavailable}</span>}
          </>
        );
        return known && onOpenSession ? (
          <button
            key={ref.sessionId + '-' + index}
            type="button"
            data-testid={'session-mention-card-' + ref.sessionId}
            title={copy.cardJump(label)}
            aria-label={copy.cardJump(label)}
            onClick={() => onOpenSession(ref.sessionId)}
            className={base + active}
          >
            {inner}
          </button>
        ) : (
          <span
            key={ref.sessionId + '-' + index}
            data-testid={'session-mention-card-' + ref.sessionId}
            title={known ? label : copy.cardUnavailable}
            // 存活但无导航回调(非主时间线场景)保持正常配色仅不可点,失效灰只留给已删除会话。
            className={base + (known ? active : dead)}
          >
            {inner}
          </span>
        );
      })}
    </div>
  );
}
