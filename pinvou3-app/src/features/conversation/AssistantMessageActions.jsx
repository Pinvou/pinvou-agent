import React, { useEffect, useRef, useState } from 'react';
import { Check, Copy, X } from '../../components/icons.jsx';
import {
  assistantMessageText,
  copyClipboardText,
  normalizeAssistantMessageText,
} from './message-clipboard.js';

export function AssistantMessageActions({ targetRef, text, copy }) {
  const [status, setStatus] = useState('idle');
  const resetTimerRef = useRef(null);
  const label = status === 'copied'
    ? copy.copyReplySuccess
    : status === 'failed'
      ? copy.copyReplyFailed
      : copy.copyReply;

  useEffect(() => () => {
    if (resetTimerRef.current) clearTimeout(resetTimerRef.current);
  }, []);

  const resetStatusLater = () => {
    if (resetTimerRef.current) clearTimeout(resetTimerRef.current);
    resetTimerRef.current = setTimeout(() => {
      resetTimerRef.current = null;
      setStatus('idle');
    }, 1400);
  };

  const handleCopy = async () => {
    const value = text == null
      ? assistantMessageText(targetRef?.current)
      : normalizeAssistantMessageText(text);
    const copied = await copyClipboardText(value);
    setStatus(copied ? 'copied' : 'failed');
    resetStatusLater();
  };

  return (
    <div data-testid="assistant-message-actions" className="mt-2 flex min-h-8 items-center gap-1">
      <button
        type="button"
        data-testid="assistant-message-copy"
        onClick={handleCopy}
        title={label}
        aria-label={label}
        className={`inline-flex h-8 items-center gap-1.5 rounded-lg px-2 text-[12px] transition-colors ${
          status === 'failed'
            ? 'bg-red-500/[0.08] text-[#C5221F] dark:bg-red-400/10 dark:text-[#F28B82]'
            : 'text-[#747775] hover:bg-black/[0.06] hover:text-[#1F1F1F] dark:text-[#9AA0A6] dark:hover:bg-white/10 dark:hover:text-[#E3E3E3]'
        }`}
      >
        {status === 'copied'
          ? <Check size={14} className="text-[#34C759]" />
          : status === 'failed'
            ? <X size={14} />
            : <Copy size={14} />}
        {status !== 'idle' && <span aria-live="polite">{label}</span>}
      </button>
    </div>
  );
}
