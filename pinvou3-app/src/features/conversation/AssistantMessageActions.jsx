import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  Check,
  ChevronDown,
  Code,
  Copy,
  Download,
  FileText,
  MessageCircle,
  Share2,
  X,
} from '../../components/icons.jsx';
import {
  openAssistantShareTarget,
  saveAssistantResponseFile,
  shareAssistantResponseWithSystem,
} from '../../platform/assistant-response.js';
import {
  assistantExportFilename,
  buildAssistantResponseExport,
} from './assistant-response-export.js';
import { copyClipboardText, normalizeAssistantMessageText } from './message-clipboard.js';

const SHARE_TARGETS = Object.freeze(['wechat', 'wecom', 'feishu', 'dingtalk', 'qq']);

function menuPosition(trigger, width, height) {
  const rect = trigger.getBoundingClientRect();
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const left = Math.max(6, Math.min(rect.left, viewportWidth - width - 6));
  const below = rect.bottom + 6;
  const top = below + height <= viewportHeight - 6
    ? below
    : Math.max(6, rect.top - height - 6);
  return { left, top, width };
}

export function AssistantMessageFooter({ children }) {
  return (
    <div data-testid="assistant-message-footer" className="!mt-0 flex min-h-8 flex-wrap items-center gap-x-2 gap-y-1 pt-2">
      {children}
    </div>
  );
}

export function AssistantMessageActions({ text, resolveText, copy }) {
  const [copyStatus, setCopyStatus] = useState('idle');
  const [menu, setMenu] = useState(null);
  const [feedback, setFeedback] = useState(null);
  const resetTimerRef = useRef(null);
  const copyLabel = copyStatus === 'copied'
    ? copy.copyReplySuccess
    : copyStatus === 'failed'
      ? copy.copyReplyFailed
      : copy.copyReply;

  const clearResetTimer = useCallback(() => {
    if (!resetTimerRef.current) return;
    clearTimeout(resetTimerRef.current);
    resetTimerRef.current = null;
  }, []);

  const resetStatusLater = useCallback(() => {
    clearResetTimer();
    resetTimerRef.current = setTimeout(() => {
      resetTimerRef.current = null;
      setCopyStatus('idle');
      setFeedback(null);
    }, 2400);
  }, [clearResetTimer]);

  useEffect(() => clearResetTimer, [clearResetTimer]);

  useEffect(() => {
    if (!menu) return undefined;
    const close = event => {
      if (event?.target?.closest?.('[data-assistant-message-action-menu]')) return;
      setMenu(null);
    };
    const onKey = event => { if (event.key === 'Escape') setMenu(null); };
    document.addEventListener('mousedown', close, true);
    document.addEventListener('keydown', onKey, true);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      document.removeEventListener('mousedown', close, true);
      document.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  }, [menu]);

  const responseText = () => normalizeAssistantMessageText(
    typeof resolveText === 'function' ? resolveText() : text,
  );

  const showFeedback = (message, failed = false) => {
    setCopyStatus('idle');
    setFeedback({ message, failed });
    resetStatusLater();
  };

  const handleCopy = async () => {
    let copied = false;
    try {
      const value = responseText();
      copied = await copyClipboardText(value);
    } catch {
      copied = false;
    }
    setFeedback(null);
    setCopyStatus(copied ? 'copied' : 'failed');
    resetStatusLater();
  };

  const openMenu = (kind, event) => {
    event.stopPropagation();
    const dimensions = kind === 'export' ? [236, 126] : [260, 320];
    const position = menuPosition(event.currentTarget, ...dimensions);
    setMenu(current => current?.kind === kind
      ? null
      : { kind, ...position });
  };

  const handleExport = async format => {
    setMenu(null);
    try {
      const generated = buildAssistantResponseExport(responseText(), format, {
        title: copy.exportReplyTitle,
        language: document.documentElement.lang || 'zh-CN',
      });
      const saved = await saveAssistantResponseFile({
        ...generated,
        format,
        filename: assistantExportFilename(format),
      });
      if (saved) showFeedback(copy.exportSuccess);
    } catch {
      showFeedback(copy.exportFailed, true);
    }
  };

  const copyForShare = async () => copyClipboardText(responseText());

  const handleSystemShare = async () => {
    setMenu(null);
    try {
      const value = responseText();
      const result = await shareAssistantResponseWithSystem({
        title: copy.shareReplyTitle,
        text: value,
      });
      if (result === 'cancelled') return;
      if (result === 'shared') {
        showFeedback(copy.shareSuccess);
        return;
      }
      const copied = await copyClipboardText(value);
      showFeedback(copied ? copy.shareUnavailable : copy.shareFailed, !copied);
    } catch {
      const copied = await copyForShare();
      showFeedback(copied ? copy.shareUnavailable : copy.shareFailed, !copied);
    }
  };

  const handleAppShare = async target => {
    setMenu(null);
    const appLabel = copy.shareTargets[target];
    try {
      const copied = await copyForShare();
      if (!copied) {
        showFeedback(copy.shareFailed, true);
        return;
      }
      let opened = false;
      try {
        opened = await openAssistantShareTarget(target);
      } catch {}
      showFeedback(opened ? copy.shareCopiedOpen(appLabel) : copy.shareCopiedWeb(appLabel));
    } catch {
      showFeedback(copy.shareFailed, true);
    }
  };

  const actionClass = 'inline-flex h-8 items-center gap-1.5 rounded-lg px-2 text-[12px] text-[#747775] transition-colors hover:bg-black/[0.06] hover:text-[#1F1F1F] focus:outline-none focus-visible:ring-2 focus-visible:ring-[#0A84FF]/50 dark:text-[#9AA0A6] dark:hover:bg-white/10 dark:hover:text-[#E3E3E3]';
  const menuClass = 'overflow-hidden rounded-xl border border-black/10 bg-white py-1 shadow-xl dark:border-white/10 dark:bg-[#2B2C2F]';
  const menuItemClass = 'flex min-h-11 w-full items-center gap-2.5 px-3 py-2 text-left text-[13px] text-[#1F1F1F] transition-colors hover:bg-black/[0.06] focus:bg-black/[0.06] focus:outline-none dark:text-[#E3E3E3] dark:hover:bg-white/10 dark:focus:bg-white/10';

  return (
    <>
      <div data-testid="assistant-message-actions" className="flex items-center gap-1">
        <button
          type="button"
          data-testid="assistant-message-copy"
          onClick={handleCopy}
          title={copyLabel}
          aria-label={copyLabel}
          className={`${actionClass} ${copyStatus === 'failed' ? '!bg-red-500/[0.08] !text-[#C5221F] dark:!bg-red-400/10 dark:!text-[#F28B82]' : ''}`}
        >
          {copyStatus === 'copied'
            ? <Check size={14} className="text-[#34C759]" />
            : copyStatus === 'failed'
              ? <X size={14} />
              : <Copy size={14} />}
          {copyStatus !== 'idle' && <span aria-live="polite">{copyLabel}</span>}
        </button>
        <button
          type="button"
          data-testid="assistant-message-export"
          onClick={event => openMenu('export', event)}
          title={copy.exportReplyTitle}
          aria-label={copy.exportReplyTitle}
          aria-haspopup="menu"
          aria-expanded={menu?.kind === 'export'}
          className={actionClass}
        >
          <Download size={14} />
          <span>{copy.exportReply}</span>
          <ChevronDown size={11} className={menu?.kind === 'export' ? 'rotate-180' : ''} />
        </button>
        <button
          type="button"
          data-testid="assistant-message-share"
          onClick={event => openMenu('share', event)}
          title={copy.shareReplyTitle}
          aria-label={copy.shareReplyTitle}
          aria-haspopup="menu"
          aria-expanded={menu?.kind === 'share'}
          className={actionClass}
        >
          <Share2 size={14} />
          <span>{copy.shareReply}</span>
          <ChevronDown size={11} className={menu?.kind === 'share' ? 'rotate-180' : ''} />
        </button>
        {feedback && (
          <span
            aria-live="polite"
            className={`max-w-[320px] truncate text-[12px] ${feedback?.failed ? 'text-[#C5221F] dark:text-[#F28B82]' : 'text-[#747775] dark:text-[#9AA0A6]'}`}
            title={feedback?.message || copyLabel}
          >
            {feedback.message}
          </span>
        )}
      </div>
      {menu && createPortal((
        <div
          role="menu"
          data-assistant-message-action-menu
          data-testid={`assistant-message-${menu.kind}-menu`}
          className={menuClass}
          style={{ position: 'fixed', zIndex: 9999, left: menu.left, top: menu.top, width: menu.width }}
          onMouseDown={event => event.stopPropagation()}
        >
          {menu.kind === 'export' ? (
            <>
              <button type="button" role="menuitem" data-testid="assistant-export-md" className={menuItemClass} onClick={() => handleExport('md')}>
                <FileText size={16} className="shrink-0" />
                <span className="min-w-0 flex-1"><span className="block font-medium">{copy.exportMarkdown}</span><span className="block truncate text-[11px] text-[#747775] dark:text-[#9AA0A6]">{copy.exportMarkdownHint}</span></span>
                <Check size={14} className="shrink-0 text-[#0B57D0] dark:text-[#8AB4F8]" />
              </button>
              <button type="button" role="menuitem" data-testid="assistant-export-html" className={menuItemClass} onClick={() => handleExport('html')}>
                <Code size={16} className="shrink-0" />
                <span className="min-w-0"><span className="block font-medium">{copy.exportHtml}</span><span className="block truncate text-[11px] text-[#747775] dark:text-[#9AA0A6]">{copy.exportHtmlHint}</span></span>
              </button>
            </>
          ) : (
            <>
              <button type="button" role="menuitem" data-testid="assistant-share-system" className={menuItemClass} onClick={handleSystemShare}>
                <Share2 size={16} className="shrink-0" />
                <span className="min-w-0"><span className="block font-medium">{copy.shareSystem}</span><span className="block truncate text-[11px] text-[#747775] dark:text-[#9AA0A6]">{copy.shareSystemHint}</span></span>
              </button>
              <div className="mx-3 my-1 border-t border-black/[0.08] dark:border-white/10" />
              <div className="px-3 pb-1 pt-1 text-[11px] font-medium text-[#747775] dark:text-[#9AA0A6]">{copy.shareApps}</div>
              {SHARE_TARGETS.map(target => (
                <button key={target} type="button" role="menuitem" data-testid={`assistant-share-${target}`} className={`${menuItemClass} !min-h-9 !py-1.5`} onClick={() => handleAppShare(target)}>
                  <MessageCircle size={15} className="shrink-0" />
                  <span>{copy.shareTargets[target]}</span>
                </button>
              ))}
            </>
          )}
        </div>
      ), document.body)}
    </>
  );
}
