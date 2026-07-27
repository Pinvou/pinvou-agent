import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft, ChevronDown, ChevronRight, Copy, ExternalLink, FileText,
  FolderOpen, Plus, RefreshCw, Search, X,
} from '../../components/icons.jsx';
import { invokeTauri } from '../../platform/tauri/client.js';

const invoke = invokeTauri;
const WORKSPACE_WIDTH_KEY = 'pinvou_codex_workspace_width';
const WORKSPACE_MIN_WIDTH = 360;
const CONVERSATION_MIN_WIDTH = 360;
const WORKSPACE_MAX_RATIO = 0.65;
const WORKSPACE_DEFAULT_WIDTH = 380;

function clampWorkspaceWidth(width, rootWidth) {
  const maximum = Math.max(
    WORKSPACE_MIN_WIDTH,
    Math.min(
      Math.round(rootWidth * WORKSPACE_MAX_RATIO),
      rootWidth - CONVERSATION_MIN_WIDTH,
    ),
  );
  return Math.max(WORKSPACE_MIN_WIDTH, Math.min(Math.round(width), maximum));
}

function savedWorkspaceWidth() {
  try {
    const value = Number.parseInt(localStorage.getItem(WORKSPACE_WIDTH_KEY) || '', 10);
    return Number.isFinite(value) && value >= WORKSPACE_MIN_WIDTH
      ? value
      : WORKSPACE_DEFAULT_WIDTH;
  } catch {
    return WORKSPACE_DEFAULT_WIDTH;
  }
}

function rememberWorkspaceWidth(width) {
  try {
    localStorage.setItem(WORKSPACE_WIDTH_KEY, String(Math.round(width)));
  } catch {
    // localStorage 不可用时只保留当前窗口内的宽度。
  }
}

function formatBytes(bytes) {
  const value = Number(bytes || 0);
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

function changeLabel(status) {
  return {
    added: '新增',
    modified: '修改',
    deleted: '删除',
    renamed: '重命名',
    copied: '复制',
    conflict: '冲突',
    untracked: '未跟踪',
    unknown: '文件',
  }[status] || status;
}

function statusTone(status) {
  if (['added', 'untracked'].includes(status)) return 'text-emerald-600 dark:text-emerald-300 bg-emerald-500/10';
  if (status === 'deleted') return 'text-red-600 dark:text-red-300 bg-red-500/10';
  if (status === 'conflict') return 'text-orange-600 dark:text-orange-300 bg-orange-500/10';
  return 'text-amber-600 dark:text-amber-300 bg-amber-500/10';
}

function originLabel(origin) {
  if (origin === 'session') return '本会话';
  if (origin === 'preexisting') return '会话前已有';
  if (origin === 'preexisting_modified') return '会话前已有 · 本会话继续修改';
  return '来源未记录';
}

function WorkspaceTree({
  directory = '',
  depth = 0,
  entriesByDirectory,
  expanded,
  loadingDirectories,
  onToggle,
  onPreview,
  onAddReference,
  referencedPaths,
}) {
  const entries = entriesByDirectory[directory] || [];
  return entries.map(entry => {
    const isDirectory = entry.kind === 'directory';
    const open = expanded.has(entry.relativePath);
    const referenced = referencedPaths.has(entry.relativePath);
    return (
      <React.Fragment key={entry.relativePath}>
        <div
          className="group h-8 flex items-center gap-1.5 rounded-lg pr-1 hover:bg-black/[0.04] dark:hover:bg-white/[0.05]"
          style={{ paddingLeft: 6 + depth * 14 }}
        >
          <button
            type="button"
            className="min-w-0 flex-1 h-full flex items-center gap-1.5 text-left"
            onClick={() => isDirectory ? onToggle(entry) : onPreview(entry)}
            title={entry.relativePath}
          >
            <span className="w-3.5 shrink-0 text-gray-400">
              {isDirectory && entry.hasChildren
                ? loadingDirectories.has(entry.relativePath)
                  ? <RefreshCw size={12} className="animate-spin" />
                  : open ? <ChevronDown size={12} /> : <ChevronRight size={12} />
                : null}
            </span>
            {isDirectory
              ? <FolderOpen size={14} className="shrink-0 text-blue-500" />
              : <FileText size={14} className="shrink-0 text-gray-400" />}
            <span className="truncate text-[12px]">{entry.name}</span>
          </button>
          {!isDirectory && (
            <button
              type="button"
              aria-label={referenced ? `已添加 ${entry.relativePath}` : `添加 ${entry.relativePath} 到对话`}
              title={referenced ? '已添加到对话' : '添加到对话'}
              onClick={() => onAddReference(entry.relativePath)}
              className={`w-6 h-6 shrink-0 rounded-md flex items-center justify-center transition-opacity ${
                referenced
                  ? 'text-blue-500 bg-blue-500/10'
                  : 'text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
              }`}
            >
              <Plus size={13} />
            </button>
          )}
        </div>
        {isDirectory && open && (
          <WorkspaceTree
            directory={entry.relativePath}
            depth={depth + 1}
            entriesByDirectory={entriesByDirectory}
            expanded={expanded}
            loadingDirectories={loadingDirectories}
            onToggle={onToggle}
            onPreview={onPreview}
            onAddReference={onAddReference}
            referencedPaths={referencedPaths}
          />
        )}
      </React.Fragment>
    );
  });
}

function PreviewPane({ preview, diff, loading, onBack, onAddReference, referenced, onOpen, onReveal }) {
  const item = preview || diff;
  if (!item) return null;
  const path = item.relativePath;
  return (
    <div className="h-full min-h-0 flex flex-col">
      <div className="h-11 shrink-0 px-2 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        <button type="button" onClick={onBack} className="w-7 h-7 rounded-lg flex items-center justify-center hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" aria-label="返回工作区列表">
          <ArrowLeft size={14} />
        </button>
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-medium" title={path}>{path}</div>
          {preview && <div className="text-[10px] text-gray-400">{formatBytes(preview.size)}</div>}
        </div>
        <button type="button" onClick={() => navigator.clipboard?.writeText(path)} className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" title="复制相对路径">
          <Copy size={13} />
        </button>
        <button type="button" onClick={onReveal} className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" title="在文件管理器中显示">
          <FolderOpen size={13} />
        </button>
        <button type="button" onClick={onOpen} className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" title="用系统应用打开">
          <ExternalLink size={13} />
        </button>
      </div>
      {loading ? (
        <div className="flex-1 flex items-center justify-center text-[11px] text-gray-400">
          <RefreshCw size={14} className="mr-2 animate-spin" />正在读取…
        </div>
      ) : (
        <div className="flex-1 min-h-0 overflow-auto custom-scrollbar">
          {diff && (
            <pre className="min-w-max p-3 text-[11px] leading-[18px] font-mono whitespace-pre">
              {diff.text || '没有可显示的文本差异'}
            </pre>
          )}
          {preview?.kind === 'image' && preview.dataUrl && (
            <div className="p-4 flex justify-center">
              <img src={preview.dataUrl} alt={preview.name} className="max-w-full h-auto rounded-lg border border-black/[0.06] dark:border-white/[0.08]" />
            </div>
          )}
          {preview?.kind === 'text' && (
            <pre className="min-w-max p-3 text-[11px] leading-[18px] font-mono whitespace-pre">{preview.text}</pre>
          )}
          {preview && preview.kind !== 'text' && !(preview.kind === 'image' && preview.dataUrl) && (
            <div className="p-6 text-center text-[11px] leading-5 text-gray-400">
              {preview.truncated ? '文件过大，未生成内置预览。' : '该文件不支持内置预览。'}
              <br />可以用系统应用打开。
            </div>
          )}
          {(preview?.truncated || diff?.truncated) && (
            <div className="sticky bottom-0 px-3 py-2 text-[10px] text-amber-600 dark:text-amber-300 bg-amber-50/95 dark:bg-amber-950/80">
              内容过大，当前只显示前一部分。
            </div>
          )}
        </div>
      )}
      <div className="shrink-0 p-2 border-t border-black/[0.05] dark:border-white/[0.06]">
        <button
          type="button"
          onClick={() => onAddReference(path)}
          className={`w-full h-8 rounded-lg inline-flex items-center justify-center gap-1.5 text-[11px] font-medium ${
            referenced
              ? 'bg-blue-500/10 text-blue-600 dark:text-blue-300'
              : 'bg-[#007AFF] text-white hover:bg-[#006EE6]'
          }`}
        >
          <Plus size={13} />{referenced ? '已添加到对话' : '添加到对话'}
        </button>
      </div>
    </div>
  );
}

export function CodexWorkspacePanel({
  session,
  visible,
  onClose,
  references = [],
  onAddReference,
  refreshToken = 0,
  onChangeCount,
}) {
  const sessionId = session?.id;
  const [tab, setTab] = useState('files');
  const [entriesByDirectory, setEntriesByDirectory] = useState({});
  const [expanded, setExpanded] = useState(new Set());
  const [loadingDirectories, setLoadingDirectories] = useState(new Set());
  const [query, setQuery] = useState('');
  const [searchResults, setSearchResults] = useState([]);
  const [searching, setSearching] = useState(false);
  const [changes, setChanges] = useState(null);
  const [preview, setPreview] = useState(null);
  const [diff, setDiff] = useState(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [error, setError] = useState('');
  const [panelWidth, setPanelWidth] = useState(savedWorkspaceWidth);
  const panelRef = useRef(null);
  const resizeCleanupRef = useRef(null);
  const referencedPaths = useMemo(() => new Set(references), [references]);

  useEffect(() => {
    if (!visible) return undefined;
    const clampToViewport = () => {
      const panel = panelRef.current;
      const rootWidth = panel?.parentElement?.getBoundingClientRect().width || window.innerWidth;
      setPanelWidth(current => clampWorkspaceWidth(current, rootWidth));
    };
    clampToViewport();
    window.addEventListener('resize', clampToViewport);
    return () => window.removeEventListener('resize', clampToViewport);
  }, [visible]);

  useEffect(() => () => {
    if (resizeCleanupRef.current) resizeCleanupRef.current();
  }, []);

  function startPanelResize(event) {
    event.preventDefault();
    const panel = panelRef.current;
    const rootRect = panel?.parentElement?.getBoundingClientRect();
    if (!panel || !rootRect) return;
    if (resizeCleanupRef.current) resizeCleanupRef.current();

    const maximum = Math.max(
      WORKSPACE_MIN_WIDTH,
      Math.min(
        Math.round(rootRect.width * WORKSPACE_MAX_RATIO),
        rootRect.width - CONVERSATION_MIN_WIDTH,
      ),
    );
    let nextWidth = panelWidth;
    let frame = 0;
    const onMove = moveEvent => {
      nextWidth = Math.max(
        WORKSPACE_MIN_WIDTH,
        Math.min(rootRect.right - moveEvent.clientX, maximum),
      );
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        panel.style.width = `${nextWidth}px`;
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
      setPanelWidth(nextWidth);
      rememberWorkspaceWidth(nextWidth);
    };
    resizeCleanupRef.current = cleanup;
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
    window.addEventListener('blur', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  function resetPanelWidth() {
    const panel = panelRef.current;
    const rootWidth = panel?.parentElement?.getBoundingClientRect().width || window.innerWidth;
    const nextWidth = clampWorkspaceWidth(WORKSPACE_DEFAULT_WIDTH, rootWidth);
    setPanelWidth(nextWidth);
    rememberWorkspaceWidth(nextWidth);
  }

  async function loadDirectory(path = '', { force = false } = {}) {
    if (!sessionId || (!force && entriesByDirectory[path])) return;
    setLoadingDirectories(current => new Set([...current, path]));
    try {
      const listing = await invoke('list_codex_workspace', {
        sessionId,
        relativePath: path || null,
      });
      setEntriesByDirectory(current => ({ ...current, [path]: listing.entries || [] }));
      setError('');
    } catch (nextError) {
      setError(String(nextError));
    } finally {
      setLoadingDirectories(current => {
        const next = new Set(current);
        next.delete(path);
        return next;
      });
    }
  }

  async function loadChanges() {
    if (!sessionId) return;
    try {
      const result = await invoke('get_codex_workspace_changes', { sessionId });
      setChanges(result);
      if (onChangeCount) onChangeCount((result.changes || []).length);
      setError('');
    } catch (nextError) {
      setError(String(nextError));
      if (onChangeCount) onChangeCount(0);
    }
  }

  useEffect(() => {
    setEntriesByDirectory({});
    setExpanded(new Set());
    setQuery('');
    setSearchResults([]);
    setChanges(null);
    setPreview(null);
    setDiff(null);
    setError('');
    if (sessionId) {
      loadDirectory('', { force: true });
      loadChanges();
    } else if (onChangeCount) {
      onChangeCount(0);
    }
  }, [sessionId]);

  useEffect(() => {
    if (!sessionId || !refreshToken) return;
    const timer = window.setTimeout(() => {
      loadChanges();
      if (visible && tab === 'files') loadDirectory('', { force: true });
    }, 350);
    return () => window.clearTimeout(timer);
  }, [refreshToken, sessionId]);

  useEffect(() => {
    if (!sessionId || !query.trim()) {
      setSearchResults([]);
      setSearching(false);
      return undefined;
    }
    setSearching(true);
    const timer = window.setTimeout(async () => {
      try {
        const results = await invoke('search_codex_workspace', {
          sessionId,
          query: query.trim(),
        });
        setSearchResults(results || []);
        setError('');
      } catch (nextError) {
        setError(String(nextError));
      } finally {
        setSearching(false);
      }
    }, 250);
    return () => window.clearTimeout(timer);
  }, [query, sessionId]);

  async function toggleDirectory(entry) {
    const path = entry.relativePath;
    const willOpen = !expanded.has(path);
    setExpanded(current => {
      const next = new Set(current);
      if (willOpen) next.add(path);
      else next.delete(path);
      return next;
    });
    if (willOpen) await loadDirectory(path);
  }

  async function showFile(entry) {
    setPreview(null);
    setDiff(null);
    setPreviewLoading(true);
    try {
      setPreview(await invoke('preview_codex_workspace_file', {
        sessionId,
        relativePath: entry.relativePath,
      }));
      setError('');
    } catch (nextError) {
      setError(String(nextError));
    } finally {
      setPreviewLoading(false);
    }
  }

  async function showDiff(change) {
    setPreview(null);
    setDiff({ relativePath: change.relativePath, text: '' });
    setPreviewLoading(true);
    try {
      setDiff(await invoke('get_codex_workspace_diff', {
        sessionId,
        relativePath: change.relativePath,
      }));
      setError('');
    } catch (nextError) {
      setError(String(nextError));
    } finally {
      setPreviewLoading(false);
    }
  }

  async function openSelected(command) {
    const path = (preview || diff)?.relativePath;
    if (!path) return;
    try {
      await invoke(command, { sessionId, relativePath: path });
    } catch (nextError) {
      setError(String(nextError));
    }
  }

  const selected = preview || diff;
  const rows = query.trim() ? searchResults : null;

  return (
    <aside
      ref={panelRef}
      style={{ width: `${panelWidth}px` }}
      className={`${visible ? 'flex' : 'hidden'} relative max-w-[88vw] min-w-0 shrink-0 border-l border-black/[0.06] dark:border-white/[0.07] bg-white/92 dark:bg-[#17181A]/96 backdrop-blur-xl flex-col`}
    >
      <div
        role="separator"
        aria-label="调整工作区宽度"
        aria-orientation="vertical"
        onMouseDown={startPanelResize}
        onDoubleClick={resetPanelWidth}
        className="absolute inset-y-0 left-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize bg-black/10 hover:bg-[#0B57D0]/50 dark:bg-white/10 dark:hover:bg-[#A8C7FA]/60 transition-colors"
        title="拖拽调整宽度，双击恢复默认"
      />
      <div className="h-14 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">工作区</div>
          <div className="truncate text-[10px] text-gray-400" title={session?.workspace_path}>
            {session?.workspace_kind === 'temporary' ? '临时工作区' : session?.workspace_path}
          </div>
        </div>
        <button
          type="button"
          onClick={() => {
            loadDirectory('', { force: true });
            loadChanges();
          }}
          className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
          title="刷新工作区"
        >
          <RefreshCw size={14} />
        </button>
        <button type="button" onClick={onClose} className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" aria-label="关闭工作区">
          <X size={14} />
        </button>
      </div>

      {selected ? (
        <div className="flex-1 min-h-0">
          <PreviewPane
            preview={preview}
            diff={diff}
            loading={previewLoading}
            onBack={() => { setPreview(null); setDiff(null); }}
            onAddReference={onAddReference}
            referenced={referencedPaths.has(selected.relativePath)}
            onOpen={() => openSelected('open_codex_workspace_file')}
            onReveal={() => openSelected('reveal_codex_workspace_file')}
          />
        </div>
      ) : (
        <>
          <div className="shrink-0 px-3 pt-2">
            <div className="grid grid-cols-2 rounded-lg bg-black/[0.035] dark:bg-white/[0.055] p-0.5">
              <button type="button" onClick={() => setTab('files')} className={`h-7 rounded-md text-[11px] ${tab === 'files' ? 'bg-white dark:bg-white/10 shadow-sm font-medium' : 'text-gray-400'}`}>
                文件
              </button>
              <button type="button" onClick={() => { setTab('changes'); loadChanges(); }} className={`h-7 rounded-md text-[11px] ${tab === 'changes' ? 'bg-white dark:bg-white/10 shadow-sm font-medium' : 'text-gray-400'}`}>
                更改{changes?.changes?.length ? ` ${changes.changes.length}` : ''}
              </button>
            </div>
          </div>
          {error && <div className="mx-3 mt-2 rounded-lg bg-red-500/8 px-2.5 py-2 text-[10px] leading-4 text-red-600 dark:text-red-300">{error}</div>}

          {tab === 'files' ? (
            <>
              <div className="shrink-0 px-3 py-2">
                <div className="h-8 px-2.5 rounded-lg bg-black/[0.035] dark:bg-white/[0.055] flex items-center gap-2">
                  <Search size={13} className="text-gray-400" />
                  <input
                    value={query}
                    onChange={event => setQuery(event.target.value)}
                    placeholder="搜索文件"
                    className="min-w-0 flex-1 bg-transparent outline-none text-[11px] placeholder:text-gray-400"
                  />
                  {searching && <RefreshCw size={12} className="animate-spin text-gray-400" />}
                  {query && <button type="button" onClick={() => setQuery('')} className="text-gray-400"><X size={12} /></button>}
                </div>
              </div>
              <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-2 pb-3">
                {rows ? rows.map(entry => (
                  <div key={entry.relativePath} className="group h-9 px-2 flex items-center gap-2 rounded-lg hover:bg-black/[0.04] dark:hover:bg-white/[0.05]">
                    <button type="button" onClick={() => showFile(entry)} className="min-w-0 flex-1 flex items-center gap-2 text-left" title={entry.relativePath}>
                      <FileText size={14} className="shrink-0 text-gray-400" />
                      <span className="min-w-0">
                        <span className="block truncate text-[11px]">{entry.name}</span>
                        <span className="block truncate text-[9px] text-gray-400">{entry.relativePath}</span>
                      </span>
                    </button>
                    <button type="button" onClick={() => onAddReference(entry.relativePath)} className={`w-6 h-6 rounded-md flex items-center justify-center ${referencedPaths.has(entry.relativePath) ? 'text-blue-500 bg-blue-500/10' : 'opacity-0 group-hover:opacity-100 text-gray-400'}`} title="添加到对话">
                      <Plus size={13} />
                    </button>
                  </div>
                )) : (
                  <WorkspaceTree
                    entriesByDirectory={entriesByDirectory}
                    expanded={expanded}
                    loadingDirectories={loadingDirectories}
                    onToggle={toggleDirectory}
                    onPreview={showFile}
                    onAddReference={onAddReference}
                    referencedPaths={referencedPaths}
                  />
                )}
                {!searching && rows && rows.length === 0 && (
                  <div className="py-10 text-center text-[11px] text-gray-400">没有匹配文件</div>
                )}
              </div>
            </>
          ) : (
            <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-2 py-3">
              {!changes?.baselineAvailable && (
                <div className="mx-1 mb-2 rounded-lg bg-amber-500/8 px-2.5 py-2 text-[10px] leading-4 text-amber-700 dark:text-amber-300">
                  该旧会话没有创建时基线，因此无法判断更改是否由本会话产生。
                </div>
              )}
              {changes?.branch && <div className="px-2 pb-2 text-[10px] text-gray-400">分支 · {changes.branch}</div>}
              {(changes?.changes || []).map(change => (
                <button
                  key={`${change.status}:${change.relativePath}`}
                  type="button"
                  onClick={() => showDiff(change)}
                  className="w-full min-h-11 px-2 py-1.5 rounded-lg flex items-center gap-2 text-left hover:bg-black/[0.04] dark:hover:bg-white/[0.05]"
                >
                  <span className={`min-w-10 h-5 px-1.5 rounded-md inline-flex items-center justify-center text-[9px] font-medium ${statusTone(change.status)}`}>
                    {changeLabel(change.status)}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[11px]" title={change.relativePath}>{change.relativePath}</span>
                    <span className="block mt-0.5 truncate text-[9px] text-gray-400">{originLabel(change.origin)}{change.staged ? ' · 已暂存' : ''}</span>
                  </span>
                  <ChevronRight size={12} className="shrink-0 text-gray-400" />
                </button>
              ))}
              {changes && changes.changes.length === 0 && (
                <div className="py-12 text-center text-[11px] text-gray-400">工作区没有更改</div>
              )}
            </div>
          )}
        </>
      )}
    </aside>
  );
}
