import { useCallback, useMemo, useState, useSyncExternalStore } from 'react';
import { RefreshCw } from '../../components/icons.jsx';
import { pathBasename } from '../../shared/path-utils.js';
import {
  getSyntaxHighlightVersion,
  highlightCode,
  highlightDiffCode,
  subscribeSyntaxHighlight,
} from '../../shared/syntax-highlighter.js';

// 代码查看内容区：CodeViewerModal（弹窗）与 ReaderApp（独立阅读器窗口）共用，
// 保持两处的高亮、截断、图片、错误态行为一致。字号持久化也在这里共享。
const VIEWER_FONT_SIZE_KEY = 'pinvou_code_viewer_font_size';
const VIEWER_MIN_FONT_SIZE = 10;
const VIEWER_MAX_FONT_SIZE = 24;
const VIEWER_DEFAULT_FONT_SIZE = 12;

export function clampViewerFontSize(value) {
  return Math.max(VIEWER_MIN_FONT_SIZE, Math.min(VIEWER_MAX_FONT_SIZE, Math.round(value)));
}

export function savedViewerFontSize() {
  try {
    const parsed = Number(localStorage.getItem(VIEWER_FONT_SIZE_KEY));
    if (Number.isFinite(parsed) && parsed > 0) return clampViewerFontSize(parsed);
  } catch {
    // localStorage 不可用时回退默认字号。
  }
  return VIEWER_DEFAULT_FONT_SIZE;
}

export function rememberViewerFontSize(fontSize) {
  try {
    localStorage.setItem(VIEWER_FONT_SIZE_KEY, String(fontSize));
  } catch {
    // localStorage 不可用时只保留当前窗口内的字号。
  }
}

export function viewerFontSizeBounds() {
  return { min: VIEWER_MIN_FONT_SIZE, max: VIEWER_MAX_FONT_SIZE };
}

// Font-size state + A-/A+ adjustment: shared by the modal (CodeViewerModal) and the standalone
// reader (ReaderApp), persisted via the same VIEWER_FONT_SIZE_KEY (the two inline copies were identical).
export function useViewerFontSize() {
  const [fontSize, setFontSize] = useState(savedViewerFontSize);
  const adjustViewerFontSize = useCallback((delta) => {
    setFontSize((current) => {
      const next = clampViewerFontSize(current + delta);
      rememberViewerFontSize(next);
      return next;
    });
  }, []);
  return [fontSize, adjustViewerFontSize];
}

// Unified style for the title-bar icon buttons (shared by the modal and reader A-/A+/copy/open/close buttons).
export const CODE_VIEWER_ICON_BUTTON = 'w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] disabled:opacity-40 disabled:hover:bg-transparent';

// 高亮语言提示：优先扩展名（app.jsx → jsx），无扩展名时用完整文件名（Dockerfile / Makefile）。
function languageHintForFile(name) {
  const base = pathBasename(name);
  const dot = base.lastIndexOf('.');
  if (dot > 0) return base.slice(dot + 1).toLowerCase();
  return base.toLowerCase();
}

export function useCodeHighlight(preview, fileName, languageHint) {
  // Lazy language registration bumps the version when done; the version must
  // stay in the useMemo deps (syntax-highlighter.js contract), otherwise code
  // rendered before registration completes stays plain text.
  const syntaxVersion = useSyncExternalStore(subscribeSyntaxHighlight, getSyntaxHighlightVersion);
  return useMemo(() => {
    if (preview?.kind !== 'text' || typeof preview.text !== 'string') return null;
    if (languageHint === 'diff') return highlightDiffCode(preview.text);
    return highlightCode(preview.text, languageHint || languageHintForFile(preview.name || fileName));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- syntaxVersion is a version counter; any change requires recomputing to restore highlighting
  }, [preview, fileName, languageHint, syntaxVersion]);
}

export function CodeViewerContent({
  preview,
  loading = false,
  error = '',
  fontSize,
  highlighted,
  copy,
}) {
  const fileName = preview?.name || 'preview';
  return (
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
          <pre
            className="pinvou-code-block min-w-max px-4 py-3 font-mono whitespace-pre"
            style={{ fontSize: `${fontSize}px`, lineHeight: `${Math.round(fontSize * 1.6)}px` }}
            data-testid="code-viewer-pre"
          >
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
  );
}
