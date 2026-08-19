import React from 'react';
import { RefreshCw } from '../../components/icons.jsx';

function VoiceAsrPopover({ visible, label, pct, cancelling, copy, onCancel }) {
  if (!visible) return null;
  return (
    <div className="absolute bottom-full right-0 z-[40] mb-2 w-[236px] overflow-hidden rounded-[20px] border border-black/10 bg-white/90 p-3 text-[#1D1D1F] shadow-[0_18px_45px_-18px_rgba(0,0,0,0.45)] backdrop-blur-2xl dark:border-white/10 dark:bg-[#1C1C1E]/90 dark:text-[#F2F2F7]">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-[12px] font-semibold">{label}</div>
          <div className="mt-0.5 text-[11px] text-[#6E6E73] dark:text-[#A1A1AA]">
            {pct != null ? `${pct}%` : copy.asrStages.preparing}
          </div>
        </div>
        <RefreshCw size={17} className="shrink-0 animate-spin text-[#0A84FF]" />
      </div>
      <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-black/10 dark:bg-white/10">
        <div
          className={`h-full rounded-full bg-[#0A84FF] transition-all ${pct == null ? 'w-1/3 animate-pulse' : ''}`}
          style={pct != null ? { width: `${pct}%` } : undefined}
        />
      </div>
      <button
        type="button"
        onClick={onCancel}
        disabled={cancelling}
        className={`mt-3 w-full rounded-full px-3 py-2 text-[13px] font-semibold transition-colors ${
          cancelling
            ? 'cursor-wait bg-black/5 text-gray-400 dark:bg-white/10'
            : 'bg-[#FFF0EF] text-[#D70015] hover:bg-[#FFE3E1] dark:bg-[#3A1F1F] dark:text-[#FF9F92] dark:hover:bg-[#4A2727]'
        }`}
      >
        {cancelling ? copy.cancelling : copy.cancelDownload}
      </button>
    </div>
  );
}

export { VoiceAsrPopover };
