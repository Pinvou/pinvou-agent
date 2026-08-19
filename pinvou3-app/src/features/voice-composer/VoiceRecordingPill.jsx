import React from 'react';
import { Check, X } from '../../components/icons.jsx';
import { voiceModeLabel, voicePostprocessingLabel } from './voice-ui-policy.mjs';

const WAVE_BARS = [
  { height: 11, colors: ['#B388FF', '#7C4DFF'] },
  { height: 18, colors: ['#64D2FF', '#0A84FF'] },
  { height: 26, colors: ['#7DFFB2', '#34C759'] },
  { height: 16, colors: ['#FFD166', '#FF9F0A'] },
  { height: 23, colors: ['#FF8FAB', '#FF375F'] },
  { height: 14, colors: ['#B388FF', '#7C4DFF'] },
  { height: 20, colors: ['#64D2FF', '#0A84FF'] },
];

function VoiceWaveform({ muted = false }) {
  return (
    <div className="flex h-8 flex-1 items-center justify-center gap-[4px] px-3" aria-hidden="true">
      {WAVE_BARS.map((bar, index) => (
        <span
          key={`${bar.height}-${index}`}
          className={`w-[3px] rounded-full ${muted ? 'opacity-45' : 'animate-pulse opacity-95'}`}
          style={{
            height: `${bar.height}px`,
            animationDelay: `${index * 80}ms`,
            background: `linear-gradient(to top, ${bar.colors[0]}, ${bar.colors[1]})`,
            boxShadow: `0 0 8px ${bar.colors[0]}55`,
          }}
        />
      ))}
    </div>
  );
}

function VoiceRecordingPill({ status, mode, message, copy, onCancel, onConfirm }) {
  const active = status === 'requesting_permission'
    || status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing';
  if (!active) return null;
  const busy = status === 'requesting_permission' || status === 'transcribing' || status === 'postprocessing';
  const modeLabel = voiceModeLabel(mode, copy);
  const statusText = status === 'recording'
    ? modeLabel
    : status === 'postprocessing'
      ? voicePostprocessingLabel(mode, copy)
      : status === 'transcribing'
        ? copy.voiceTranscribing
        : status === 'requesting_permission'
          ? copy.voiceRequesting
          : (message || copy.voiceInputFailed);
  return (
    <div className="pointer-events-auto flex flex-col items-center gap-1">
      <div className="flex w-[min(240px,calc(100vw-32px))] items-center justify-between rounded-full border border-white/15 bg-[#16161C]/80 px-2 py-1.5 shadow-[0_18px_38px_-16px_rgba(0,0,0,0.7),0_0_28px_rgba(161,140,209,0.16),inset_0_1px_2px_rgba(255,255,255,0.14),inset_0_-1px_2px_rgba(0,0,0,0.45)] backdrop-blur-xl">
        <button
          type="button"
          onClick={onCancel}
          aria-label={copy.voiceCancel}
          title={copy.voiceCancel}
          className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-[#FF7675] to-[#D63031] text-white shadow-[0_8px_16px_-8px_rgba(214,48,49,0.85),inset_0_1px_3px_rgba(255,255,255,0.28)] transition-transform hover:shadow-[0_10px_20px_-10px_rgba(214,48,49,0.9),inset_0_1px_3px_rgba(255,255,255,0.32)] active:scale-90"
        >
          <X size={20} strokeWidth={2.6} />
        </button>
        <VoiceWaveform muted={busy} />
        <button
          type="button"
          onClick={onConfirm}
          disabled={busy}
          aria-label={copy.voiceStop}
          title={copy.voiceStop}
          className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-[#55EFC4] to-[#00B894] text-white shadow-[0_8px_16px_-8px_rgba(0,184,148,0.85),inset_0_1px_3px_rgba(255,255,255,0.3)] transition-transform hover:shadow-[0_10px_20px_-10px_rgba(0,184,148,0.9),inset_0_1px_3px_rgba(255,255,255,0.34)] active:scale-90 ${busy ? 'cursor-wait opacity-60' : ''}`}
        >
          <Check size={21} strokeWidth={3} />
        </button>
      </div>
      {status !== 'recording' && (
        <div className="max-w-[min(220px,calc(100vw-40px))] truncate rounded-full bg-black/75 px-2.5 py-0.5 text-[11px] font-medium text-white shadow-lg backdrop-blur-md">
          {statusText}
        </div>
      )}
    </div>
  );
}

export { VoiceRecordingPill, VoiceWaveform };
