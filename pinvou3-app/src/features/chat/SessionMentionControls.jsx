import { MessageSquare, X } from '../../components/icons.jsx';

/**
 * Session mention (referenced chats): the composer chip strip and the @ panel
 * candidate list. Visually aligned with the existing pill language of
 * AttachmentChips / BackgroundTasksIndicator; all copy is injected via
 * props.copy (trilingual i18n keys live under uiSessionMention in shared/i18n).
 */

const CHIP_CLS =
  'h-7 max-w-[220px] rounded-lg pl-2 pr-1 inline-flex items-center gap-1.5 text-[12px] ' +
  'bg-[#E8F0FE] text-[#1967D2] dark:bg-[#1F3A5F] dark:text-[#A8C7FA]';

// Chip colors for the feature-off state (docs/builtin-toolset-contract.md §3.3
// layer 4 degradation of existing entry points): greyed out but still removable.
const CHIP_DISABLED_CLS =
  'h-7 max-w-[220px] rounded-lg pl-2 pr-1 inline-flex items-center gap-1.5 text-[12px] ' +
  'bg-black/[0.04] text-[#9AA0A6] dark:bg-white/[0.06] dark:text-[#80868B]';

/** Mention chip strip above the composer (removable one by one); with the feature off the whole strip degrades to grey (disabledNotice on hover). */
export function SessionMentionChips({ refs, onRemove, copy, disabled = false, disabledNotice = '' }) {
  if (!refs || refs.length === 0) return null;
  return (
    <div data-testid="session-mention-chips" className="flex flex-wrap gap-1.5 mb-2 px-2">
      {refs.map((ref) => (
        <span
          key={ref.sessionId}
          className={disabled ? CHIP_DISABLED_CLS : CHIP_CLS}
          title={disabled ? disabledNotice : ref.title}
        >
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

/** Session candidate list of the @ panel (the keyboard selection is driven by selectedIndex; Enter/click picks). */
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
 * Reference cards inside sent messages (click to jump to the target session).
 * knownSessionIds provides the liveness check: when the referenced session was
 * deleted the card degrades to an unavailable state (not clickable).
 * With disabled (feature off, §3.3 layer 4 degradation) every card is
 * unclickable and shows the short feature-off label (copy.cardDisabled), with
 * the full disabledNotice on hover; historical injection blocks are untouched.
 */
export function SessionMentionCards({ refs, knownSessionIds, onOpenSession, copy, disabled = false, disabledNotice = '' }) {
  if (!refs || refs.length === 0) return null;
  return (
    <div data-testid="session-mention-cards" className="flex max-w-full flex-wrap justify-end gap-1.5 mb-1.5">
      {refs.map((ref, index) => {
        const known = !disabled && (!knownSessionIds || knownSessionIds.has(ref.sessionId));
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
            {disabled
              ? <span className="shrink-0 text-[11px]">{copy.cardDisabled}</span>
              : (!known && <span className="shrink-0 text-[11px]">{copy.cardUnavailable}</span>)}
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
            title={disabled ? disabledNotice : (known ? label : copy.cardUnavailable)}
            // Alive but without a navigation callback (non-main-timeline
            // contexts): keep the normal colors, only unclickable — the dead
            // grey is reserved for deleted sessions and the feature-off state.
            className={base + (known ? active : dead)}
          >
            {inner}
          </span>
        );
      })}
    </div>
  );
}
