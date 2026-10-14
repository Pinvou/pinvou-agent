import { useEffect } from 'react';

// Session archive dialogs stay out of the startup chunk: the archive confirm
// dialog and toast load on first use through VIEW_LOADERS, and the
// archived-delete confirm is only used by lazily loaded views.

// Shared iOS-style confirm dialog: backdrop + Escape handling + panel +
// 2-button footer. ArchiveConfirmDialog and ArchivedDeleteConfirmDialog
// differ only in copy and the confirm-action tint; the destructive
// variant is the newer semantics and wins for the shared role/aria
// attributes (role=alertdialog).
const ConfirmDialogShell = ({ theme, t, labelledById, title, message, detail, actionLabel, destructive = false, onCancel, onConfirm }) => {
  const isDark = theme === 'dark';
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel]);
  return (
    // Backdrop click-to-close; keyboard path: Escape (effect listener below) and the real "Cancel" button inside the dialog.
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer; the keyboard path is handled by the Escape listener and the cancel button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{
        background: 'rgba(0,0,0,.34)',
        backdropFilter: 'blur(14px) saturate(140%)',
        WebkitBackdropFilter: 'blur(14px) saturate(140%)',
        fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif'
      }}
      onClick={onCancel}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body only stops bubbling to avoid accidentally triggering backdrop close; not an interactive control itself */}
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={labelledById}
        className="w-[320px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{
          // isDark stays for the dynamic boxShadow value.
          boxShadow: isDark ? '0 24px 60px rgba(0,0,0,.55)' : '0 24px 60px rgba(0,0,0,.22)'
        }}
        onClick={e => e.stopPropagation()}
      >
        <div className="px-6 pt-6 pb-5 text-center">
          <div id={labelledById} className="text-[20px] font-semibold leading-[26px]">{title}</div>
          <div className="mt-2.5 text-[15px] leading-[22px] text-[rgba(60,60,67,.72)] dark:text-[rgba(235,235,245,.72)]">
            <div>{message}</div>
            {detail && <div className="mt-1">{detail}</div>}
          </div>
        </div>
        <div className="h-px bg-[rgba(60,60,67,.24)] dark:bg-[rgba(84,84,88,.65)]" />
        <div className="grid grid-cols-2">
          <button
            type="button"
            onClick={onCancel}
            className="h-[50px] text-[17px] active:opacity-70 text-[#007AFF] dark:text-[#0A84FF]"
          >
            {t.cpCancel}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            className={`h-[50px] text-[17px] font-semibold active:opacity-70 ${destructive ? 'text-[#FF3B30]' : 'text-[#007AFF] dark:text-[#0A84FF]'} border-l border-l-[rgba(60,60,67,.24)] dark:border-l-[rgba(84,84,88,.65)]`}
          >
            {actionLabel}
          </button>
        </div>
      </div>
    </div>
  );
};

export const ArchiveConfirmDialog = ({ theme, t, onCancel, onConfirm }) => (
  <ConfirmDialogShell
    theme={theme}
    t={t}
    labelledById="archive-confirm-title"
    title={t.archiveConfirmTitle}
    message={t.archiveConfirmMessage}
    detail={t.archiveConfirmDetail}
    actionLabel={t.archiveConfirmAction}
    onCancel={onCancel}
    onConfirm={onConfirm}
  />
);

export const ArchivedDeleteConfirmDialog = ({ theme, t, onCancel, onConfirm }) => (
  <ConfirmDialogShell
    theme={theme}
    t={t}
    labelledById="archived-delete-confirm-title"
    title={t.archivedDeleteTitle}
    message={t.archivedDeleteMessage}
    actionLabel={t.archivedDeleteAction}
    destructive
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
