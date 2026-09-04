import { AlertTriangle, X } from '../../components/icons.jsx';
import { voiceStatusLabel } from './voice-ui-policy.mjs';

function VoiceReadyNotice({ visible, copy }) {
  if (!visible) return null;
  return (
    <div className="mb-2 flex justify-end px-2">
      <div className="inline-flex max-w-full items-center gap-2 rounded-full border border-[#34C759]/20 bg-white/90 px-3 py-1.5 text-[12px] font-medium text-[#1B7F3A] shadow-sm backdrop-blur-xl dark:border-white/10 dark:bg-[#1C1C1E]/85 dark:text-[#D1FADF]">
        <span className="h-2 w-2 shrink-0 rounded-full bg-[#34C759]" />
        <span className="truncate">{copy.asrReadyNotice}</span>
      </div>
    </div>
  );
}

function VoiceNoticeBar({ voiceInput, voiceMode, copy, dark = false, canInstallLocalAsr, onGotoSettings, onRetry, onCancel, onClose }) {
  if (!voiceInput || voiceInput.status !== 'failed' || !voiceInput.message) return null;
  if (voiceInput.category === 'empty_result') {
    const cardClass = dark
      ? 'border-white/[0.08] bg-[#1C1C1E]/88 text-[#F5F5F7] shadow-[0_18px_42px_-30px_rgba(0,0,0,0.9)]'
      : 'border-black/[0.06] bg-white/88 text-[#1D1D1F] shadow-[0_18px_42px_-30px_rgba(0,0,0,0.35)]';
    const iconClass = dark
      ? 'bg-[#FF9F0A]/16 text-[#FFD18A]'
      : 'bg-[#FF9500]/12 text-[#B96800]';
    const hintClass = dark ? 'text-[#AEAEB2]' : 'text-[#6E6E73]';
    const secondaryButtonClass = dark
      ? 'bg-white/[0.08] text-[#F5F5F7] hover:bg-white/[0.13]'
      : 'bg-black/[0.05] text-[#3A3A3C] hover:bg-black/[0.08]';
    const closeClass = dark
      ? 'text-[#AEAEB2] hover:bg-white/10'
      : 'text-[#8E8E93] hover:bg-black/[0.06]';
    return (
      <div className="mb-2 flex justify-end px-2">
        <div className={`inline-flex max-w-full items-center gap-3 rounded-[18px] border px-3 py-2.5 backdrop-blur-2xl ${cardClass}`}>
          <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full ${iconClass}`}>
            <AlertTriangle size={16} />
          </div>
          <div className="min-w-[190px] flex-1">
            <div className="text-[13px] font-semibold leading-5">
              {copy.voiceEmptyResultTitle || copy.voiceInputFailed}
            </div>
            <div className={`mt-0.5 text-[12px] leading-5 ${hintClass}`}>
              {copy.voiceEmptyResultHint || voiceStatusLabel(voiceInput, voiceMode, copy)}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onRetry}
              className="rounded-full bg-[#007AFF] px-3.5 py-1.5 text-[12px] font-semibold text-white shadow-sm hover:bg-[#0A84FF]"
            >
              {copy.voiceRetryAgain || copy.voiceRetry}
            </button>
            <button
              type="button"
              onClick={onCancel}
              className={`rounded-full px-3.5 py-1.5 text-[12px] font-medium ${secondaryButtonClass}`}
            >
              {copy.voiceCancelShort || copy.voiceCancel}
            </button>
            <button
              type="button"
              onClick={onClose}
              title={copy.voiceClose}
              className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-full ${closeClass}`}
            >
              <X size={15} />
            </button>
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="mb-2 flex items-center justify-between gap-2 rounded-2xl bg-[#FCE8E6] px-3 py-2 text-[12px] text-[#C5221F] dark:bg-[#3A1F1F] dark:text-[#F28B82]">
      <span className="min-w-0 truncate">
        {voiceStatusLabel(voiceInput, voiceMode, copy)}
      </span>
      <div className="flex shrink-0 items-center gap-1">
        {voiceInput.category === 'recognition_failed' && canInstallLocalAsr && onGotoSettings && (
          <button type="button" onClick={onGotoSettings} className="rounded-full bg-black/5 px-2 py-1 font-medium hover:bg-black/10 dark:bg-white/10 dark:hover:bg-white/20">
            {copy.voiceGotoDeps}
          </button>
        )}
        <button type="button" onClick={onRetry} className="rounded-full px-2 py-1 hover:bg-black/5 dark:hover:bg-white/10">
          {copy.voiceRetry}
        </button>
        <button type="button" onClick={onCancel} className="rounded-full px-2 py-1 hover:bg-black/5 dark:hover:bg-white/10">
          {copy.voiceCancel}
        </button>
        <button type="button" onClick={onClose} title={copy.voiceClose} className="flex h-6 w-6 items-center justify-center rounded-full hover:bg-black/5 dark:hover:bg-white/10">
          ×
        </button>
      </div>
    </div>
  );
}

export { VoiceNoticeBar, VoiceReadyNotice };
