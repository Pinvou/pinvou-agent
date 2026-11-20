// Session archive dialogs stay out of the startup chunk: the archive confirm
// dialog and toast load on first use through VIEW_LOADERS, and the
// archived-delete confirm is only used by lazily loaded views.

// #817 B1: this file's internal ConfirmDialogShell was the audit's reference
// template (role=alertdialog + aria) and is now components/ConfirmDialog.jsx
// itself — the two wrappers keep only their copy, the destructive tint, and
// the old shell's z-[200] tier (explicit, so the confirms stay peers of the
// other body-portal dialogs instead of dropping to ConfirmDialog's z-[110]
// default and losing to e.g. the blocking WebConnectionStatus overlay).
// `theme` used to pick a boxShadow tone; the shared dialog handles dark via
// Tailwind, so the callers' theme prop is simply no longer read. ArchiveToast
// is untouched.
import { ConfirmDialog } from './ConfirmDialog.jsx';

export const ArchiveConfirmDialog = ({ t, onCancel, onConfirm }) => (
  <ConfirmDialog
    zIndexClass="z-[200]"
    title={t.archiveConfirmTitle}
    description={(
      // The message + detail two-line block (detail is empty in zh — the
      // conditional keeps the old "no empty second line" behavior).
      <>
        <span className="block">{t.archiveConfirmMessage}</span>
        {t.archiveConfirmDetail ? <span className="block mt-1">{t.archiveConfirmDetail}</span> : null}
      </>
    )}
    confirmLabel={t.archiveConfirmAction}
    cancelLabel={t.cpCancel}
    onCancel={onCancel}
    onConfirm={onConfirm}
  />
);

export const ArchivedDeleteConfirmDialog = ({ t, onCancel, onConfirm }) => (
  <ConfirmDialog
    zIndexClass="z-[200]"
    destructive
    title={t.archivedDeleteTitle}
    description={t.archivedDeleteMessage}
    confirmLabel={t.archivedDeleteAction}
    cancelLabel={t.cpCancel}
    onCancel={onCancel}
    onConfirm={onConfirm}
  />
);

export const ArchiveToast = ({ t, onClose, onView }) => {
  return (
    <div
      className="fixed left-1/2 top-6 z-[210] -translate-x-1/2 px-2 py-2 rounded-[18px] flex items-center gap-1 shadow-2xl bg-[rgba(250,250,250,.94)] dark:bg-[rgba(44,44,46,.94)] text-[#1C1C1E] dark:text-[#F2F2F7] border border-[rgba(0,0,0,.08)] dark:border-[rgba(255,255,255,.10)]"
      style={{
        backdropFilter: 'blur(18px) saturate(150%)',
        WebkitBackdropFilter: 'blur(18px) saturate(150%)',
        fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
      }}
    >
      <div className="pl-3 pr-2 text-[14px] leading-5 whitespace-nowrap">{t.archiveSuccess}</div>
      <button
        type="button"
        onClick={onView}
        className="h-8 min-w-[76px] px-3 rounded-[12px] text-[14px] font-semibold whitespace-nowrap active:opacity-70 text-[#007AFF] dark:text-[#0A84FF] bg-[rgba(0,122,255,.10)] dark:bg-[rgba(10,132,255,.12)]"
      >
        {t.archiveSuccessView}
      </button>
      <button
        type="button"
        onClick={onClose}
        className="w-8 h-8 rounded-full text-[18px] leading-none active:opacity-70 text-[rgba(60,60,67,.62)] dark:text-[rgba(235,235,245,.62)]"
        aria-label={t.cpCancel}
      >
        ×
      </button>
    </div>
  );
};
