// Folder rebind confirmation: shown after the user picked a successor folder
// for a project root that vanished from disk. Purely presentational; the
// container owns picking and the two-phase confirmExisting handshake (first
// attempt without confirmation, backend rejects when the original folder still
// exists, then this dialog escalates to the strong warning).
import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, RefreshCw, X } from '../../components/icons.jsx';

const RebindFolderDialog = ({ from, to, warnExisting, t, busy, onCancel, onConfirm }) => {
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape' && !busy) onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onCancel]);

  if (typeof document === 'undefined') return null;

  return createPortal(
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close; keyboard path is the Escape listener and the cancel button
    <div
      role="presentation"
      className="fixed inset-0 z-[200] flex items-center justify-center p-4"
      style={{ background: 'rgba(0,0,0,.34)', backdropFilter: 'blur(14px) saturate(140%)', WebkitBackdropFilter: 'blur(14px) saturate(140%)' }}
      onClick={() => !busy && onCancel()}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: dialog body stops bubbling so backdrop close is not triggered accidentally; not interactive itself */}
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t.uiProjects.rebindTitle}
        onClick={e => e.stopPropagation()}
        className="w-[380px] max-w-[calc(100vw-48px)] overflow-hidden rounded-[16px] shadow-2xl bg-[rgba(250,250,250,.96)] dark:bg-[rgba(44,44,46,.96)] text-[#000] dark:text-[#F2F2F7]"
        style={{ fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}
        data-testid="rebind-folder-confirm"
      >
        <div className="px-4 pt-4 pb-2 flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <RefreshCw size={17} className="shrink-0 text-[#5F6368] dark:text-[#9AA0A6]" />
            <span className="text-[15px] font-semibold truncate">{t.uiProjects.rebindTitle}</span>
          </div>
          <button
            type="button"
            title={t.cpCancel}
            disabled={busy}
            onClick={onCancel}
            className="w-8 h-8 shrink-0 rounded-full flex items-center justify-center text-[#5F6368] hover:bg-[#D3D7DB] dark:text-[#C4C7C5] dark:hover:bg-[#444746]"
          >
            <X size={16} />
          </button>
        </div>
        <div className="px-4 pb-2 space-y-2 text-[13px]">
          <div className="break-all">
            <span className="text-[#8A8F94] dark:text-[#9AA0A6]">{t.uiProjects.rebindFolder}: </span>
            <span className="line-through decoration-[#C5221F]/70">{from}</span>
          </div>
          <div className="break-all">
            <span className="text-[#8A8F94] dark:text-[#9AA0A6]">→ </span>
            <span className="font-medium break-all">{to}</span>
          </div>
          <div className="text-[12px] text-[#5F6368] dark:text-[#C4C7C5]">
            {t.uiProjects.rebindSessionsHint()}
          </div>
          {warnExisting && (
            <div className="flex items-start gap-2 rounded-2xl bg-[#FCE8E6] dark:bg-[#3C2A29] px-3 py-2 text-[12px] text-[#C5221F] dark:text-[#F28B82]" data-testid="rebind-warn-existing">
              <AlertTriangle size={14} className="shrink-0 mt-0.5" />
              <span>{t.uiProjects.rebindOldExistsWarn}</span>
            </div>
          )}
        </div>
        <div className="px-4 pb-4 pt-1 flex gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => onConfirm(!!warnExisting)}
            className="flex-1 h-10 rounded-full bg-[#0B57D0] text-white text-[14px] font-medium hover:bg-[#0A4CB8] disabled:opacity-50"
          >
            {t.uiProjects.rebindConfirm}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={onCancel}
            className="h-10 px-4 rounded-full bg-[#D3D7DB] dark:bg-[#444746] text-[#1F1F1F] dark:text-[#E3E3E3] text-[14px] font-medium hover:opacity-90 disabled:opacity-50"
          >
            {t.cpCancel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
};

export { RebindFolderDialog };
