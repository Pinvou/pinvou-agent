import React, { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import {
  Check, Copy, ExternalLink, FolderOpen, RefreshCw, X,
} from '../../components/icons.jsx';
import { FileColoredIcon } from '../../components/files/FileColoredIcon.jsx';
import { highlightCode } from '../../shared/syntax-highlighter.js';

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

// 高亮语言提示：优先扩展名（app.jsx → jsx），无扩展名时用完整文件名（Dockerfile / Makefile）。
function languageHintForFile(name) {
  const base = String(name || '').split(/[\\/]/u).pop() || '';
  const dot = base.lastIndexOf('.');
  if (dot > 0) return base.slice(dot + 1).toLowerCase();
  return base.toLowerCase();
}

export function CodeViewerModal({
  name,
  relativePath,
  preview,
  loading = false,
  error = '',
  onClose,
  onOpen,
  onReveal,
  copy,
}) {
  const dialogRef = useRef(null);
  const resizeCleanupRef = useRef(null);
  const [size, setSize] = useState(savedViewerSize);
  const [copied, setCopied] = useState('');

  const fileName = preview?.name || name || String(relativePath || '').split('/').pop() || '';

  const highlighted = useMemo(() => {
    if (preview?.kind !== 'text' || typeof preview.text !== 'string') return null;
    return highlightCode(preview.text, languageHintForFile(preview.name || fileName));
  }, [preview, fileName]);

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
        className="relative flex flex-col overflow-hidden rounded-2xl border border-black/10 dark:border-white/10 bg-white dark:bg-[#1E1E20] shadow-2xl"
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
            onClick={() => copyText('content', preview?.kind === 'text' ? preview.text : '')}
            disabled={preview?.kind !== 'text'}
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
            {copied === 'path' ? <Check size={13} className="text-emerald-500" /> : <Copy size={13} />}
          </button>
          <button type="button" onClick={onReveal} className={iconButton} title={copy.reveal}>
            <FolderOpen size={13} />
          </button>
          <button type="button" onClick={onOpen} className={iconButton} title={copy.open}>
            <ExternalLink size={13} />
          </button>
          <button type="button" onClick={onClose} className={iconButton} aria-label={copy.closeViewer} title={copy.closeViewer}>
            <X size={14} />
          </button>
        </div>

        <div data-testid="code-viewer-body" className="code-viewer-body flex-1 min-h-0 flex flex-col">
          {loading ? (
            <div className="flex-1 flex items-center justify-center text-[12px] text-gray-400">
              <RefreshCw size={15} className="mr-2 animate-spin" />{copy.reading}
            </div>
          ) : error ? (
            <div className="flex-1 flex flex-col items-center justify-center gap-2 px-6 text-center">
              <div className="text-[12px] text-red-600 dark:text-red-300">{copy.loadFailed}</div>
              <div className="text-[10px] leading-4 text-gray-400 break-all">{error}</div>
            </div>
          ) : preview?.kind === 'text' && highlighted ? (
            <div className="flex-1 min-h-0 overflow-auto custom-scrollbar">
              {preview.truncated && (
                <div className="sticky top-0 z-10 px-3 py-2 text-[10px] leading-4 text-amber-600 dark:text-amber-300 bg-amber-50/95 dark:bg-amber-950/80 border-b border-amber-200/40 dark:border-amber-500/20">
                  {copy.truncated}
                </div>
              )}
              <pre className="pinvou-code-block min-w-max px-4 py-3 text-[12px] leading-[19px] font-mono whitespace-pre">
                <code
                  className={`hljs language-${highlighted.language}`}
                  dangerouslySetInnerHTML={{ __html: highlighted.html }}
                />
              </pre>
            </div>
          ) : preview?.kind === 'image' && preview.dataUrl ? (
            <div className="flex-1 min-h-0 overflow-auto custom-scrollbar p-4">
              <img
                src={preview.dataUrl}
                alt={fileName}
                className="mx-auto max-w-full h-auto rounded-lg border border-black/[0.06] dark:border-white/[0.08]"
              />
            </div>
          ) : (
            <div className="flex-1 flex items-center justify-center p-6">
              <div className="text-center text-[12px] leading-5 text-gray-400">
                {copy.unsupported}
                <br />{copy.openHint}
              </div>
            </div>
          )}
        </div>

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
