import React from 'react';
import { createPortal } from 'react-dom';
import { Sparkles, X } from '../../components/icons.jsx';

function VoiceShortcutIntroModal({
  isDark,
  copy,
  onClose,
  onToggleShortcut,
  shortcutEnabled = false,
  closeLabel,
  primaryLabel,
}) {
  const canEnable = !shortcutEnabled && typeof onToggleShortcut === 'function';
  const resolvedCloseLabel = closeLabel || copy.voiceIntroGotIt;
  const resolvedPrimaryLabel = primaryLabel
    || (canEnable ? (copy.voiceIntroEnable || copy.voiceIntroStart) : (copy.voiceIntroDone || copy.voiceIntroStart || copy.voiceIntroGotIt));

  return createPortal(
    <div
      className="fixed inset-0 z-[1200] flex items-center justify-center bg-black/35 px-4 py-6 backdrop-blur-md"
      role="dialog"
      aria-modal="true"
      aria-labelledby="voice-shortcut-intro-title"
      onMouseDown={onClose}
    >
      <div
        className={`w-full max-w-[980px] overflow-hidden rounded-[32px] shadow-[0_24px_80px_-32px_rgba(0,0,0,0.45)] ${
          isDark ? 'bg-[#111214] text-[#F1F3F4]' : 'bg-[#F7F9FC] text-[#0F172A]'
        }`}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="relative px-5 pb-5 pt-6 md:px-8 md:pb-7">
          <button
            type="button"
            onClick={onClose}
            aria-label={copy.voiceIntroClose}
            title={copy.voiceIntroClose}
            className={`absolute right-4 top-4 flex h-9 w-9 shrink-0 items-center justify-center rounded-full ${
              isDark ? 'text-[#BDC1C6] hover:bg-white/10' : 'text-[#64748B] hover:bg-black/5'
            }`}
          >
            <X size={18} />
          </button>
          <div className="mb-6 pr-10 text-center">
            <h2 id="voice-shortcut-intro-title" className="text-[24px] font-bold tracking-tight md:text-[30px]">
              {copy.voiceIntroTitle}
            </h2>
            {copy.voiceIntroSubtitle && (
              <p className={`mx-auto mt-2 max-w-[620px] text-[13px] leading-5 md:text-[14px] ${
                isDark ? 'text-[#BDC1C6]' : 'text-[#64748B]'
              }`}>
                {copy.voiceIntroSubtitle}
              </p>
            )}
          </div>
          <div className="grid grid-cols-1 gap-4 md:grid-cols-2 md:gap-5">
            <div className="flex min-h-[360px] flex-col overflow-hidden rounded-[28px] bg-[linear-gradient(120deg,#E0E0E0_0%,#F8FAFC_52%,#DCDCDC_100%)] shadow-[0_12px_36px_-18px_rgba(0,0,0,0.35)]">
              <div className="m-3 flex shrink-0 items-start justify-between rounded-[24px] bg-white/85 p-5 shadow-sm backdrop-blur-xl">
                <div>
                  <div className="mb-1 text-[13px] font-semibold text-slate-500">{copy.voiceIntroShortcutLabel}</div>
                  <div className="text-[38px] font-extrabold leading-none tracking-tight text-slate-950">Alt</div>
                </div>
                <div className="text-right">
                  <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-slate-500">{copy.voiceIntroModeLabel}</div>
                  <div className="text-[17px] font-bold text-slate-800">{copy.voiceIntroDictationMode}</div>
                </div>
              </div>
              <div className="flex flex-1 items-center justify-center px-6 pb-8 pt-3">
                <div className="w-full max-w-sm rounded-2xl bg-white/95 p-5 text-slate-700 shadow-[0_18px_36px_-22px_rgba(15,23,42,0.45)]">
                  <div className="space-y-3">
                    {copy.voiceIntroDictationSteps.map((step, index) => (
                      <div key={step} className="flex items-start gap-3">
                        <span className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-slate-900 text-[12px] font-bold text-white">
                          {index + 1}
                        </span>
                        <span className="text-[15px] font-semibold leading-6 text-slate-700">{step}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </div>
            <div className="flex min-h-[360px] flex-col overflow-hidden rounded-[28px] bg-[linear-gradient(120deg,#E0F2FE_0%,#FCE7F3_50%,#DBEAFE_100%)] shadow-[0_14px_40px_-18px_rgba(79,70,229,0.55)]">
              <div className="m-3 flex shrink-0 items-start justify-between rounded-[24px] bg-white/85 p-5 shadow-sm backdrop-blur-xl">
                <div>
                  <div className="mb-1 text-[13px] font-semibold text-slate-500">{copy.voiceIntroComboLabel}</div>
                  <div className="text-[38px] font-extrabold leading-none tracking-tight text-indigo-600">Alt</div>
                </div>
                <div className="text-right">
                  <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-slate-500">{copy.voiceIntroModeLabel}</div>
                  <div className="text-[17px] font-bold text-slate-800">{copy.voiceIntroTaskMode}</div>
                </div>
              </div>
              <div className="flex flex-1 items-center justify-center px-6 pb-8 pt-3">
                <div className="w-full max-w-md rounded-[24px] bg-white/95 p-6 text-slate-700 shadow-[0_18px_36px_-22px_rgba(79,70,229,0.5)]">
                  <div className="space-y-3">
                    {copy.voiceIntroTaskSteps.map((step, index) => (
                      <div key={step} className="flex items-start gap-3">
                        <span className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-indigo-600 text-[12px] font-bold text-white">
                          {index + 1}
                        </span>
                        <span className="text-[15px] font-semibold leading-6 text-slate-700">{step}</span>
                      </div>
                    ))}
                  </div>
                  <div className="mt-5 rounded-2xl bg-indigo-50/80 px-4 py-3 text-[14px] font-medium leading-6 text-slate-600">
                    <div className="mb-1 flex items-center gap-1.5 text-[12px] font-bold text-indigo-600">
                      <Sparkles size={14} />
                      {copy.voiceIntroTaskExampleLabel}
                    </div>
                    {copy.voiceIntroTaskExample}
                  </div>
                </div>
              </div>
            </div>
          </div>
          <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            {canEnable && (
              <button
                type="button"
                onClick={onClose}
                className={`rounded-full px-4 py-2 text-[13px] font-medium ${
                  isDark ? 'text-[#E8EAED] hover:bg-white/10' : 'text-[#3C4043] hover:bg-black/5'
                }`}
              >
                {resolvedCloseLabel}
              </button>
            )}
            <button
              type="button"
              onClick={() => {
                if (canEnable) {
                  onToggleShortcut(true);
                } else {
                  onClose();
                }
              }}
              className="rounded-full bg-[#0B57D0] px-5 py-2 text-[13px] font-semibold text-white shadow-sm hover:bg-[#1967D2]"
            >
              {resolvedPrimaryLabel}
            </button>
          </div>
        </div>
      </div>
    </div>,
    document.body
  );
}

export { VoiceShortcutIntroModal };
