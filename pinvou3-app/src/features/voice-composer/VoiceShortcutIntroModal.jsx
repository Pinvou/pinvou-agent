import { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { Sparkles, X } from '../../components/icons.jsx';
import { isWeb } from '../../shared/platform.js';
import { setVoiceShortcutIntroOpen } from '../chat/voice-shortcut-state.mjs';

function VoiceShortcutIntroModal({
  isDark,
  copy,
  onClose,
  onToggleShortcut,
  shortcutEnabled = false,
  closeLabel,
  primaryLabel,
}) {
  const dialogRef = useRef(null);
  const onCloseRef = useRef(onClose);
  // web 车道是 web_asr_only:无智能整理、Alt 有文本时静默降级为追加听写,
  // 语音编辑卡不再展示,避免承诺 web 上不存在的能力。
  const editSupported = !isWeb;
  const canEnable = !shortcutEnabled && typeof onToggleShortcut === 'function';
  const resolvedCloseLabel = closeLabel || copy.voiceIntroGotIt;
  const resolvedPrimaryLabel = primaryLabel
    || (canEnable ? (copy.voiceIntroEnable || copy.voiceIntroStart) : (copy.voiceIntroDone || copy.voiceIntroStart || copy.voiceIntroGotIt));

  // latest-ref sync: 视图每次渲染都会重建 onClose,经 ref 读取避免下方弹窗
  // 生命周期 effect 反复重挂(重挂会把焦点抢回主按钮)。
  useEffect(() => {
    onCloseRef.current = onClose;
  });

  // 弹窗打开期间:标记给快捷键路由(Esc 让路给弹窗)、自动聚焦主按钮、
  // 捕获 Esc 关闭与 Tab 焦点循环,卸载时归还焦点。只在挂载/卸载时运行一次。
  useEffect(() => {
    setVoiceShortcutIntroOpen(true);
    const dialog = dialogRef.current;
    const previouslyFocused = document.activeElement;
    const focusableSelector = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';
    const focusables = () => (dialog ? [...dialog.querySelectorAll(focusableSelector)]
      .filter(el => !el.disabled) : []);
    const initial = (dialog && dialog.querySelector('[data-autofocus]')) || focusables()[0];
    if (initial && typeof initial.focus === 'function') initial.focus();
    function handleKeyDown(event) {
      if (!event || event.defaultPrevented) return;
      if (event.key === 'Escape') {
        if (event.repeat) return;
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab' || !dialog) return;
      const items = focusables();
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      const activeElement = document.activeElement;
      if (event.shiftKey && (activeElement === first || !dialog.contains(activeElement))) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (activeElement === last || !dialog.contains(activeElement))) {
        event.preventDefault();
        first.focus();
      }
    }
    document.addEventListener('keydown', handleKeyDown, true);
    return () => {
      setVoiceShortcutIntroOpen(false);
      document.removeEventListener('keydown', handleKeyDown, true);
      if (previouslyFocused && previouslyFocused !== document.body
        && typeof previouslyFocused.focus === 'function') {
        previouslyFocused.focus();
      }
    };
  }, []);

  return createPortal(
    <div
      className="fixed inset-0 z-[1200] flex items-center justify-center bg-black/35 px-4 py-6 backdrop-blur-md"
      role="dialog"
      aria-modal="true"
      aria-labelledby="voice-shortcut-intro-title"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={`w-full max-w-[980px] overflow-hidden rounded-[32px] shadow-[0_24px_80px_-32px_rgba(0,0,0,0.45)] ${
          isDark ? 'bg-[#111214] text-[#F1F3F4]' : 'bg-[#F7F9FC] text-[#0F172A]'
        }`}
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
          </div>
          <div className={`grid grid-cols-1 gap-4 md:gap-5 ${editSupported ? 'md:grid-cols-2' : 'md:max-w-[520px] md:mx-auto'}`}>
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
            {editSupported && (
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
            )}
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
              data-autofocus
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
