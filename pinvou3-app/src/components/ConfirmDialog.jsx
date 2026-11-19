// Shared iOS-style confirm dialog (#817 B1), built on ModalShell so every
// confirm gets the hardened shell for free: portal to <body>, Escape to cancel
// (IME-guarded and disabled while busy), focus capture/restore, and a backdrop
// button that cannot dismiss an in-flight confirmation. The alert semantics
// follow ArchiveConfirmDialog's ConfirmDialogShell — the audit's best-specced
// template: role=alertdialog + aria-modal + labelled title, centered copy, and
// a hairline button block in two layouts:
// - `grid` (default): side-by-side Cancel | Action — the alert idiom
//   (ArchiveConfirmDialog, SheetRowConfirm, the scheduled delete confirm).
// - `stacked`: Action (destructive red) above Cancel — the settings sheet idiom
//   (model / search / memory delete, feedback close, add-all models).
// Absorbed per the audit: SheetConfirmDialog, SheetRowConfirm,
// SavedPersonaConfirmDialog, ProviderFormModal's confirm layer, the scheduled
// delete confirm, and ArchiveConfirmDialog/ArchivedDeleteConfirmDialog.
// Deliberately NOT absorbed: TsAlert (an OK-only loading/result alert, not a
// cancel/confirm pair — force-fitting it here would be the over-abstraction
// the audit warns against) and ProvidersSection's CardConfirm /
// TransferModalShell (already rebuilt on ModalShell; converging their pill
// footer onto this iOS idiom is follow-up material, not dedup).
// No copy lives here: callers pass their own `t.*` labels, so trilingual
// parity stays with the sites (tests/ui_language_coverage keeps enforcing it).

import { useId } from 'react';
import { ModalShell } from './ModalShell.jsx';

// Hairline separators shared by both layouts (ConfirmDialogShell's recipe).
const HAIRLINE = 'border-black/[0.12] dark:border-white/[0.12]';
const CANCEL_BLUE = 'text-[#007AFF] dark:text-[#0A84FF]';
const DESTRUCTIVE_RED = 'text-[#FF3B30] dark:text-[#FF453A]';

export function ConfirmDialog({
  testid,
  confirmTestId,
  cancelTestId,
  zIndexClass = 'z-[110]',
  title = null,
  description,
  confirmLabel,
  cancelLabel,
  destructive = false,
  layout = 'grid',
  busy = false,
  describedBy,
  children = null,
  onCancel,
  onConfirm,
}) {
  // A confirm without an accessible name is the exact gap the audit flagged;
  // derive stable ids for the title (labelledby) and the description
  // (describedby) so screen readers announce both without callers wiring ids.
  // alertdialog requires an accessible name, so a title-less confirm (the
  // provider-form confirm layer) names itself from the description instead.
  // An explicit `describedBy` (pointing at richer external copy) wins.
  const titleId = useId();
  const descriptionId = useId();
  const stacked = layout === 'stacked';
  const confirmTone = destructive ? DESTRUCTIVE_RED : CANCEL_BLUE;
  return (
    <ModalShell
      testid={testid}
      zIndexClass={zIndexClass}
      backdropClass="absolute inset-0 cursor-default bg-black/35 backdrop-blur-md"
      backdropLabel={cancelLabel}
      busy={busy}
      role="alertdialog"
      labelledBy={title ? titleId : (description ? descriptionId : undefined)}
      describedBy={describedBy ?? (description ? descriptionId : undefined)}
      panelClass={`max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-white text-[#1C1C1E] dark:bg-[#2C2C2E] dark:text-[#F2F2F7] ${stacked ? 'w-[270px]' : 'w-[320px]'}`}
      onCancel={onCancel}
    >
      <div className="px-5 pt-5 pb-4 text-center">
        {title && <h3 id={titleId} className="text-[17px] leading-6 font-semibold">{title}</h3>}
        {description && (
          <p id={descriptionId} className={`text-[13px] leading-[18px] text-[#8A8A8E] dark:text-[#98989D] ${title ? 'mt-1.5' : 'mt-0.5'}`}>
            {description}
          </p>
        )}
      </div>
      {children}
      <div className={stacked ? `border-t ${HAIRLINE}` : `grid grid-cols-2 border-t ${HAIRLINE}`}>
        {stacked ? (
          <>
            <button
              type="button"
              data-testid={confirmTestId}
              onClick={onConfirm}
              disabled={busy}
              className={`w-full h-12 text-[17px] font-semibold border-b ${HAIRLINE} disabled:opacity-45 ${confirmTone}`}
            >
              {confirmLabel}
            </button>
            <button
              type="button"
              data-testid={cancelTestId}
              onClick={onCancel}
              disabled={busy}
              className={`w-full h-12 text-[17px] font-semibold disabled:opacity-45 ${CANCEL_BLUE}`}
            >
              {cancelLabel}
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              data-testid={cancelTestId}
              onClick={onCancel}
              disabled={busy}
              className={`h-12 text-[17px] font-normal border-r ${HAIRLINE} disabled:opacity-45 ${CANCEL_BLUE}`}
            >
              {cancelLabel}
            </button>
            <button
              type="button"
              data-testid={confirmTestId}
              onClick={onConfirm}
              disabled={busy}
              className={`h-12 text-[17px] font-semibold disabled:opacity-45 ${confirmTone}`}
            >
              {confirmLabel}
            </button>
          </>
        )}
      </div>
    </ModalShell>
  );
}
