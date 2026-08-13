import React from 'react';
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

function VoiceNoticeBar({ voiceInput, voiceMode, copy, canInstallLocalAsr, onGotoSettings, onRetry, onCancel, onClose }) {
  if (!voiceInput || voiceInput.status !== 'failed' || !voiceInput.message) return null;
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
