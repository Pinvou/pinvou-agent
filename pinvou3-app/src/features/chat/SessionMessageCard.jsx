// biome-ignore-all lint/a11y/useAriaPropsSupportedByRole: the non-interactive arms are labelled pills (not widget roles), mirroring the sibling SessionMentionCards' per-instance suppression — the deleted-session/unattributed reason must reach assistive tech instead of staying tooltip-only.
import { MessageSquare } from '../../components/icons.jsx';

/**
 * Sender card of a received cross-session message (session-reader's
 * send_message_to_session, delivered by features/messaging): sits above the
 * message body in the receiving session's timeline. Attributed senders are
 * clickable and jump to the sender session (same navigation as the mention
 * reference cards); a deleted sender degrades to unavailable, an
 * unattributed one to a generic notice. Copy is injected via props.copy
 * (trilingual keys under uiSessionMessage).
 *
 * Lives in its own module and is lazy-loaded by ChatView through
 * CHAT_PANEL_LOADERS: the static copy pushed the web main entry past the
 * then-500 kB budget (the enforced gate is MAIN_ENTRY_BUDGET_BYTES, now
 * 502 kB — see vite.config.mjs; round-18 CI), and the entry-budget policy
 * requires new entry work to be split or lazy-loaded. ChatView prefetches
 * the chunk on mount, so the split costs no perceptible latency.
 */
export function SessionMessageCard({ sender, knownSessionIds, onOpenSession, copy }) {
  // Round-8 M4: the {sessionId:null, title:null} shape is REAL production
  // input (an unattributed delivery — the server documents omitting
  // from_session); the old guard returned null for exactly that shape and
  // the message rendered as a plain first-party bubble with no untrusted
  // framing. It now falls through to the static fromUnknown card below.
  if (!sender) return null;
  // An EMPTY id set is the hydration window (both session lists momentarily
  // blank), not proof of deletion — treat it as known so the "unavailable"
  // label never flashes on a freshly mounted view (same guard as the
  // sibling SessionMentionCards, round-28 asto should-fix 6).
  const known = Boolean(sender.sessionId) && (!knownSessionIds
    || knownSessionIds.size === 0 || knownSessionIds.has(sender.sessionId));
  const deleted = Boolean(sender.sessionId) && !known;
  const label = sender.title || sender.sessionId;
  const base =
    'max-w-[240px] rounded-xl px-3 py-1.5 inline-flex items-center gap-1.5 text-[12px] border ';
  const active =
    'border-blue-200 bg-blue-50 text-blue-700 hover:bg-blue-100 ' +
    'dark:border-blue-500/20 dark:bg-blue-500/10 dark:text-blue-300 dark:hover:bg-blue-500/20';
  // Alive-but-not-interactive (mirrors SessionMentionCards round-25 minor 9):
  // the same colors as active WITHOUT the hover variants — a hover affordance
  // on a card that cannot navigate invites the click it cannot serve.
  const alive =
    'border-blue-200 bg-blue-50 text-blue-700 ' +
    'dark:border-blue-500/20 dark:bg-blue-500/10 dark:text-blue-300';
  const dead = 'border-black/[0.06] text-[#9AA0A6] dark:border-white/5 dark:text-[#80868B]';
  const inner = (
    <>
      <MessageSquare size={13} className="shrink-0" />
      <span className="min-w-0 truncate">{copy.from(label)}</span>
    </>
  );
  if (known && onOpenSession) {
    return (
      <div data-testid="session-message-card" className="flex max-w-full justify-end mb-1.5">
        <button
          type="button"
          data-testid={'session-message-card-' + sender.sessionId}
          title={copy.jump(label)}
          aria-label={copy.jump(label)}
          onClick={() => onOpenSession(sender.sessionId)}
          className={base + active}
        >
          {inner}
        </button>
      </div>
    );
  }
  return (
    <div data-testid="session-message-card" className="flex max-w-full justify-end mb-1.5">
      <span
        title={deleted ? copy.unavailable : copy.fromUnknown}
        tabIndex={deleted ? 0 : undefined}
        aria-label={deleted ? `${label} — ${copy.unavailable}` : undefined}
        className={base + (deleted ? dead : alive)}
      >
        {deleted
          ? <span className="min-w-0 truncate text-[11px]">{copy.from(label)} · {copy.unavailable}</span>
          : sender.sessionId
            ? inner
            : <span className="min-w-0 truncate text-[11px]">{copy.fromUnknown}</span>}
      </span>
    </div>
  );
}
