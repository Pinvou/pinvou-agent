import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  AppWindow, Check, ChevronDown, ChevronRight, ExternalLink, FileText,
  Link, Plus, RefreshCw, Search, X,
} from '../../components/icons.jsx';
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  listAcpWorkspace,
  previewAcpWorkspaceFile,
  searchAcpWorkspace,
} from '../../platform/acp/client.js';
import { FileColoredIcon } from '../../components/files/FileColoredIcon.jsx';
import { CodeViewerModal } from './CodeViewerModal.jsx';
import { can } from '../../shared/platform.js';

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

function changeLabel(status, copy) {
  return copy.changes[status] || status;
}

function statusTone(status) {
  if (['added', 'untracked'].includes(status)) return 'text-emerald-600 dark:text-emerald-300 bg-emerald-500/10';
  if (status === 'deleted') return 'text-red-600 dark:text-red-300 bg-red-500/10';
  if (status === 'conflict') return 'text-orange-600 dark:text-orange-300 bg-orange-500/10';
  return 'text-amber-600 dark:text-amber-300 bg-amber-500/10';
}

function originLabel(origin, copy) {
  return copy.origins[origin] || copy.origins.unknown;
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
  onOpenExternal,
  onOpenReader,
  systemOpenAvailable,
  referencedPaths,
  copy,
}) {
  const entries = entriesByDirectory[directory] || [];
  const [copiedPath, setCopiedPath] = useState('');

  function copyRowPath(relativePath) {
    navigator.clipboard?.writeText(relativePath);
    setCopiedPath(relativePath);
    window.setTimeout(() => {
      setCopiedPath(current => (current === relativePath ? '' : current));
    }, 1200);
  }

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
            <FileColoredIcon name={entry.name} isDir={isDirectory} isOpen={open} size={14} />
            <span className="truncate text-[12px]">{entry.name}</span>
          </button>
          {!isDirectory && systemOpenAvailable && (
            <button
              type="button"
              aria-label={copy.openInNewWindow}
              title={copy.openInNewWindow}
              onClick={() => onOpenReader(entry)}
              className="w-6 h-6 shrink-0 rounded-md flex items-center justify-center text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] transition-opacity"
            >
              <AppWindow size={13} />
            </button>
          )}
          <button
            type="button"
            aria-label={copiedPath === entry.relativePath ? copy.copied : copy.copyPath}
            title={copiedPath === entry.relativePath ? copy.copied : copy.copyPath}
            onClick={() => copyRowPath(entry.relativePath)}
            className="w-6 h-6 shrink-0 rounded-md flex items-center justify-center text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] transition-opacity"
          >
            {copiedPath === entry.relativePath
              ? <Check size={13} className="text-emerald-500" />
              : <Link size={13} />}
          </button>
          {!isDirectory && (
            <>
              <button
                type="button"
                aria-label={referenced ? copy.addedPath(entry.relativePath) : copy.addPath(entry.relativePath)}
                title={referenced ? copy.added : copy.add}
                onClick={() => onAddReference(entry.relativePath)}
                className={`w-6 h-6 shrink-0 rounded-md flex items-center justify-center transition-opacity ${
                  referenced
                    ? 'text-blue-500 bg-blue-500/10'
                    : 'text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
                }`}
              >
                <Plus size={13} />
              </button>
              {systemOpenAvailable && (
                <button
                  type="button"
                  aria-label={copy.open}
                  title={copy.open}
                  onClick={() => onOpenExternal(entry)}
                  className="w-6 h-6 shrink-0 rounded-md flex items-center justify-center text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07] transition-opacity"
                >
                  <ExternalLink size={13} />
                </button>
              )}
            </>
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
            onOpenExternal={onOpenExternal}
            onOpenReader={onOpenReader}
            systemOpenAvailable={systemOpenAvailable}
            referencedPaths={referencedPaths}
            copy={copy}
          />
        )}
      </React.Fragment>
    );
  });
}

export function CodexWorkspacePanel({
  session,
  workspacePath = '',
  visible,
  onClose,
  references = [],
  onAddReference,
  refreshToken = 0,
  onChangeCount,
  copy,
}) {
  const sessionId = session?.id;
  // 会话前（draft）模式：无 sessionId，直接按项目路径浏览；变更/差异依赖会话基线，仅会话内可用。
  const browsePath = sessionId ? '' : String(workspacePath || '');
  const browsable = Boolean(sessionId || browsePath);
  const scopePayload = () => (sessionId ? { sessionId } : { workspacePath: browsePath });
  const [tab, setTab] = useState('files');
  const [entriesByDirectory, setEntriesByDirectory] = useState({});
  const [expanded, setExpanded] = useState(new Set());
  const [loadingDirectories, setLoadingDirectories] = useState(new Set());
  const [query, setQuery] = useState('');
  const [searchResults, setSearchResults] = useState([]);
  const [searching, setSearching] = useState(false);
  const [changes, setChanges] = useState(null);
  const [viewer, setViewer] = useState(null);
  const [error, setError] = useState('');
  const previewRequestRef = useRef(0);
  const showError = (nextError) => {
    console.error('Codex workspace operation failed:', nextError);
    setError(copy.showRawErrors ? String(nextError) : copy.operationFailed);
  };
  const [panelWidth, setPanelWidth] = useState(savedWorkspaceWidth);
  const panelRef = useRef(null);
  const resizeCleanupRef = useRef(null);
  const referencedPaths = useMemo(() => new Set(references), [references]);
  const systemOpenAvailable = can('externalSystemOpen');

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
    if (!browsable || (!force && entriesByDirectory[path])) return;
    setLoadingDirectories(current => new Set([...current, path]));
    try {
      const listing = await listAcpWorkspace({
        ...scopePayload(),
        relativePath: path || null,
      });
      setEntriesByDirectory(current => ({ ...current, [path]: listing.entries || [] }));
      setError('');
    } catch (nextError) {
      showError(nextError);
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
      showError(nextError);
      if (onChangeCount) onChangeCount(0);
    }
  }

  useEffect(() => {
    // 工作区切换：作废在途预览响应（见 showFile 的序号校验）。
    previewRequestRef.current += 1;
    setEntriesByDirectory({});
    setExpanded(new Set());
    setQuery('');
    setSearchResults([]);
    setChanges(null);
    setViewer(null);
    setError('');
    if (browsable) {
      loadDirectory('', { force: true });
    }
    if (sessionId) {
      loadChanges();
    } else if (onChangeCount) {
      onChangeCount(0);
    }
  }, [sessionId, browsePath]);

  useEffect(() => {
    if (!sessionId || !refreshToken) return;
    const timer = window.setTimeout(() => {
      loadChanges();
      if (visible && tab === 'files') {
        const loadedDirectories = ['', ...expanded];
        Promise.all(loadedDirectories.map(
          path => loadDirectory(path, { force: true }),
        ));
      }
    }, 350);
    return () => window.clearTimeout(timer);
  }, [refreshToken, sessionId, visible, tab, expanded]);

  useEffect(() => {
    if (!visible || !browsable) return undefined;
    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'visible') return;
      if (tab === 'files') {
        const loadedDirectories = ['', ...expanded];
        Promise.all(loadedDirectories.map(
          path => loadDirectory(path, { force: true }),
        ));
      }
      if (sessionId && tab === 'changes') loadChanges();
    }, 2000);
    return () => window.clearInterval(timer);
  }, [visible, browsable, tab, sessionId, browsePath, expanded]);

  useEffect(() => {
    if (!browsable || !query.trim()) {
      setSearchResults([]);
      setSearching(false);
      return undefined;
    }
    setSearching(true);
    const timer = window.setTimeout(async () => {
      try {
        const results = await searchAcpWorkspace({
          ...scopePayload(),
          query: query.trim(),
        });
        setSearchResults(results || []);
        setError('');
      } catch (nextError) {
        showError(nextError);
      } finally {
        setSearching(false);
      }
    }, 250);
    return () => window.clearTimeout(timer);
  }, [query, sessionId, browsePath]);

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
    // 请求序号防竞态：只应用最后一次点击的响应，慢响应（旧文件/旧工作区）直接丢弃。
    const requestId = ++previewRequestRef.current;
    setViewer({ name: entry.name, relativePath: entry.relativePath, preview: null, loading: true, error: '' });
    try {
      const preview = await previewAcpWorkspaceFile({
        ...scopePayload(),
        relativePath: entry.relativePath,
      });
      if (requestId !== previewRequestRef.current) return;
      setViewer({ name: entry.name, relativePath: entry.relativePath, preview, loading: false, error: '' });
      setError('');
    } catch (nextError) {
      if (requestId !== previewRequestRef.current) return;
      console.error('Codex workspace preview failed:', nextError);
      setViewer({
        name: entry.name,
        relativePath: entry.relativePath,
        preview: null,
        loading: false,
        error: copy.showRawErrors ? String(nextError) : copy.operationFailed,
      });
    }
  }

  // 变更项 → 弹窗 diff 视图；与 showFile 共用 viewer 弹窗（diff 字段驱动 diff 模式），
  // 同样用请求序号防竞态。
  async function showDiff(change) {
    const name = String(change.relativePath).split('/').pop() || change.relativePath;
    const requestId = ++previewRequestRef.current;
    setViewer({ name, relativePath: change.relativePath, preview: null, diff: null, loading: true, error: '' });
    try {
      const diff = await invoke('get_codex_workspace_diff', {
        sessionId,
        relativePath: change.relativePath,
      });
      if (requestId !== previewRequestRef.current) return;
      setViewer({ name, relativePath: change.relativePath, preview: null, diff, loading: false, error: '' });
      setError('');
    } catch (nextError) {
      if (requestId !== previewRequestRef.current) return;
      console.error('Codex workspace diff failed:', nextError);
      setViewer({
        name,
        relativePath: change.relativePath,
        preview: null,
        diff: null,
        loading: false,
        error: copy.showRawErrors ? String(nextError) : copy.operationFailed,
      });
    }
  }

  async function openWorkspacePath(command, relativePath, extra = {}) {
    if (!relativePath || !browsable) return false;
    try {
      await invoke(command, { ...scopePayload(), relativePath, ...extra });
      return true;
    } catch (nextError) {
      showError(nextError);
      return false;
    }
  }

  const rows = query.trim() ? searchResults : null;

  return (
    <aside
      ref={panelRef}
      style={{ width: `${panelWidth}px` }}
      className={`${visible ? 'flex' : 'hidden'} relative max-w-[88vw] min-w-0 shrink-0 border-l border-black/[0.06] dark:border-white/[0.07] bg-white/92 dark:bg-[#17181A]/96 backdrop-blur-xl flex-col`}
    >
      <div
        role="separator"
        aria-label={copy.resize}
        aria-orientation="vertical"
        onMouseDown={startPanelResize}
        onDoubleClick={resetPanelWidth}
        className="absolute inset-y-0 left-0 z-20 w-1.5 -translate-x-1/2 cursor-col-resize bg-black/10 hover:bg-[#0B57D0]/50 dark:bg-white/10 dark:hover:bg-[#A8C7FA]/60 transition-colors"
        title={copy.resizeHint}
      />
      <div className="h-14 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{copy.title}</div>
          <div className="truncate text-[10px] text-gray-400" title={session?.workspace_path || browsePath}>
            {session?.workspace_kind === 'temporary' ? copy.temporary : (session?.workspace_path || browsePath)}
          </div>
        </div>
        <button
          type="button"
          onClick={() => {
            loadDirectory('', { force: true });
            loadChanges();
          }}
          className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
          title={copy.refresh}
        >
          <RefreshCw size={14} />
        </button>
        <button type="button" onClick={onClose} className="w-7 h-7 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]" aria-label={copy.close}>
          <X size={14} />
        </button>
      </div>

      <>
          <div className="shrink-0 px-3 pt-2">
            <div className="grid grid-cols-2 rounded-lg bg-black/[0.035] dark:bg-white/[0.055] p-0.5">
              <button type="button" onClick={() => setTab('files')} className={`h-7 rounded-md text-[11px] ${tab === 'files' ? 'bg-white dark:bg-white/10 shadow-sm font-medium' : 'text-gray-400'}`}>
                {copy.files}
              </button>
              <button type="button" onClick={() => { setTab('changes'); loadChanges(); }} className={`h-7 rounded-md text-[11px] ${tab === 'changes' ? 'bg-white dark:bg-white/10 shadow-sm font-medium' : 'text-gray-400'}`}>
                {copy.changed}{changes?.changes?.length ? ` ${changes.changes.length}` : ''}
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
                    placeholder={copy.search}
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
                    <button type="button" onClick={() => onAddReference(entry.relativePath)} className={`w-6 h-6 rounded-md flex items-center justify-center ${referencedPaths.has(entry.relativePath) ? 'text-blue-500 bg-blue-500/10' : 'opacity-0 group-hover:opacity-100 text-gray-400'}`} title={copy.add}>
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
                    onOpenExternal={(entry) => openWorkspacePath('open_codex_workspace_file', entry.relativePath)}
                    onOpenReader={(entry) => openWorkspacePath('open_code_reader', entry.relativePath)}
                    systemOpenAvailable={systemOpenAvailable}
                    referencedPaths={referencedPaths}
                    copy={copy}
                  />
                )}
                {!searching && rows && rows.length === 0 && (
                  <div className="py-10 text-center text-[11px] text-gray-400">{copy.noFiles}</div>
                )}
              </div>
            </>
          ) : (
            <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-2 py-3">
              {!sessionId ? (
                // draft（无会话）模式：变更对比依赖会话基线，给专属空态而非复用旧会话文案。
                <div className="py-12 px-4 text-center text-[11px] leading-5 text-gray-400">{copy.noSessionChanges}</div>
              ) : (
                <>
                  {!changes?.baselineAvailable && (
                    <div className="mx-1 mb-2 rounded-lg bg-amber-500/8 px-2.5 py-2 text-[10px] leading-4 text-amber-700 dark:text-amber-300">
                      {copy.noBaseline}
                    </div>
                  )}
                  {changes?.branch && <div className="px-2 pb-2 text-[10px] text-gray-400">{copy.branch} · {changes.branch}</div>}
                  {(changes?.changes || []).map(change => (
                    <div
                      key={`${change.status}:${change.relativePath}`}
                      className="group min-h-11 px-2 py-1.5 flex items-center gap-2 rounded-lg hover:bg-black/[0.04] dark:hover:bg-white/[0.05]"
                    >
                      <button type="button" onClick={() => showDiff(change)} className="min-w-0 flex-1 flex items-center gap-2 text-left" title={change.relativePath}>
                        <span className={`min-w-10 h-5 px-1.5 rounded-md inline-flex items-center justify-center text-[9px] font-medium ${statusTone(change.status)}`}>
                          {changeLabel(change.status, copy)}
                        </span>
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-[11px]" title={change.relativePath}>{change.relativePath}</span>
                          <span className="block mt-0.5 truncate text-[9px] text-gray-400">{originLabel(change.origin, copy)}{change.staged ? ` · ${copy.staged}` : ''}</span>
                        </span>
                        <ChevronRight size={12} className="shrink-0 text-gray-400" />
                      </button>
                      <button
                        type="button"
                        aria-label={referencedPaths.has(change.relativePath) ? copy.addedPath(change.relativePath) : copy.addPath(change.relativePath)}
                        title={referencedPaths.has(change.relativePath) ? copy.added : copy.add}
                        onClick={() => onAddReference(change.relativePath)}
                        className={`w-6 h-6 shrink-0 rounded-md flex items-center justify-center transition-opacity ${
                          referencedPaths.has(change.relativePath)
                            ? 'text-blue-500 bg-blue-500/10'
                            : 'text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
                        }`}
                      >
                        <Plus size={13} />
                      </button>
                    </div>
                  ))}
                  {changes && changes.changes.length === 0 && (
                    <div className="py-12 text-center text-[11px] text-gray-400">{copy.noChanges}</div>
                  )}
                </>
              )}
            </div>
          )}
      </>
      {viewer && (
        <CodeViewerModal
          name={viewer.name}
          relativePath={viewer.relativePath}
          preview={viewer.preview}
          diff={viewer.diff}
          loading={viewer.loading}
          error={viewer.error}
          onClose={() => setViewer(null)}
          onOpen={systemOpenAvailable
            ? () => openWorkspacePath('open_codex_workspace_file', viewer.relativePath)
            : undefined}
          onReveal={systemOpenAvailable
            ? () => openWorkspacePath('reveal_codex_workspace_file', viewer.relativePath)
            : undefined}
          onOpenInNewWindow={systemOpenAvailable
            ? async () => {
                const opened = viewer.diff
                  ? await openWorkspacePath('open_code_reader', viewer.relativePath, { kind: 'diff' })
                  : await openWorkspacePath('open_code_reader', viewer.relativePath);
                if (opened) setViewer(null);
              }
            : undefined}
          copy={copy}
        />
      )}
    </aside>
  );
}
