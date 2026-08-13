import React from 'react';
import { Check, ChevronDown, Mic, RefreshCw, Send, X } from '../../components/icons.jsx';
import { COMPOSER_ICON_BUTTON_CLASS } from '../chat/composer-controls.jsx';
import { VoiceAsrPopover } from './VoiceAsrPopover.jsx';
import { VoiceNoticeBar, VoiceReadyNotice } from './VoiceNoticeBar.jsx';
import { VoiceRecordingPill } from './VoiceRecordingPill.jsx';
import {
  isVoiceActive,
  isVoiceRecording,
  primaryVoiceLabel,
  shouldShowVoicePill,
  voiceAsrBusyState,
} from './voice-ui-policy.mjs';

function VoiceComposerPillLayer({ voiceInput, voiceMode, copy, onCancel, onConfirm }) {
  if (!shouldShowVoicePill(voiceInput)) return null;
  return (
    <div className="pointer-events-none absolute inset-x-0 top-1/2 z-[3] flex -translate-y-1/2 justify-center px-4">
      <VoiceRecordingPill
        status={voiceInput.status}
        mode={voiceMode}
        message={voiceInput.message}
        copy={copy}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    </div>
  );
}

function VoiceComposerButton({
  refProp,
  voiceInput,
  voiceMode,
  voiceAsrSetup,
  voiceAsrPopoverOpen,
  copy,
  asrCopy,
  disabled,
  testId = 'composer-voice-button',
  onClick,
  menuOpen,
  menuItems,
  onToggleMenu,
  onToggleAsrPopover,
  onCancelAsr,
}) {
  const resolvedAsrCopy = asrCopy || (copy && copy.uiChat) || copy;
  const asrBusy = voiceAsrBusyState(voiceAsrSetup, resolvedAsrCopy);
  const active = isVoiceActive(voiceInput);
  const recording = isVoiceRecording(voiceInput);
  const label = asrBusy.busy ? asrBusy.label : primaryVoiceLabel(voiceInput, voiceMode, copy);
  const hasMenu = !asrBusy.busy && !active && Array.isArray(menuItems) && menuItems.length > 0;
  return (
    <div ref={refProp} className="relative flex shrink-0 items-center">
      <VoiceAsrPopover
        visible={asrBusy.busy && voiceAsrPopoverOpen}
        label={asrBusy.label}
        pct={asrBusy.pct}
        cancelling={asrBusy.cancelling}
        copy={resolvedAsrCopy}
        onCancel={onCancelAsr}
      />
      <div className="flex items-center">
        <button
          type="button"
          onClick={asrBusy.busy ? onToggleAsrPopover : onClick}
          disabled={disabled || asrBusy.cancelling}
          data-testid={testId}
          aria-label={label}
          title={label}
          className={`${
            recording
              ? 'flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-transparent bg-[#C5221F] text-white transition-colors hover:bg-[#A50E0E]'
              : asrBusy.busy
                ? `${COMPOSER_ICON_BUTTON_CLASS} text-[#174EA6] dark:text-[#A8C7FA]`
                : active
                  ? `${COMPOSER_ICON_BUTTON_CLASS} text-[#174EA6] dark:text-[#A8C7FA]`
                  : COMPOSER_ICON_BUTTON_CLASS
          } ${(disabled || asrBusy.cancelling) ? 'cursor-wait opacity-70' : ''}`}
        >
          {asrBusy.busy ? <RefreshCw size={18} className="animate-spin" /> : <Mic size={18} />}
        </button>
        {hasMenu && (
          <button
            type="button"
            onClick={onToggleMenu}
            disabled={disabled}
            aria-label={copy.voiceModeMenu || '语音模式'}
            title={copy.voiceModeMenu || '语音模式'}
            className="ml-[-5px] flex h-9 w-6 shrink-0 items-center justify-center rounded-full text-[#5F6368] hover:bg-black/5 dark:text-[#C4C7C5] dark:hover:bg-white/10"
          >
            <ChevronDown size={13} />
          </button>
        )}
      </div>
      {hasMenu && menuOpen && (
        <div className="absolute bottom-11 right-0 z-40 w-44 rounded-xl border border-black/[0.08] bg-white p-1.5 text-[12px] text-[#202124] shadow-xl dark:border-white/10 dark:bg-[#202124] dark:text-[#E8EAED]">
          {menuItems.map(item => (
            <button
              key={item.key}
              type="button"
              onClick={item.onSelect}
              className="flex h-8 w-full items-center rounded-lg px-2.5 text-left hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
            >
              <span className="truncate">{item.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function VoiceComposerStatus({
  voiceInput,
  voiceMode,
  copy,
  chatCopy,
  dark = false,
  voiceAsrReadyNotice,
  canInstallLocalAsr,
  onGotoSettings,
  onRetry,
  onCancel,
  onClose,
}) {
  return (
    <>
      <VoiceReadyNotice visible={voiceAsrReadyNotice} copy={chatCopy} />
      <VoiceNoticeBar
        voiceInput={voiceInput}
        voiceMode={voiceMode}
        copy={copy}
        dark={dark}
        canInstallLocalAsr={canInstallLocalAsr}
        onGotoSettings={onGotoSettings}
        onRetry={onRetry}
        onCancel={onCancel}
        onClose={onClose}
      />
    </>
  );
}

function VoiceEditPreview({ preview, copy, onApply, onApplyAndSend, onCancel }) {
  if (!preview) return null;
  const original = String(preview.original || '').trim();
  const next = String(preview.next || '').trim();
  const title = copy.voiceEditPreviewTitle || '语音编辑预览';
  const applyLabel = copy.voiceEditApply || '应用修改';
  const applyAndSendLabel = copy.voiceEditApplyAndSend || '应用并发送';
  const cancelLabel = copy.voiceEditCancel || copy.voiceCancel || '取消';
  return (
    <div
      data-testid="voice-edit-preview"
      className="mb-2 rounded-2xl border border-[#DADCE0] bg-[#F8FAFD] p-3 text-[12px] text-[#202124] shadow-sm dark:border-white/10 dark:bg-[#202124] dark:text-[#E8EAED]"
    >
      <div className="mb-2 flex items-center justify-between gap-2">
        <div className="font-medium">{title}</div>
        <button
          type="button"
          onClick={onCancel}
          aria-label={cancelLabel}
          title={cancelLabel}
          className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-[#5F6368] hover:bg-black/5 dark:text-[#C4C7C5] dark:hover:bg-white/10"
        >
          <X size={15} />
        </button>
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        <div className="min-w-0 rounded-xl bg-white px-2.5 py-2 dark:bg-[#17181A]">
          <div className="mb-1 text-[10px] font-medium uppercase text-[#80868B]">{copy.voiceEditOriginal || '原文'}</div>
          <div className="max-h-24 overflow-y-auto whitespace-pre-wrap break-words leading-5">{original}</div>
        </div>
        <div className="min-w-0 rounded-xl bg-white px-2.5 py-2 ring-1 ring-[#A8C7FA] dark:bg-[#17181A] dark:ring-[#3B6EA8]">
          <div className="mb-1 text-[10px] font-medium uppercase text-[#1A73E8] dark:text-[#A8C7FA]">{copy.voiceEditResult || '修改后'}</div>
          <div className="max-h-24 overflow-y-auto whitespace-pre-wrap break-words leading-5">{next}</div>
        </div>
      </div>
      <div className="mt-2 flex flex-wrap justify-end gap-2">
        <button
          type="button"
          onClick={onCancel}
          className="inline-flex h-8 items-center gap-1.5 rounded-lg px-3 text-[12px] font-medium text-[#5F6368] hover:bg-black/5 dark:text-[#C4C7C5] dark:hover:bg-white/10"
        >
          <X size={14} />{cancelLabel}
        </button>
        <button
          type="button"
          onClick={onApply}
          className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-[#E8F0FE] px-3 text-[12px] font-medium text-[#174EA6] hover:bg-[#D2E3FC] dark:bg-[#1E2B3A] dark:text-[#A8C7FA] dark:hover:bg-[#24364A]"
        >
          <Check size={14} />{applyLabel}
        </button>
        {onApplyAndSend && (
          <button
            type="button"
            onClick={onApplyAndSend}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-[#0B57D0] px-3 text-[12px] font-medium text-white hover:bg-[#1967D2] dark:bg-[#A8C7FA] dark:text-[#041E49]"
          >
            <Send size={14} />{applyAndSendLabel}
          </button>
        )}
      </div>
    </div>
  );
}

export {
  VoiceComposerButton,
  VoiceEditPreview,
  VoiceComposerPillLayer,
  VoiceComposerStatus,
};
