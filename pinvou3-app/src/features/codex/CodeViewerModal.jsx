import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  AppWindow, Check, Copy, ExternalLink, FolderOpen, Link, X,
} from '../../components/icons.jsx';
import { FileColoredIcon } from '../../components/files/FileColoredIcon.jsx';
import {
  CodeViewerContent,
  clampViewerFontSize,
  rememberViewerFontSize,
  savedViewerFontSize,
  useCodeHighlight,
  viewerFontSizeBounds,
} from './CodeViewerContent.jsx';

const VIEWER_SIZE_KEY = 'pinvou_code_viewer_size';
const VIEWER_MIN_WIDTH = 480;
const VIEWER_MIN_HEIGHT = 320;
const VIEWER_DEFAULT_WIDTH = 1100;
const VIEWER_DEFAULT_HEIGHT = 760;

function clampViewerSize(width, height) {
  return {
    width: Math.max(VIEWER_MIN_WIDTH, Math.min(Math.round(width), Math.round(window.innerWidth * 0.95))),
    height: Math.max(VIEWER_MIN_HEIGHT, Math.min(Math.round(height), Math.round(window.innerHeight * 0.95))),
  };
}

function defaultViewerSize() {
  return clampViewerSize(
    Math.min(VIEWER_DEFAULT_WIDTH, window.innerWidth * 0.92),
    Math.min(VIEWER_DEFAULT_HEIGHT, window.innerHeight * 0.85),
  );
}

function savedViewerSize() {
  try {
    const parsed = JSON.parse(localStorage.getItem(VIEWER_SIZE_KEY) || '');
    if (parsed && Number.isFinite(parsed.width) && Number.isFinite(parsed.height)) {
      return clampViewerSize(parsed.width, parsed.height);
    }
  } catch {
    // localStorage 不可用时回退默认尺寸。
  }
  return defaultViewerSize();
}

function rememberViewerSize(size) {
  try {
    localStorage.setItem(VIEWER_SIZE_KEY, JSON.stringify({
      width: Math.round(size.width),
      height: Math.round(size.height),
    }));
  } catch {
    // localStorage 不可用时只保留当前窗口内的尺寸。
  }
}

export function CodeViewerModal({
  name,
  relativePath,
  preview,
  diff,
  loading = false,
  error = '',
  onClose,
  onOpen,
  onReveal,
  onOpenInNewWindow,
  copy,
}) {
  const dialogRef = useRef(null);
  const resizeCleanupRef = useRef(null);
  const [size, setSize] = useState(savedViewerSize);
  const [fontSize, setFontSize] = useState(savedViewerFontSize);
  const [copied, setCopied] = useState('');

  const fileName = preview?.name || name || String(relativePath || '').split('/').pop() || '';

  // diff 模式：把 WorkspaceDiff 适配为文本预览对象，复用 CodeViewerContent 的文本分支
  // （空文本保留旧 PreviewPane 的 noDiff 兜底，diff 为虚拟视图、无文件可定位/打开）。
  const renderPreview = useMemo(
    () => diff
      ? { kind: 'text', text: diff.text || copy.noDiff, truncated: diff.truncated, name: fileName }
      : preview,
    [diff, preview, fileName, copy],
  );

  const highlighted = useCodeHighlight(renderPreview, fileName, diff ? 'diff' : undefined);
  const fontBounds = viewerFontSizeBounds();

  // Esc 关闭 + 打开期间锁定页面滚动。
  useEffect(() => {
    const onKeyDown = (event) => {
      if (event.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', onKeyDown);
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.body.style.overflow = previousOverflow;
    };
  }, [onClose]);

  useEffect(() => () => {
    if (resizeCleanupRef.current) resizeCleanupRef.current();
  }, []);

  useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(''), 1200);
    return () => window.clearTimeout(timer);
  }, [copied]);

  function copyText(target, value) {
    if (!value) return;
    navigator.clipboard?.writeText(value);
    setCopied(target);
  }

  function startViewerResize(direction, event) {
    event.preventDefault();
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (resizeCleanupRef.current) resizeCleanupRef.current();

    const startX = event.clientX;
    const startY = event.clientY;
    const startWidth = dialog.offsetWidth;
    const startHeight = dialog.offsetHeight;
    let nextSize = { width: startWidth, height: startHeight };
    let frame = 0;
    const cursor = direction === 'x' ? 'col-resize' : direction === 'y' ? 'row-resize' : 'nwse-resize';
    const onMove = (moveEvent) => {
      nextSize = clampViewerSize(
        direction === 'y' ? startWidth : startWidth + moveEvent.clientX - startX,
        direction === 'x' ? startHeight : startHeight + moveEvent.clientY - startY,
      );
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        dialog.style.width = `${nextSize.width}px`;
        dialog.style.height = `${nextSize.height}px`;
      });
    };
    const cleanup = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      window.removeEventListener('blur', onUp);
      if (frame) window.cancelAnimationFrame(frame);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      resizeCleanupRef.current = null;
    };
    const onUp = () => {
      cleanup();
      setSize(nextSize);
      rememberViewerSize(nextSize);
    };
    resizeCleanupRef.current = cleanup;
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    window.addEventListener('blur', onUp);
    document.body.style.cursor = cursor;
    document.body.style.userSelect = 'none';
  }

  function resetViewerSize() {
    const nextSize = defaultViewerSize();
    setSize(nextSize);
    rememberViewerSize(nextSize);
  }

  function adjustViewerFontSize(delta) {
    setFontSize((current) => {
      const next = clampViewerFontSize(current + delta);
      rememberViewerFontSize(next);
      return next;
    });
  }

  const iconButton = 'w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-40 disabled:hover:bg-transparent';

  return createPortal(
    <div data-testid="code-viewer-modal" className="fixed inset-0 z-[300] flex items-center justify-center">
      <div className="absolute inset-0 bg-black/30 backdrop-blur-[1px]" onClick={onClose} />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={fileName}
        style={{ width: size.width, height: size.height }}
        className="relative flex flex-col overflow-hidden rounded-2xl border border-black/10 dark:border-white/10 bg-white dark:bg-[#1E1E20] text-gray-900 dark:text-gray-100 shadow-2xl"
      >
        <div className="h-12 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
          <FileColoredIcon name={fileName} size={15} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-[13px] font-medium" title={fileName}>{fileName}</span>
              {highlighted?.label && (
                <span className="shrink-0 rounded-md bg-black/[0.05] dark:bg-white/[0.08] px-1.5 py-0.5 text-[9px] font-medium text-gray-500 dark:text-gray-300">
                  {highlighted.label}
                </span>
              )}
            </div>
            <div className="truncate text-[10px] text-gray-400" title={relativePath}>{relativePath}</div>
          </div>
          <button
            type="button"
            onClick={() => adjustViewerFontSize(-1)}
            disabled={fontSize <= fontBounds.min}
            className={`${iconButton} text-[12px] font-medium tracking-tight`}
            title={copy.fontDecrease}
            aria-label={copy.fontDecrease}
            data-testid="code-viewer-font-decrease"
          >
            A-
          </button>
          <button
            type="button"
            onClick={() => adjustViewerFontSize(1)}
            disabled={fontSize >= fontBounds.max}
            className={`${iconButton} text-[12px] font-medium tracking-tight`}
            title={copy.fontIncrease}
            aria-label={copy.fontIncrease}
            data-testid="code-viewer-font-increase"
          >
            A+
          </button>
          <button
            type="button"
            onClick={() => copyText('content', renderPreview?.kind === 'text' ? renderPreview.text : '')}
            disabled={renderPreview?.kind !== 'text'}
            className={iconButton}
            title={copied === 'content' ? copy.copied : copy.copyContent}
          >
            {copied === 'content' ? <Check size={13} className="text-emerald-500" /> : <Copy size={13} />}
          </button>
          <button
            type="button"
            onClick={() => copyText('path', relativePath)}
            className={iconButton}
            title={copied === 'path' ? copy.copied : copy.copyPath}
          >
            {copied === 'path' ? <Check size={13} className="text-emerald-500" /> : <Link size={13} />}
          </button>
          {!diff && onReveal && (
            <button type="button" onClick={onReveal} className={iconButton} title={copy.reveal}>
              <FolderOpen size={13} />
            </button>
          )}
          {!diff && onOpen && (
            <button type="button" onClick={onOpen} className={iconButton} title={copy.open}>
              <ExternalLink size={13} />
            </button>
          )}
          {onOpenInNewWindow && (
            <button
              type="button"
              onClick={onOpenInNewWindow}
              className={iconButton}
              aria-label={copy.openInNewWindow}
              title={copy.openInNewWindow}
              data-testid="code-viewer-open-in-new-window"
            >
              <AppWindow size={13} />
            </button>
          )}
          <button type="button" onClick={onClose} className={iconButton} aria-label={copy.closeViewer} title={copy.closeViewer}>
            <X size={14} />
          </button>
        </div>

        <CodeViewerContent
          preview={renderPreview}
          loading={loading}
          error={error}
          fontSize={fontSize}
          highlighted={highlighted}
          copy={copy}
        />

        <div
          role="separator"
          aria-label={copy.resizeWidth}
          aria-orientation="vertical"
          data-testid="code-viewer-resize-x"
          onMouseDown={(event) => startViewerResize('x', event)}
          className="absolute inset-y-0 right-0 z-10 w-1.5 cursor-col-resize hover:bg-[#0B57D0]/40 dark:hover:bg-[#A8C7FA]/50 transition-colors"
          title={copy.resizeWidth}
        />
        <div
          role="separator"
          aria-label={copy.resizeHeight}
          aria-orientation="horizontal"
          data-testid="code-viewer-resize-y"
          onMouseDown={(event) => startViewerResize('y', event)}
          className="absolute inset-x-0 bottom-0 z-10 h-1.5 cursor-row-resize hover:bg-[#0B57D0]/40 dark:hover:bg-[#A8C7FA]/50 transition-colors"
          title={copy.resizeHeight}
        />
        <div
          role="separator"
          aria-label={copy.resizeCorner}
          data-testid="code-viewer-resize-xy"
          onMouseDown={(event) => startViewerResize('xy', event)}
          onDoubleClick={resetViewerSize}
          className="absolute bottom-0 right-0 z-20 w-4 h-4 cursor-nwse-resize"
          title={copy.resizeCorner}
        />
      </div>
    </div>,
    document.body,
  );
}
