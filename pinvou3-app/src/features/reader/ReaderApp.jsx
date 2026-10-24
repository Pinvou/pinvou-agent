import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { X } from '../../components/icons.jsx';
import { useCopyFlash } from '../../hooks/useCopyFlash.js';
import { useFollowMainWindowLanguage } from '../../hooks/useFollowMainWindowLanguage.js';
import { pathBasename } from '../../shared/path-utils.js';
import { FileColoredIcon } from '../../components/files/FileColoredIcon.jsx';
import { invokeTauri, listenTauri } from '../../platform/tauri/client.js';
import { useSystemDarkMode } from '../../hooks/useSystemDarkMode.js';
import { normalizeColorScheme, resolveTheme } from '../../shared/color-scheme.js';
import { dict, zhDict } from '../../shared/i18n.js';
import {
  CodeViewerContent,
  CodeViewerToolbar,
  useCodeHighlight,
  useViewerFontSize,
  viewerFontSizeBounds,
} from '../codex/CodeViewerContent.jsx';

// 独立代码阅读器窗口（code-reader 单例）：主窗口弹窗「新窗口打开」的文件在此以
// tab 累积（Win11 记事本模式）。建窗前排队的请求经 take_code_reader_pending 拉取，
// 窗口存活期间的后续请求经 code-reader:open 事件推送。
const READER_OPEN_EVENT = 'code-reader:open';

// kind 纳入 key：同一路径可同时存在文件 tab 与 diff tab，互不覆盖；旧载荷无 kind 时默认文件。
function tabKey(request) {
  return `${request.kind || 'file'}|${request.sessionId || ''}|${request.workspacePath || ''}|${request.relativePath || ''}`;
}

function tabName(relativePath) {
  return pathBasename(relativePath, { fallback: relativePath }) || '';
}

function ReaderTabContent({ tab, state, fontSize, copy }) {
  const highlighted = useCodeHighlight(state.preview, tab.name, tab.kind === 'diff' ? 'diff' : undefined);
  return (
    <CodeViewerContent
      preview={state.preview}
      loading={state.loading}
      error={state.error}
      fontSize={fontSize}
      highlighted={highlighted}
      copy={copy}
    />
  );
}

export function ReaderApp() {
  // Appearance follows the app preference; while it is `system` the reader must
  // track OS theme flips live, same as the main and detached windows.
  const systemDark = useSystemDarkMode();
  const [colorScheme, setColorScheme] = useState('system');
  const onMainWindowSettings = useCallback(
    (settings) => setColorScheme(normalizeColorScheme(settings?.color_scheme)),
    [],
  );
  const language = useFollowMainWindowLanguage({ onSettings: onMainWindowSettings });
  // Lazy-load window (a direct mount that bypasses the entry's
  // ensureLanguage preload): fall back to an empty table — string keys
  // render blank instead of crashing. Production entries (reader-main.jsx)
  // preload before the first render, so the normal path never sees it.
  const copy = useMemo(
    () => (dict[language] || zhDict()).uiCodexWorkspace || {},
    [language],
  );
  const [tabs, setTabs] = useState([]);
  const [activeKey, setActiveKey] = useState('');
  const [previews, setPreviews] = useState({});
  const [fontSize, adjustFontSize] = useViewerFontSize();
  const [copied, copyText] = useCopyFlash();
  const fontBounds = viewerFontSizeBounds();
  const loadedKeysRef = useRef(new Set());

  // Re-apply when the preference resolves and on every system flip while `system`;
  // explicit light/dark ignore systemDark.
  useEffect(() => {
    document.documentElement.classList.toggle('dark', resolveTheme(colorScheme, systemDark) === 'dark');
  }, [colorScheme, systemDark]);

  useEffect(() => {
    document.title = copy.readerTitle;
  }, [copy]);

  const loadPreview = useCallback((request) => {
    const key = tabKey(request);
    setPreviews(current => ({ ...current, [key]: { loading: true, error: '', preview: null } }));
    // diff tab 调差异接口并把 WorkspaceDiff 适配为文本预览，与文件 tab 共用渲染/缓存路径。
    const fetch = request.kind === 'diff'
      ? invokeTauri('get_codex_workspace_diff', {
          sessionId: request.sessionId || null,
          relativePath: request.relativePath,
        }).then(diff => ({
          kind: 'text',
          name: tabName(request.relativePath),
          relativePath: request.relativePath,
          size: 0,
          modified: 0,
          text: diff.text || copy.noDiff,
          dataUrl: null,
          truncated: diff.truncated,
        }))
      : invokeTauri('preview_codex_workspace_file', {
          sessionId: request.sessionId || null,
          workspacePath: request.workspacePath || null,
          relativePath: request.relativePath,
        });
    fetch.then((preview) => {
      setPreviews(current => ({ ...current, [key]: { loading: false, error: '', preview } }));
    }).catch((nextError) => {
      console.error('code reader preview failed:', nextError);
      setPreviews(current => ({
        ...current,
        [key]: {
          loading: false,
          error: copy.showRawErrors ? String(nextError) : copy.operationFailed,
          preview: null,
        },
      }));
    });
  }, [copy]);

  const openTab = useCallback((request) => {
    if (!request?.relativePath) return;
    const key = tabKey(request);
    setTabs((current) => {
      if (current.some(tab => tab.key === key)) return current;
      return [...current, {
        key,
        kind: request.kind === 'diff' ? 'diff' : 'file',
        sessionId: request.sessionId || '',
        workspacePath: request.workspacePath || '',
        relativePath: request.relativePath,
        name: tabName(request.relativePath),
      }];
    });
    setActiveKey(key);
    if (!loadedKeysRef.current.has(key)) {
      loadedKeysRef.current.add(key);
      loadPreview(request);
    }
  }, [loadPreview]);

  // 启动时拉取建窗前排队的请求；窗口存活期间监听打开事件，并在窗口获得焦点时
  // 再次拉取 pending 队列（DOM focus 是原生事件，不依赖 Tauri 事件通道，兜底不丢文件）。
  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    const pullPending = () => {
      invokeTauri('take_code_reader_pending').then((pending) => {
        if (disposed || !Array.isArray(pending) || !pending.length) return;
        pending.forEach(openTab);
      }).catch(() => {});
    };
    pullPending();
    listenTauri(READER_OPEN_EVENT, (event) => {
      if (!disposed) openTab(event.payload || {});
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => {});
    window.addEventListener('focus', pullPending);
    return () => {
      disposed = true;
      window.removeEventListener('focus', pullPending);
      if (unlisten) unlisten();
    };
  }, [openTab]);

  const activeTab = tabs.find(tab => tab.key === activeKey) || null;
  const activeState = activeTab ? previews[activeTab.key] || { loading: true, error: '', preview: null } : null;

  function closeTab(key) {
    const index = tabs.findIndex(tab => tab.key === key);
    const next = tabs.filter(tab => tab.key !== key);
    if (key === activeKey) {
      const fallback = next[Math.min(index, next.length - 1)];
      setActiveKey(fallback ? fallback.key : '');
    }
    setTabs(next);
    // 释放缓存并允许重开时重新加载（预览失败/文件已变更时有重载路径）。
    loadedKeysRef.current.delete(key);
    setPreviews((current) => {
      if (!current[key]) return current;
      const nextPreviews = { ...current };
      delete nextPreviews[key];
      return nextPreviews;
    });
  }

  function openActive(command) {
    if (!activeTab) return;
    invokeTauri(command, {
      sessionId: activeTab.sessionId || null,
      workspacePath: activeTab.workspacePath || null,
      relativePath: activeTab.relativePath,
    }).catch(nextError => console.error('code reader open failed:', nextError));
  }


  return (
    <div className="h-screen flex flex-col bg-white dark:bg-[#1E1E20] text-gray-900 dark:text-gray-100">
      <div className="h-10 shrink-0 flex items-stretch overflow-x-auto custom-scrollbar border-b border-black/[0.05] dark:border-white/[0.06]">
        {tabs.map(tab => (
          <div
            key={tab.key}
            className={`group shrink-0 flex items-center gap-1.5 pl-3 pr-1 border-r border-black/[0.05] dark:border-white/[0.06] ${
              tab.key === activeKey ? 'bg-black/[0.04] dark:bg-white/[0.07]' : 'hover:bg-black/[0.02] dark:hover:bg-white/[0.04]'
            }`}
          >
            <button
              type="button"
              onClick={() => setActiveKey(tab.key)}
              title={tab.relativePath}
              className="flex items-center gap-1.5 min-w-0"
            >
              <FileColoredIcon name={tab.name} size={14} />
              <span className="truncate max-w-[180px] text-[12px]">{tab.name}{tab.kind === 'diff' ? copy.diffSuffix : ''}</span>
            </button>
            <button
              type="button"
              onClick={() => closeTab(tab.key)}
              aria-label={copy.closeTab}
              title={copy.closeTab}
              className="w-5 h-5 rounded flex items-center justify-center text-gray-400 opacity-0 group-hover:opacity-100 hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"
            >
              <X size={11} />
            </button>
          </div>
        ))}
      </div>

      {activeTab ? (
        <>
          <div className="h-11 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
            <div className="min-w-0 flex-1 truncate text-[11px] text-gray-400" title={activeTab.relativePath}>
              {activeTab.relativePath}
            </div>
            <CodeViewerToolbar
              copy={copy}
              fontSize={fontSize}
              fontBounds={fontBounds}
              adjust={adjustFontSize}
              copied={copied}
              copyText={copyText}
              contentText={activeState?.preview?.kind === 'text' ? activeState.preview.text : ''}
              canCopyContent={activeState?.preview?.kind === 'text'}
              copyPathText={activeTab.relativePath}
              hideDesktopActions={activeTab.kind === 'diff'}
              onReveal={() => openActive('reveal_codex_workspace_file')}
              onOpen={() => openActive('open_codex_workspace_file')}
            />
          </div>
          <ReaderTabContent tab={activeTab} state={activeState} fontSize={fontSize} copy={copy} />
        </>
      ) : (
        <div className="flex-1 flex items-center justify-center p-6">
          <div className="text-center text-[12px] leading-5 text-gray-400">{copy.readerEmpty}</div>
        </div>
      )}
    </div>
  );
}
