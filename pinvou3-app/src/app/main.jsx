import { lazy, startTransition as scheduleViewTransition, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import { Edit2, BarChart2, Settings, Smartphone, Clock, Package, Search, ChevronDown, Menu, MoreHorizontal, Check, Filter, Layers, MessageSquare, X, XIcon, Globe, BookOpen, Puzzle, PetPawIcon } from '../components/icons.jsx';
import { ArchiveConfirmDialog, ArchiveToast, NavItem, RecentItem } from '../components/layout/NavigationComponents.jsx';
import { SidePanelLayoutProvider } from '../components/layout/ResizableSidePanel.jsx';
import {
  RightDockHost,
  RightDockPanel,
  RightDockProvider,
} from '../components/layout/RightDock.jsx';
import { AcpAgentLogo } from '../features/codex/AcpAgentLogo.jsx';
import { CodexAcpView } from '../features/codex/LazyCodexAcpView.jsx';
import { PinvouLogo } from '../components/PinvouLogo.jsx';
import { MobileMoreSheet, MobileTabBar, MobileTopBar } from '../components/layout/MobileShell.jsx';
import { VllmSetupProgress } from '../components/VllmSetupProgress.jsx';
import { bridge, useBridgeState, usePlatformCapability, activeModelIsLocal, shouldShowApiKeyGate } from '../hooks/useBridge.js';
import { useCompactViewport, useVisualViewportHeight } from '../hooks/useViewport.js';
import { useSystemDarkMode } from '../hooks/useSystemDarkMode.js';
import { COLOR_SCHEME_STORAGE_KEY, normalizeColorScheme, resolveTheme } from '../shared/color-scheme.js';
import { DEFAULT_CHAT_TITLES, dict, createLatestLanguageGate, ensureLanguage, LANG_TO_TAG, initialSystemLanguage, SEARCH_KEY_PROVIDERS, TAG_TO_LANG } from '../shared/i18n.js';
import { formatSessionDate, localDateKey, formatDateGroupLabel } from '../shared/date-utils.js';
import { groupSessionsWithProjects, resolveSessionProjectId, needsAddFolderConfirm, WORKSPACE_KIND_BOUND } from '../features/projects/projectGrouping.js';
import { ProjectGroupHeader } from '../features/projects/ProjectGroupHeader.jsx';
import { MoveToProjectDialog } from '../features/projects/MoveToProjectDialog.jsx';
import { RebindFolderDialog } from '../features/projects/RebindFolderDialog.jsx';
import { WorkspacePickerDialog } from '../features/projects/WorkspacePickerDialog.jsx';
import { computePickerRows, pickerPrimaryRoot, pickerProjectRoots, workspaceNoticeTone } from '../features/projects/workspacePickerState.js';
import { removeRootPlan, rootAlreadyPresent, rootConflictsWithExisting, rootPathOf } from '../features/projects/manageFoldersState.js';
import { ManageProjectFoldersDialog } from '../features/projects/ManageProjectFoldersDialog.jsx';
import { classifyRebindError } from '../features/projects/rebindErrors.js';
import { runSessionBatch } from '../shared/session-management.js';
import { filterSessionsByTab, groupSessionsByLocalDate, sessionListComparator } from '../shared/session-list-pipeline.js';
import { can, isWeb } from '../shared/platform.js';
import { installGlobalMarkdownRenderer } from '../shared/markdown-renderer.js';
import {
  acquireNativeSurfaceTransitionHide,
  BrowserView,
} from '../features/browser/BrowserView.jsx';
import {
  createNativeSurfaceTransitionGate,
  settleBrowserUiPublicationAfterCommit,
} from '../features/browser/native-surface-transition.mjs';
import {
  awaitBrowserListenerReadiness,
  createBrowserSessionCommandEchoGuard,
  createBrowserSessionEpochTracker,
} from '../features/browser/browser-state-sync.mjs';
import {
  activateBrowserPane,
  beginBrowserOpen,
  browserOpenStateFor,
  browserPaneStateFor,
  closeBrowserPane,
  removeBrowserPaneState,
  restoreBrowserPane,
  selectArtifactsPane,
  settleBrowserOpen,
} from '../features/browser/browser-pane-state.mjs';
import { ViewErrorBoundary } from '../shared/ViewErrorBoundary.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { createPinvouModeScopeKey, savePinvouModeState } from '../features/chat/pinvou-mode-state.js';
import { WebConnectionStatus } from '../features/web/WebConnectionStatus.jsx';
import { VoiceShortcutRouter } from '../features/voice-composer/VoiceShortcutRouter.jsx';
import { createPetActivationGuard } from '../features/pet/activation-guard.js';
import { SessionAttachmentTitle } from '../features/attachments/SessionAttachmentTitle.jsx';
import {
  sessionTitlePlainText,
  sessionTitlePresentation,
} from '../features/attachments/attachment-message.js';
import {
  invokeTauri,
  isTauriAvailable,
  tauriCommands,
  tauriEvents,
} from '../platform/tauri/client.js';
import { listAcpSessions } from '../features/codex/acpClient.js';
import { revealStartupWindow } from '../platform/tauri/startup-window.js';

// 后端默认会话标题哨兵集合(bridge 按当前语言生成三语兜底标题,并据此判断是否自动改名)——
// 显示层把任意一种哨兵标题映射成当前语言的「新对话」文案。哨兵是跨语言的后端
// 契约而非当前 UI 文案,直接使用 shared/i18n.js 的静态集合,与词典装载进度无关
// (zh 主用户不会装载 en/ja chunk,不能从 dict 派生)。
function isDefaultChatTitle(title) {
  return DEFAULT_CHAT_TITLES.has(title);
}
// 定时运行侧栏条目的 leadingIcon(Clock 图标 + 未读角标):scheduledRunItems 与
// decorateScheduledRunChat 共用同一节点形态,提取成纯函数避免两处拷贝漂移。
function scheduledRunIcon(run, activeTheme) {
  return (
    <span className="relative inline-flex h-5 w-5 items-center justify-center">
      <Clock size={18} />
      {run.unread && (
        <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full border-2"
          style={{ background: '#0B57D0', borderColor: activeTheme === 'dark' ? '#1E1F20' : '#F0F4F9' }} />
      )}
    </span>
  );
}
// Static regression anchor: (<NavItem icon={<Clock size={18} />} label={t.scheduledPlans} unread={!!(bs && ((bs.scheduledTasks || []).some(task => task.hasUnreadRuns) || (bs.scheduledTaskRecentRuns || []).some(run => run && run.unread)))} />)
const PREVIEW_SCHEDULED_RUN_SHORTCUTS = [
  { id: 'preview-run-1', automationId: 'preview-daily-brief', taskNameKey: 'previewTaskDailyBrief', sessionId: 'preview-session-1', status: 'completed', scheduledFor: '2026-07-14T08:00:00+08:00', unread: true },
  { id: 'preview-run-4', automationId: 'preview-follow-up', taskNameKey: 'previewTaskFollowUp', sessionId: 'preview-session-4', status: 'running', scheduledFor: '2026-07-14T09:00:00+08:00', unread: false },
  { id: 'preview-run-6', automationId: 'preview-weekly-report', taskNameKey: 'previewTaskSalesWeekly', sessionId: 'preview-session-6', status: 'completed', scheduledFor: '2026-07-10T16:00:00+08:00', unread: false },
];
import { PinvouSummonCard } from '../features/tools/tool-renderers.jsx';
import { SearchOverlay } from '../features/search/SearchOverlay.jsx';
import { UpdateNoticeButton } from '../features/updater/UpdateNoticeButton.jsx';
import { Lanyard } from '../features/personas/persona-shared.jsx';
import { VIEW_LOADERS, prefetchView } from './view-loaders.js';
// Low-traffic views are lazy-loaded: VIEW_LOADERS (see view-loaders.js) is the
// single dynamic-import outlet; React.lazy and the NavItem hover/focus
// prefetch share the same factory so they hit the same module cache.
// codex is the exception: it renders through the LazyCodexAcpView wrapper,
// which imports the same CodexAcpView module (sharing the prefetch cache) and
// adds an in-place retry boundary for chunk fetch failures.
// ChatView and Lanyard render at startup and stay statically imported.
const LazySettingsView = lazy(() => VIEW_LOADERS.settings().then(m => ({ default: m.SettingsView })));
const LazyToolStoreView = lazy(() => VIEW_LOADERS.toolStore().then(m => ({ default: m.ToolStoreView })));
const LazyCardPoolView = lazy(() => VIEW_LOADERS.cardpool().then(m => ({ default: m.CardPoolView })));
const LazyScheduledTasksView = lazy(() => VIEW_LOADERS.scheduled().then(m => ({ default: m.ScheduledTasksView })));
const LazyKnowledgeView = lazy(() => VIEW_LOADERS.knowledge().then(m => ({ default: m.KnowledgeView })));
const LazyMonitorView = lazy(() => VIEW_LOADERS.monitor().then(m => ({ default: m.MonitorView })));
const LazySearchView = lazy(() => VIEW_LOADERS.search().then(m => ({ default: m.SearchView })));
const LazyPersonaEditorModal = lazy(() => VIEW_LOADERS.cardpool().then(m => ({ default: m.PersonaEditorModal })));
const LazyWebAccessModal = lazy(() => VIEW_LOADERS.settings().then(m => ({ default: m.WebAccessModal })));
const LazyDetachedShell = lazy(() => import('./DetachedShell.jsx').then(m => ({ default: m.DetachedShell })));

// 视图 chunk 加载占位:沿用 DetachedShell 的「…」惯例,不引入新视觉语言。
function ViewFallback() {
  return <div className="p-6 text-sm opacity-60" data-testid="lazy-view-fallback">…</div>;
}

// personas-i18n overlay 的 UI 语言兜底注入:实现收敛在 personas-overlay.js,
// 与撕离窗(DetachedShell 的 useDetachedBase)共用同一模块。
import { ensurePersonaI18nOverlay } from './personas-overlay.js';
import { TitleBar } from './DesktopTitleBar.jsx';

installGlobalMarkdownRenderer(window);
window.__PINVOU_STARTUP__.mark('app:main_module_body_enter');

let appFirstRenderMarked = false;

const APP_BRIDGE_STATE_DOMAINS = [
  'platform', 'sessions', 'chat', 'voice', 'knowledge', 'scheduled', 'monitor',
  'settings', 'models', 'vllm', 'interaction', 'personas',
  'memory', 'remoteControl', 'updater', 'dependencies', 'projects',
];

function emitPetEvent(ev, name, payload) {
  if (!ev) return Promise.resolve(false);
  try {
    if (typeof ev.emitTo === 'function') {
      return Promise.resolve(ev.emitTo('pet', name, payload));
    }
    if (typeof ev.emit === 'function') {
      return Promise.resolve(ev.emit(name, payload));
    }
  } catch (error) {
    return Promise.reject(error);
  }
  return Promise.resolve(false);
}

function workspaceDisplayName(path) {
  const parts = String(path || '').split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || String(path || '');
}

// Per-item callback cache behind the RecentItem memo: sidebar task items
// (item) are derived by useMemo and keep stable references while the
// underlying data is unchanged, so caching the onPickUp / scheduled-run
// onSelect closures keyed by item keeps RecentItem props shallow-equal
// across unrelated re-renders (local UI state, pure chat streaming tokens)
// and skips the row re-render. Every captured handler is a stable useCallback
// reference, and whenever a dependency (t etc.) changes the item is rebuilt
// too (the derived memo depends on the same state), so the cache can never
// hand out a stale closure; old item keys are garbage-collected along with
// their closures.
function cachedItemCallback(cache, item, build) {
  let fn = cache.get(item);
  if (!fn) {
    fn = build(item);
    cache.set(item, fn);
  }
  return fn;
}
const sidebarPickUpCallbacks = new WeakMap();
const sidebarScheduledSelectCallbacks = new WeakMap();
// 拖拽 payload 与 onPickUp/onSelect 同因缓存:内联对象每次渲染都是新引用,
// 会击穿 RecentItem 的 memo(见 NavigationComponents 内注释)。
const sidebarDndPayloads = new WeakMap();

// Static icon elements for the sidebar main nav: module-level constants keep
// the element references stable so the NavItem memo can hit.
const NAV_ICON_NEW_CHAT = <Edit2 size={18} />;
const NAV_ICON_SEARCH = <Search size={18} />;
const NAV_ICON_SCHEDULED = <Clock size={18} />;
const NAV_ICON_OUTPUTS = <Package size={18} />;
const NAV_ICON_MONITOR = <BarChart2 size={18} />;
const NAV_ICON_TOOL_STORE = <Puzzle size={18} />;
const NAV_ICON_CARD_POOL = <Layers size={18} />;
const NAV_ICON_KNOWLEDGE = <BookOpen size={18} />;
const NAV_ICON_CURRENT_CHAT = <MessageSquare size={18} />;

// Hover/focus prefetch callbacks (prefetchView is a module function):
// constant references for the NavItem memo comparison.
const NAV_PREFETCH = {
  scheduled: () => prefetchView('scheduled'),
  knowledge: () => prefetchView('knowledge'),
  monitor: () => prefetchView('monitor'),
  toolStore: () => prefetchView('toolStore'),
  cardpool: () => prefetchView('cardpool'),
};

    // App root component: aggregates bridge state, routing, and all sidebar/overlay UI. Size and complexity are historical
    // evolution; splitting requires a dedicated refactor task (involving a hundred-plus closure handlers and test contracts);
    // only lint fixes here, no behavior change.
    // eslint-disable-next-line sonarjs/cognitive-complexity -- due to the App root component's size; behavior preservation takes priority, split needs a dedicated refactor
    const App = () => {
      if (!appFirstRenderMarked) {
        // One-shot startup marker: module-level boolean set on first render, unrelated to component state (no rerender needed).
        appFirstRenderMarked = true;
        window.__PINVOU_STARTUP__.mark('react:app_render_start');
      }
      const bs = useBridgeState(APP_BRIDGE_STATE_DOMAINS);
      // latest-ref mirror: stable useCallbacks (e.g. navigateFromScheduledRun)
      // read the latest bridge snapshot when the event fires instead of
      // depending on bs, which would change the callback identity on every
      // notify and defeat the memo.
      const bsRef = useRef(bs);
      bsRef.current = bs;
      useLayoutEffect(() => {
        window.__PINVOU_STARTUP__.mark('react:first_commit');
        window.__PINVOU_STARTUP__.flush();
        // Linux 的主窗口在配置中隐藏创建。首次 React 提交说明可交互 DOM 已就绪，
        // 此时再映射 XWayland 窗口，避免冷启动阶段把尚未稳定的输入表面暴露给用户。
        void revealStartupWindow().then((revealed) => {
          if (!revealed) return;
          window.__PINVOU_STARTUP__.mark('react:startup_window_revealed');
          window.__PINVOU_STARTUP__.flush();
        });
      }, []);
      useEffect(() => {
        window.__PINVOU_STARTUP__.mark('react:first_effect');
        window.__PINVOU_STARTUP__.flush();
        // 连续两个 rAF：第二个回调发生在首次提交已经交给 WebView 绘制之后。此时再启动
        // 558 MiB embedding 模型的 blocking 后台加载，避免模型 IO/ONNX 初始化阻塞白屏。
        let secondFrame = 0;
        const firstFrame = window.requestAnimationFrame(() => {
          window.__PINVOU_STARTUP__.mark('react:first_animation_frame');
          secondFrame = window.requestAnimationFrame(() => {
            window.__PINVOU_STARTUP__.mark('react:first_frame_presented');
            window.__PINVOU_STARTUP__.flush();
            if (bridge.available && bridge.knowledge.loadKnowledgeEmbedderAfterFirstFrame) {
              bridge.knowledge.loadKnowledgeEmbedderAfterFirstFrame();
            }
            // 首帧已呈现后空闲预取低频视图 chunk:桌宠/定时快捷方式会不经侧栏直接
            // 跳视图(如 scheduledTaskAutoOpenId),悬停预取覆盖不到这些入口。
            if (typeof window.requestIdleCallback === 'function') {
              window.requestIdleCallback(() => {
                prefetchView('scheduled');
                prefetchView('settings');
                prefetchView('codex');
              }, { timeout: 4000 });
            } else {
              window.setTimeout(() => {
                prefetchView('scheduled');
                prefetchView('settings');
                prefetchView('codex');
              }, 1500);
            }
          });
        });
        // 让首帧先交给 WebView 绘制，再异步校验飞书/企微实时鉴权状态。
        // 后端并行跑两个 CLI；结果只刷新技能目录，不阻塞主界面。
        const authTimer = window.setTimeout(() => {
          if (bridge.available && bridge.platform.refreshConnectorAuthGates) {
            bridge.platform.refreshConnectorAuthGates().catch(error => {
              console.warn('[startup] connector auth refresh failed', error);
            });
          }
        }, 0);
        return () => {
          window.cancelAnimationFrame(firstFrame);
          if (secondFrame) window.cancelAnimationFrame(secondFrame);
          window.clearTimeout(authTimer);
        };
      }, []);
      const [activeChat, setActiveChat] = useState(null);
      const [currentView, setCurrentViewState] = useState('chat');
      const [sessionSyncEpoch, setSessionSyncEpoch] = useState(0);
      // Color-scheme preference: `system` follows the OS (fresh-install default,
      // live tracking), light/dark are explicit picks. activeTheme is the resolved
      // theme to render; light when the system preference is undeterminable
      // (see shared/color-scheme.js).
      const systemDark = useSystemDarkMode();
      const [colorScheme, setColorScheme] = useState('system');
      const activeTheme = resolveTheme(colorScheme, systemDark);
      // Browser state is scoped to workspace sessions: switching chats only shows
      // that chat's WebView2. The login profile remains globally shared by the
      // backend. Legacy events without a sessionId must fail closed.
      const [browserSessions, setBrowserSessions] = useState({});
      const [browserPaneStates, setBrowserPaneStates] = useState({});
      const [browserOpenStates, setBrowserOpenStates] = useState({});
      const browserOpenAttemptsRef = useRef({});
      const browserOpenAttemptSequenceRef = useRef(0);
      const browserLifecycleListenersReadyRef = useRef(null);
      const browserLifecycleEventEpochRef = useRef(null);
      const browserLifecycleStatusRequestEpochRef = useRef(null);
      if (!browserLifecycleEventEpochRef.current) {
        browserLifecycleEventEpochRef.current = createBrowserSessionEpochTracker();
      }
      if (!browserLifecycleStatusRequestEpochRef.current) {
        browserLifecycleStatusRequestEpochRef.current = createBrowserSessionEpochTracker();
      }
      const [browserResizeActive, setBrowserResizeActive] = useState(false);
      const [browserOwnershipSlot, setBrowserOwnershipSlot] = useState(null);
      const [browserDocumentHidden, setBrowserDocumentHidden] = useState(() => (
        typeof document !== 'undefined' && document.visibilityState === 'hidden'
      ));
      const [rightDockState, setRightDockState] = useState({
        activePanelId: null,
        occluded: false,
      });
      // Child overlays publish through RightDockProvider. Keep their reservation
      // in App state in the same React batch as the child permit so BrowserView is
      // suspended in the very commit that first exposes the overlay.
      const [rightDockOcclusionPublications, setRightDockOcclusionPublications] = useState([]);
      const browserSurfaceTransitionContextRef = useRef({
        sessionId: null,
        hasWorkspace: false,
        visible: false,
        compact: false,
        scheduledRunChat: false,
      });
      const browserTransitionPublishingRef = useRef(0);
      const browserSessionTransitionPendingRef = useRef(0);
      const browserBridgeSessionTransitionRef = useRef(null);
      const [browserSessionCommandEchoGuard] = useState(
        () => createBrowserSessionCommandEchoGuard(),
      );
      const [browserUiCommitEpoch, setBrowserUiCommitEpoch] = useState(0);
      const browserUiCommitSequenceRef = useRef(0);
      const browserUiCommitWaitersRef = useRef(new Map());
      const browserUiCommitMountedRef = useRef(true);
      const requestBrowserUiCommitAck = useCallback(() => {
        if (!browserUiCommitMountedRef.current) return Promise.resolve(false);
        const epoch = browserUiCommitSequenceRef.current + 1;
        browserUiCommitSequenceRef.current = epoch;
        const committed = new Promise((resolve) => {
          browserUiCommitWaitersRef.current.set(epoch, resolve);
        });
        // This marker is queued after the publication's state mutations. With
        // no transition-priority update in the guarded path, its layout effect
        // proves the target React tree has committed before native show resumes.
        setBrowserUiCommitEpoch(epoch);
        return committed;
      }, []);
      useLayoutEffect(() => {
        for (const [epoch, resolve] of browserUiCommitWaitersRef.current) {
          if (epoch > browserUiCommitEpoch) continue;
          browserUiCommitWaitersRef.current.delete(epoch);
          resolve(true);
        }
      }, [browserUiCommitEpoch]);
      useEffect(() => {
        const waiters = browserUiCommitWaitersRef.current;
        browserUiCommitMountedRef.current = true;
        return () => {
          browserUiCommitMountedRef.current = false;
          for (const resolve of waiters.values()) resolve(false);
          waiters.clear();
        };
      }, []);
      const browserUiTransitionGateRef = useRef(null);
      const createBrowserUiTransitionGate = () => (
        createNativeSurfaceTransitionGate({
          acquireHide: acquireNativeSurfaceTransitionHide,
          getContext: () => browserSurfaceTransitionContextRef.current,
          onError: (error) => {
            console.error('[browser] native-surface transition failed', error);
          },
        })
      );
      if (!browserUiTransitionGateRef.current) {
        browserUiTransitionGateRef.current = createBrowserUiTransitionGate();
      }
      useEffect(() => {
        if (!browserUiTransitionGateRef.current) {
          browserUiTransitionGateRef.current = createBrowserUiTransitionGate();
        }
        const gate = browserUiTransitionGateRef.current;
        return () => {
          gate?.dispose();
          if (browserUiTransitionGateRef.current === gate) {
            browserUiTransitionGateRef.current = null;
          }
        };
      }, []);
      const handleRightDockStateChange = useCallback((next) => {
        setRightDockState((current) => (
          current.activePanelId === next.activePanelId
          && current.occluded === next.occluded
            ? current
            : {
                activePanelId: next.activePanelId,
                occluded: next.occluded,
              }
        ));
      }, []);
      // Keep the currently published task identity stable while a bridge session
      // switch is waiting behind the native-surface hide barrier.
      const browserSessionId = activeChat || (bs && bs.activeSessionId) || null;
      const platformCapabilities = (bs && bs.platformCapabilities) || {};
      const browserNativeDisplayAvailable = !!platformCapabilities.browserNativeDisplay;
      const browserSessionIdRef = useRef(browserSessionId);
      browserSessionIdRef.current = browserSessionId;
      const browserPaneState = browserPaneStateFor(browserPaneStates, browserSessionId);
      const browserOpenState = browserOpenStateFor(browserOpenStates, browserSessionId);
      const browserPaneOpen = browserPaneState.open;
      const browserPaneSelected = browserPaneState.browserSelected;
      const browserDockSelectedPanelId = browserPaneSelected ? 'browser' : 'artifact-preview';
      const browserDockActivationKey = `${browserSessionId || ''}:${browserPaneState.activation}`;
      const browserActive = browserNativeDisplayAvailable
        && !!(browserSessionId && browserSessions[browserSessionId]);
      const browserViewSessionId = browserSessionId;
      const browserPaneAllowed = currentView === 'chat'
        || (currentView === 'scheduled' && !!(bs && bs.scheduledRunContext));
      const browserWorkspaceStarting = browserOpenState.status === 'starting';
      const browserWorkspaceError = browserOpenState.status === 'failed'
        ? browserOpenState.error
        : '';
      const runBrowserUiTransition = useCallback((publish, options) => {
        const tracksSession = options?.channel === 'session';
        const tracksCommandEcho = tracksSession
          && options?.sessionSource !== 'bridge'
          && Object.keys(options).includes('sessionTarget');
        const sessionCommandToken = tracksCommandEcho
          ? browserSessionCommandEchoGuard.begin(
            options.sessionTarget,
            bridge.activeSessionId || null,
          )
          : null;
        if (tracksSession) browserSessionTransitionPendingRef.current += 1;
        const finishRequest = () => {
          if (sessionCommandToken) {
            browserSessionCommandEchoGuard.settle(sessionCommandToken);
          }
          if (!tracksSession) return;
          browserSessionTransitionPendingRef.current -= 1;
          if (
            browserSessionTransitionPendingRef.current === 0
            && options?.reconcileSessionOnSettle !== false
            && browserUiCommitMountedRef.current
          ) {
            setSessionSyncEpoch((epoch) => epoch + 1);
          }
        };
        let result;
        try {
          // Session and view channels are primary navigation (switch chat, new
          // chat, main tab). When the native hide barrier fails after its
          // retries they may publish without a successful hide ACK only while
          // retaining a cleanup lease, rather than silently dropping the
          // action; overlay/dock channels stay
          // fail-closed so menus never open above a visible native page.
          const transitionOptions = {
            ...options,
            degradeOnHideFailure: options?.degradeOnHideFailure
              ?? (options?.channel === 'session' || options?.channel === 'view'),
          };
          result = browserUiTransitionGateRef.current.run((transition) => {
            browserTransitionPublishingRef.current += 1;
            return settleBrowserUiPublicationAfterCommit({
              publish: async () => {
                try {
                  return await publish(transition);
                } finally {
                  // Nested updates may share this publication only until the
                  // callback (including async work) has settled. Once the commit
                  // marker is queued, later effects need their own hide barrier.
                  browserTransitionPublishingRef.current -= 1;
                }
              },
              waitForCommit: requestBrowserUiCommitAck,
            });
          }, transitionOptions);
        } catch (error) {
          finishRequest();
          throw error;
        }
        if (result && typeof result.then === 'function') {
          return Promise.resolve(result).finally(finishRequest);
        }
        finishRequest();
        return result;
      }, [browserSessionCommandEchoGuard, requestBrowserUiCommitAck]);
      const setCurrentView = useCallback((nextView) => {
        const resolvedView = typeof nextView === 'function'
          ? nextView(currentViewRef.current)
          : nextView;
        // A publication that already owns the native-surface hide lease must be
        // urgent: the later React-commit marker is only authoritative when the
        // target tree cannot remain deferred behind it. Normal view switches keep
        // the main-branch transition priority so lazy chunks stay interruptible.
        if (browserTransitionPublishingRef.current > 0) {
          setCurrentViewState(resolvedView);
          return true;
        }
        const context = browserSurfaceTransitionContextRef.current;
        const keepsDesktopBrowserVisible = !context.compact && (
          resolvedView === 'chat'
          || (resolvedView === 'scheduled' && context.scheduledRunChat)
        );
        const needsNativeHide = !!context.visible && !keepsDesktopBrowserVisible;
        if (!needsNativeHide) {
          browserUiTransitionGateRef.current.invalidate('view');
          scheduleViewTransition(() => setCurrentViewState(resolvedView));
          return true;
        }
        return runBrowserUiTransition(() => {
          setCurrentViewState(resolvedView);
        }, {
          channel: 'view',
          hideMode: 'visible',
        });
      }, [runBrowserUiTransition]);
      const publishRightDockOcclusion = useCallback((occlusionId, publish) => (
        browserUiTransitionGateRef.current.run(({ isCurrent }) => {
          if (!isCurrent()) return false;
          const published = publish();
          if (published === false) return false;
          setRightDockOcclusionPublications((current) => (
            current.includes(occlusionId) ? current : [...current, occlusionId]
          ));
          return requestBrowserUiCommitAck().then(() => true);
        }, {
          channel: `right-dock-occlusion:${occlusionId}`,
          hideMode: 'visible',
        })
      ), [requestBrowserUiCommitAck]);
      const releaseRightDockOcclusion = useCallback((occlusionId) => {
        browserUiTransitionGateRef.current.invalidate(`right-dock-occlusion:${occlusionId}`);
        setRightDockOcclusionPublications((current) => (
          current.includes(occlusionId)
            ? current.filter((id) => id !== occlusionId)
            : current
        ));
      }, []);
      const selectRightDockPanel = useCallback((panelId, sessionId, publishSelection) => {
        const selectedSessionId = sessionId || browserSessionIdRef.current;
        if (!selectedSessionId) return false;
        const publish = ({ isCurrent }) => {
          const selectionIsCurrent = () => (
            isCurrent() && browserSessionIdRef.current === selectedSessionId
          );
          if (!selectionIsCurrent()) return false;
          const childPublished = publishSelection?.({
            isCurrent: selectionIsCurrent,
            sessionId: selectedSessionId,
          });
          if (childPublished === false || !selectionIsCurrent()) return false;
          setBrowserPaneStates((current) => (
            panelId === 'browser'
              ? activateBrowserPane(current, selectedSessionId)
              : selectArtifactsPane(current, selectedSessionId)
          ));
          return true;
        };
        const context = browserSurfaceTransitionContextRef.current;
        return runBrowserUiTransition(publish, {
          channel: 'right-dock',
          hideMode: panelId !== 'browser' && context.sessionId === selectedSessionId
            ? 'visible'
            : 'none',
        });
      }, [runBrowserUiTransition]);
      const closeBrowserDock = useCallback((sessionId) => {
        const selectedSessionId = sessionId || browserSessionIdRef.current;
        if (!selectedSessionId) return;
        const context = browserSurfaceTransitionContextRef.current;
        void runBrowserUiTransition(() => {
          setBrowserPaneStates((current) => closeBrowserPane(current, selectedSessionId));
        }, {
          channel: 'right-dock',
          hideMode: context.sessionId === selectedSessionId ? 'visible' : 'none',
        });
      }, [runBrowserUiTransition]);
      const openBrowserDock = useCallback(async () => {
        const requestedSessionId = browserSessionId;
        if (!browserNativeDisplayAvailable || !requestedSessionId || !browserPaneAllowed) return;
        setBrowserPaneStates((current) => activateBrowserPane(current, requestedSessionId));
        if (browserActive) return;
        const attempt = browserOpenAttemptSequenceRef.current + 1;
        browserOpenAttemptSequenceRef.current = attempt;
        browserOpenAttemptsRef.current[requestedSessionId] = attempt;
        setBrowserOpenStates((current) => (
          beginBrowserOpen(current, requestedSessionId, attempt)
        ));
        try {
          const prepared = await invokeTauri('browser_prepare', { sessionId: requestedSessionId });
          if (
            !browserUiCommitMountedRef.current
            || browserOpenAttemptsRef.current[requestedSessionId] !== attempt
          ) return;
          if (!prepared || prepared.sessionId !== requestedSessionId) {
            throw new Error('browser_prepare returned an invalid session identity');
          }
          setBrowserSessions((current) => ({ ...current, [requestedSessionId]: true }));
          setBrowserOpenStates((current) => (
            settleBrowserOpen(current, requestedSessionId, attempt, 'idle')
          ));
        } catch (error) {
          if (
            !browserUiCommitMountedRef.current
            || browserOpenAttemptsRef.current[requestedSessionId] !== attempt
          ) return;
          setBrowserOpenStates((current) => settleBrowserOpen(
            current,
            requestedSessionId,
            attempt,
            'failed',
            typeof error === 'string' ? error : String(error),
          ));
        }
      }, [
        browserActive,
        browserNativeDisplayAvailable,
        browserPaneAllowed,
        browserSessionId,
      ]);
      useEffect(() => {
        if (!browserNativeDisplayAvailable) {
          browserLifecycleListenersReadyRef.current = null;
          setBrowserSessions({});
          setBrowserPaneStates({});
          setBrowserOpenStates({});
          browserOpenAttemptsRef.current = {};
          return;
        }
        let disposed = false;
        let reconciliationTimer = 0;
        let listenerRegistrationFailed = false;
        const unlisteners = [];
        // Register listeners before the fallback status query. Reversing this
        // order can miss browser:activated between the query and registration;
        // Rust will not resend it after setting the activated marker.
        const registerActivated = tauriEvents.listen('browser:activated', (event) => {
          if (disposed) return;
          const sessionId = event.payload?.sessionId;
          if (!sessionId) return;
          browserLifecycleEventEpochRef.current.advance(sessionId);
          browserLifecycleStatusRequestEpochRef.current.advance(sessionId);
          setBrowserSessions((current) => ({ ...current, [sessionId]: true }));
          setBrowserPaneStates((current) => activateBrowserPane(current, sessionId));
          const attempt = browserOpenAttemptsRef.current[sessionId] || 0;
          setBrowserOpenStates((current) => (
            settleBrowserOpen(current, sessionId, attempt, 'idle')
          ));
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch((error) => {
          if (disposed) return;
          listenerRegistrationFailed = true;
          console.error('[browser] failed to register browser:activated listener', error);
        });
        const registerStopped = tauriEvents.listen('browser:stopped', (event) => {
          if (disposed) return;
          const sessionId = event.payload?.sessionId;
          browserLifecycleEventEpochRef.current.advance(sessionId || null);
          browserLifecycleStatusRequestEpochRef.current.advance(sessionId || null);
          if (sessionId) {
            browserOpenAttemptSequenceRef.current += 1;
            browserOpenAttemptsRef.current[sessionId] = browserOpenAttemptSequenceRef.current;
            setBrowserSessions((current) => {
              const next = { ...current };
              delete next[sessionId];
              return next;
            });
            setBrowserPaneStates((current) => removeBrowserPaneState(current, sessionId));
            setBrowserOpenStates((current) => {
              const next = { ...current };
              delete next[sessionId];
              return next;
            });
          } else {
            browserOpenAttemptsRef.current = {};
            setBrowserSessions({});
            setBrowserPaneStates({});
            setBrowserOpenStates({});
          }
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch((error) => {
          if (disposed) return;
          listenerRegistrationFailed = true;
          console.error('[browser] failed to register browser:stopped listener', error);
        });
        const readiness = awaitBrowserListenerReadiness(
          [registerActivated, registerStopped],
          {
            schedule: window.setTimeout.bind(window),
            cancel: window.clearTimeout.bind(window),
          },
        ).then((listenersReady) => {
          if (disposed || browserLifecycleListenersReadyRef.current !== readiness) return false;
          if (!listenersReady) {
            listenerRegistrationFailed = true;
            console.error('[browser] lifecycle listener registration timed out; enabling reconciliation');
          }
          if (listenerRegistrationFailed) {
            const reconcileCurrentSession = () => {
              const requestedSessionId = browserSessionIdRef.current;
              if (!requestedSessionId) return;
              const eventEpoch = browserLifecycleEventEpochRef.current.snapshot(requestedSessionId);
              const requestEpoch = browserLifecycleStatusRequestEpochRef.current.advance(
                requestedSessionId,
              );
              invokeTauri('browser_status', { sessionId: requestedSessionId }).then((st) => {
                if (
                  disposed
                  || browserSessionIdRef.current !== requestedSessionId
                  || !browserLifecycleEventEpochRef.current.isCurrent(
                    requestedSessionId,
                    eventEpoch,
                  )
                  || !browserLifecycleStatusRequestEpochRef.current.isCurrent(
                    requestedSessionId,
                    requestEpoch,
                  )
                  || !st
                  || st.sessionId !== requestedSessionId
                ) return;
                if (!st.running && !st.restoreError) {
                  setBrowserSessions((current) => {
                    const next = { ...current };
                    delete next[requestedSessionId];
                    return next;
                  });
                  setBrowserPaneStates((current) => (
                    removeBrowserPaneState(current, requestedSessionId)
                  ));
                  return;
                }
                setBrowserSessions((current) => ({ ...current, [requestedSessionId]: true }));
                setBrowserPaneStates((current) => restoreBrowserPane(current, requestedSessionId));
              }).catch((error) => {
                console.error('[browser] lifecycle reconciliation failed', error);
              });
            };
            reconcileCurrentSession();
            reconciliationTimer = window.setInterval(reconcileCurrentSession, 2000);
          }
          return true;
        });
        // The session hydration effect below awaits this exact promise. Merely
        // starting listen() first is insufficient because Tauri listener
        // registration itself is asynchronous.
        browserLifecycleListenersReadyRef.current = readiness;
        return () => {
          disposed = true;
          if (browserLifecycleListenersReadyRef.current === readiness) {
            browserLifecycleListenersReadyRef.current = null;
          }
          if (reconciliationTimer) window.clearInterval(reconciliationTimer);
          unlisteners.forEach((unlisten) => {
            if (unlisten) unlisten();
          });
        };
      }, [browserNativeDisplayAvailable]);
      useEffect(() => {
        const syncVisibility = () => setBrowserDocumentHidden(document.visibilityState === 'hidden');
        const handlePageHide = () => setBrowserDocumentHidden(true);
        document.addEventListener('visibilitychange', syncVisibility);
        window.addEventListener('pagehide', handlePageHide);
        window.addEventListener('pageshow', syncVisibility);
        return () => {
          document.removeEventListener('visibilitychange', syncVisibility);
          window.removeEventListener('pagehide', handlePageHide);
          window.removeEventListener('pageshow', syncVisibility);
        };
      }, []);
      // Query the session after a WebView reload or chat switch. Restore its entry
      // when pages exist, but retain other sessions so they recover when revisited.
      useEffect(() => {
        if (!browserNativeDisplayAvailable || !browserSessionId) return;
        let disposed = false;
        const requestedSessionId = browserSessionId;
        const readiness = browserLifecycleListenersReadyRef.current;
        if (!readiness) return () => { disposed = true; };
        Promise.resolve(readiness).then(() => {
          if (
            disposed
            || browserLifecycleListenersReadyRef.current !== readiness
            || browserSessionIdRef.current !== requestedSessionId
          ) return null;
          const eventEpoch = browserLifecycleEventEpochRef.current.snapshot(requestedSessionId);
          const requestEpoch = browserLifecycleStatusRequestEpochRef.current.advance(
            requestedSessionId,
          );
          return invokeTauri('browser_status', { sessionId: requestedSessionId }).then((st) => ({
            eventEpoch,
            requestEpoch,
            st,
          }));
        }).then((snapshot) => {
          const st = snapshot?.st;
          if (
            disposed
            || !st
            || browserSessionIdRef.current !== requestedSessionId
            || !browserLifecycleEventEpochRef.current.isCurrent(
              requestedSessionId,
              snapshot.eventEpoch,
            )
            || !browserLifecycleStatusRequestEpochRef.current.isCurrent(
              requestedSessionId,
              snapshot.requestEpoch,
            )
            || st.sessionId !== requestedSessionId
          ) return;
          if (!st.running && !st.restoreError) {
            setBrowserSessions((current) => {
              const next = { ...current };
              delete next[requestedSessionId];
              return next;
            });
            setBrowserPaneStates((current) => (
              removeBrowserPaneState(current, requestedSessionId)
            ));
            return;
          }
          setBrowserSessions((current) => ({ ...current, [requestedSessionId]: true }));
          // Expand a restored workspace on first discovery. Preserve any explicit
          // collapse or artifact selection made for this session in this window.
          setBrowserPaneStates((current) => restoreBrowserPane(current, requestedSessionId));
        }).catch((error) => {
          if (!disposed) console.error('[browser] initial lifecycle hydration failed', error);
        });
        return () => { disposed = true; };
      }, [browserNativeDisplayAvailable, browserSessionId]);
      // Compact layouts keep the fullscreen browser view; desktop uses the chat dock.
      useEffect(() => {
        if (!browserActive && currentView === 'browser') {
          setCurrentView('chat');
        }
      }, [browserActive, currentView, setCurrentView]);
      const codexAcpSupported = usePlatformCapability('acpCodeMode') && (isWeb || !!platformCapabilities.codexAcpSupported);
      const [codexSessions, setCodexSessions] = useState([]);
      const [codexDraftEpoch, setCodexDraftEpoch] = useState(0);
      const [activeCodexId, setActiveCodexId] = useState(() => {
        try {
          return localStorage.getItem('pinvou_codex_active_session') || null;
        } catch {
          return null;
        }
      });
      const [codexBusyBySession, setCodexBusyBySession] = useState({});
      // 代码会话等待用户输入（request_user_input 挂起）的会话集合：侧边栏用
      // 「等待你的选择」橙色点提示，与 running 灰点区分——后台会话提问不再无感知。
      const [codexWaitingInputBySession, setCodexWaitingInputBySession] = useState({});
      // 全局事件监听器按 id 判断是否为代码会话（监听器注册一次，不能闭包旧列表）。
      const codexSessionIdsRef = useRef(new Set());
      // 进入设置前的页面（openSettingsSection 记录），关闭设置时原路返回。
      const settingsReturnViewRef = useRef(null);
      useEffect(() => {
        codexSessionIdsRef.current = new Set(codexSessions.map(session => session && session.id));
      }, [codexSessions]);
      const refreshCodexSessions = useCallback(async () => {
        if (!codexAcpSupported || !isTauriAvailable()) {
          setCodexSessions([]);
          return [];
        }
        const sessions = await listAcpSessions();
        const next = Array.isArray(sessions) ? sessions : [];
        setCodexSessions(next);
        return next;
      }, [codexAcpSupported]);
      const updateActiveCodexSession = useCallback((id) => {
        const next = id || null;
        setActiveCodexId(next);
        try {
          if (next) localStorage.setItem('pinvou_codex_active_session', next);
          else localStorage.removeItem('pinvou_codex_active_session');
        } catch {
          // WebView 禁用 storage 时仍允许当前窗口内切换。
        }
      }, []);
      useEffect(() => {
        if (!codexAcpSupported || !isTauriAvailable()) {
          // Clear the code-session mirror when bridge capabilities change to avoid stale unreachable sessions;
          // the synchronous setState guarantees it takes effect in this render pass.
          setCodexSessions([]);
          return;
        }
        let disposed = false;
        const unlisteners = [];
        refreshCodexSessions().catch(error => {
          if (!disposed) console.warn('[codex] list sessions failed', error);
        });
        tauriEvents.listen('acp:event', (message) => {
          if (disposed) return;
          const incoming = message && message.payload;
          const sessionId = incoming && incoming.sessionId;
          const type = incoming && incoming.event && incoming.event.type;
          if (!sessionId || !type) return;
          if (type === 'turn_started') {
            setCodexBusyBySession(current => ({ ...current, [sessionId]: true }));
            refreshCodexSessions().catch(() => {});
          } else if (type === 'turn_completed') {
            setCodexBusyBySession(current => ({ ...current, [sessionId]: false }));
            refreshCodexSessions().catch(() => {});
          }
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        tauriEvents.listen('session:deleted', () => {
          if (!disposed) refreshCodexSessions().catch(() => {});
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        // 原生（品悟）代码会话的 turn 走 chat:* 事件：busy 徽标与 ACP 会话同机制，
        // 只跟踪代码会话列表内的 session，普通聊天会话不影响。
        ['chat:turn_started', 'chat:done'].forEach(eventName => {
          tauriEvents.listen(eventName, (message) => {
            if (disposed) return;
            const sessionId = message && message.payload && message.payload.session_id;
            if (!sessionId || !codexSessionIdsRef.current.has(sessionId)) return;
            setCodexBusyBySession(current => ({ ...current, [sessionId]: eventName === 'chat:turn_started' }));
            if (eventName === 'chat:done') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: false }));
            }
            refreshCodexSessions().catch(() => {});
          }).then(unlisten => {
            if (disposed) unlisten();
            else unlisteners.push(unlisten);
          }).catch(() => {});
        });
        // 后台会话提问（request_user_input 挂起）时点亮「等待你的选择」提示，
        // 收口（提交/取消/超时→tool_end）后熄灭；turn 结束由上面 chat:done 兜底。
        ['chat:user_input_required', 'chat:tool_end'].forEach(eventName => {
          tauriEvents.listen(eventName, (message) => {
            if (disposed) return;
            const p = message && message.payload || {};
            const sessionId = p.session_id;
            if (!sessionId || !codexSessionIdsRef.current.has(sessionId)) return;
            if (eventName === 'chat:user_input_required') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: true }));
              setCodexBusyBySession(current => ({ ...current, [sessionId]: true }));
            } else if (p.name === 'request_user_input') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: false }));
              // 只有提问收口才刷新会话列表；普通工具 tool_end 不动列表，避免
              // 工具密集 turn 下每个 chat:tool_end 都触发一次 IPC + 重渲染。
              refreshCodexSessions().catch(() => {});
            }
          }).then(unlisten => {
            if (disposed) unlisten();
            else unlisteners.push(unlisten);
          }).catch(() => {});
        });
        return () => {
          disposed = true;
          unlisteners.forEach(unlisten => { unlisten(); });
        };
      }, [codexAcpSupported, refreshCodexSessions]);
      // 供全局事件监听器读取最新视图状态（监听器只注册一次，不能闭包旧值）。
      // latest-ref render-time mirror: event callbacks (fired post-commit) read the latest value, an officially
      // sanctioned React escape hatch; writing back via an effect would introduce a brief stale-value window around commit.
      const activeChatRef = useRef(activeChat);
      activeChatRef.current = activeChat;
      const currentViewRef = useRef(currentView);
      currentViewRef.current = currentView;
      useEffect(() => {
        if (!isTauriAvailable()) return;
        const guard = createPetActivationGuard();
        let disposed = false;
        let unlisten = null;
        tauriEvents.listen('pet:activation_guard', guard.arm).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        // 只拦截由上面的桌宠专用事件武装后的一个 click。普通 window.focus、
        // Alt-Tab、任务栏回焦和其它平台不会触发保护，也就不会丢掉正常首击。
        window.addEventListener('click', guard.handleClick, true);
        return () => {
          disposed = true;
          if (unlisten) unlisten();
          window.removeEventListener('click', guard.handleClick, true);
        };
      }, []);
      useEffect(() => {
        const liveBridge = window.TauriBridge || bridge;
        if (!liveBridge?.monitor || typeof liveBridge.monitor.startMonitorPolling !== 'function') return;
        if (currentView === 'monitor') {
          liveBridge.monitor.startMonitorPolling();
          return () => { if (typeof liveBridge.monitor.stopMonitorPolling === 'function') liveBridge.monitor.stopMonitorPolling(); };
        }
      }, [currentView]);
      // 工具商店/卡片用 Tailwind dark: 变体(darkMode:'class'),全局挂 <html>.dark 让其随 app 主题切换
      useEffect(() => { document.documentElement.classList.toggle('dark', activeTheme === 'dark'); }, [activeTheme]);
      // 厂商预装本地大模型首屏检测:仅启动一次,检测「预装但未启用」本地大模型环境(后端短路保证普通机零开销)。
      useEffect(() => {
        if (bridge.available && platformCapabilities.localVllmSupported) {
          bridge.vllm.detectLocalVllmSetup();
        }
      }, [platformCapabilities.localVllmSupported]);
      const [vllmDeclineConfirm, setVllmDeclineConfirm] = useState(false); // 引导框「不再提醒」二次确认子态
      const [language, setLanguage] = useState(() => {
        const systemLanguage = initialSystemLanguage();
        if (!isWeb) return systemLanguage;
        try {
          const value = window.localStorage.getItem('pinvou.web.language');
          // 首帧引导(文件尾 ensureLanguage)已保证 localStorage 选中的语言词典就位
          return value && dict[value] ? value : systemLanguage;
        } catch { return systemLanguage; }
      });
      // 语言切换统一走该门(handleSetLanguage):装载完成乱序时只落地最新选择。
      const switchToLanguage = useRef(createLatestLanguageGate()).current;
      // UI 语言为 en/ja 时确保 personas-i18n overlay 已加载(覆盖「系统中文 + 手动切
      // 英/日 UI」、index.html 快速路径跳过的场景),加载完成 bump 一次让卡名重渲染。
      const [, setPersonaI18nTick] = useState(0);
      useEffect(() => {
        if (language === 'en' || language === 'ja') {
          ensurePersonaI18nOverlay(() => setPersonaI18nTick(v => v + 1));
        }
      }, [language]);
      const [superPerm, setSuperPerm] = useState(false);
      const defaultTaskCompletedNotif = platformCapabilities.taskCompletionNotificationsDefault !== false;
      const [taskCompletedNotif, setTaskCompletedNotif] = useState(defaultTaskCompletedNotif);
      // search 后端配置:provider 默认 bing(对齐 bridge prefs::SearchProvider::default());
      // bs.settings 加载后 useEffect 同步进来。
      const [searchProvider, setSearchProvider] = useState('bing');
      const [enabledSearchProviders, setEnabledSearchProviders] = useState(['bing']);
      const [searchKeyDrafts, setSearchKeyDrafts] = useState({});
      const [searchKeyActions, setSearchKeyActions] = useState({});
      const searchConfigInitRef = useRef(false);
      const uiPrefsInitRef = useRef(false);

      const [isSidebarOpen, setIsSidebarOpen] = useState(false);
      const [openSidePanelCount, setOpenSidePanelCount] = useState(0);
      const restoreSidebarAfterConstraintRef = useRef(false);
      // 移动壳层只作用于 Web 端紧凑视口：底部 Tab + 顶栏，侧栏只保留抽屉形态。
      const compactViewport = useCompactViewport();
      const isCompactShell = isWeb && compactViewport;
      const browserDockAvailable = !isCompactShell
        && browserNativeDisplayAvailable;
      // On narrow windows, an open right panel takes priority over the left sidebar.
      // This is a temporary layout constraint; restore the user's sidebar choice later.
      useEffect(() => {
        if (isCompactShell) return;
        const fitWorkspace = () => {
          const constrained = openSidePanelCount > 0 && window.innerWidth < 1320;
          if (constrained) {
            setIsSidebarOpen((current) => {
              if (current) restoreSidebarAfterConstraintRef.current = true;
              return false;
            });
          } else if (restoreSidebarAfterConstraintRef.current) {
            restoreSidebarAfterConstraintRef.current = false;
            setIsSidebarOpen(true);
          }
        };
        fitWorkspace();
        window.addEventListener('resize', fitWorkspace);
        return () => window.removeEventListener('resize', fitWorkspace);
      }, [isCompactShell, isSidebarOpen, openSidePanelCount]);
      // iOS Safari 上 100dvh 不等于真实可见高度（动态工具栏/安全区），用 visualViewport 兜底。
      const visualViewportHeight = useVisualViewportHeight();
      // iOS Safari 聚焦输入框时会尝试滚动整个文档。紧凑 Web 壳层本身已经按
      // visualViewport 缩高，若再允许文档级平移，整个应用会被推到键盘上方，只剩白屏。
      useEffect(() => {
        if (!isCompactShell) return;

        const html = document.documentElement;
        const body = document.body;
        html.classList.add('compact-web-viewport');

        let frame = 0;
        let settleTimer = 0;
        const resetDocumentScroll = () => {
          window.cancelAnimationFrame(frame);
          window.clearTimeout(settleTimer);
          const reset = () => {
            window.scrollTo(0, 0);
            html.scrollTop = 0;
            body.scrollTop = 0;
          };
          frame = window.requestAnimationFrame(reset);
          // Safari 的自动聚焦平移可能晚于 focusin/viewport resize，再收敛一次。
          settleTimer = window.setTimeout(reset, 120);
        };

        const viewport = window.visualViewport;
        document.addEventListener('focusin', resetDocumentScroll);
        viewport?.addEventListener('resize', resetDocumentScroll);
        viewport?.addEventListener('scroll', resetDocumentScroll);
        resetDocumentScroll();

        return () => {
          html.classList.remove('compact-web-viewport');
          document.removeEventListener('focusin', resetDocumentScroll);
          viewport?.removeEventListener('resize', resetDocumentScroll);
          viewport?.removeEventListener('scroll', resetDocumentScroll);
          window.cancelAnimationFrame(frame);
          window.clearTimeout(settleTimer);
        };
      }, [isCompactShell]);
      const [mobileMoreOpen, setMobileMoreOpen] = useState(false);
      const canDetachWindows = can('detachWindows');
      const [chatPrefill, setChatPrefill] = useState('');
      // Append mode for failure-recovery prefills (replaced template prefills keep
      // whole-draft replacement semantics, re-review #4).
      const [chatPrefillAppend, setChatPrefillAppend] = useState(false);
      const [searchOverlayOpen, setSearchOverlayOpen] = useState(false);
      const composerPrefillSeenRef = useRef(0);
      const scheduledTaskAutoOpenSeenRef = useRef(null);
      const [personaEditor, setPersonaEditor] = useState(null); // 聊天里"存入卡牌池"草稿 → App 级编辑器
      const [savedConfirm, setSavedConfirm] = useState(null); // 存入成功 → iOS 确认窗 {name}
      const [poolMyOnly, setPoolMyOnly] = useState(false); // 跳卡池时是否直接落「我的卡牌」筛选(从确认窗"去查看"进来=true)
      const [webAccessOpen, setWebAccessOpen] = useState(false);
      const [publishedBrowserOverlayIntent, setPublishedBrowserOverlayIntent] = useState('');
      const [settingsUpdateFocusTick, setSettingsUpdateFocusTick] = useState(0);
      const [settingsInitialSection, setSettingsInitialSection] = useState('general');
      // 收纳 toast「前往查看」→ 对话管理页并直接展开「已收纳」面板(一次性信号,SearchView 消费后复位)
      const [searchShowArchived, setSearchShowArchived] = useState(false);
      const [petFocusComposerTick, setPetFocusComposerTick] = useState(0);
      const petSnapshotRef = useRef([]);
      const petSnapshotSequenceRef = useRef(0);
      // 上次广播的快照内容指纹。ref 而非 effect 局部变量:effect 依赖
      // bs.sessionBusy/sessions 引用,多会话并发时每次 notify 都换引用导致
      // effect 重跑;指纹必须跨重跑保持,才能挡住"内容没变"的重复广播。
      const petSnapshotFingerprintRef = useRef('');

      // ── 多窗口(撕离/tear-off):长按标签 → 浮起跟手 → 拖到目标屏 → 松手 → 该屏最大化打开 ──
      // dragAvatar = 被拎起的标签副本(跟随光标的 DOM 元素);null=没在拖。原生只判落点,视觉全在这。
      const [dragAvatar, setDragAvatar] = useState(null); // {key,label,dx,dy,w,h,x,y}
      const dragOffsetRef = useRef({ dx: 0, dy: 0 });
      // Stable useCallback: the per-item onPickUp closure caches of
      // RecentItem/NavItem (see renderSidebarTaskItem) rely on this reference
      // staying constant across renders so fresh callbacks per render cannot
      // defeat the memo.
      const beginTearOff = useCallback((kind, id, label, info) => {
        const inv = isTauriAvailable() ? invokeTauri : null;
        if (!inv || !info) return;
        inv('begin_detach_drag', { kind, id: id == null ? null : id });
        dragOffsetRef.current = { dx: info.dx, dy: info.dy };
        setDragAvatar({
          key: kind + ':' + (id == null ? '' : id), label: label || kind,
          w: info.w, h: info.h, x: info.startX - info.dx, y: info.startY - info.dy,
        });
        if (window.getSelection) { const s = window.getSelection(); if (s && s.removeAllRanges) s.removeAllRanges(); }
      }, []);
      // 拖拽中:光标移动 → 更新 avatar 位置(光标 - 抓取偏移,相对位置锁定);禁选 + 抓手光标。
      const dragAvatarActive = !!dragAvatar;
      useEffect(() => {
        if (!dragAvatarActive) return;
        const prevUS = document.body.style.userSelect, prevCur = document.body.style.cursor;
        document.body.style.userSelect = 'none';
        document.body.style.cursor = 'grabbing';
        const onMove = (e) => {
          const o = dragOffsetRef.current;
          setDragAvatar(a => a ? { ...a, x: e.clientX - o.dx, y: e.clientY - o.dy } : a);
        };
        window.addEventListener('pointermove', onMove);
        return () => {
          window.removeEventListener('pointermove', onMove);
          document.body.style.userSelect = prevUS;
          document.body.style.cursor = prevCur;
        };
      }, [dragAvatarActive]);
      // 原生拖拽结束(松手/取消)→ 收起 avatar。
      useEffect(() => {
        if (!isTauriAvailable()) return;
        // If unmount happens after listen() resolves, unlisten immediately
        // to avoid a leak (same policy as browser:activated).
        let disposed = false;
        let un;
        tauriEvents.listen('detach:drag-ended', () => setDragAvatar(null)).then(f => {
          if (disposed) f();
          else un = f;
        });
        return () => { disposed = true; if (un) un(); };
      }, []);

      // 兜底 zh:词典 chunk 装载失败时按 zh 渲染而非白屏(与 PetWindow/ReaderApp 同口径)。
      const t = dict[language] || dict.zh;
      // The desk-pet reply consumption loop (mounted once) reads a latest-ref mirror of current-language error copy.
      const petI18nTextRef = useRef(null);
      petI18nTextRef.current = t.uiMainApp;
      // 静态 HTML 的 <title>/<html lang> 与非模块脚本(远程文件选择器、web bootstrap)拿不到语言上下文,
      // 在此按当前语言同步,并把选择器/bootstrap 错误文案暴露给 platform/web/ 下的脚本。
      // 桌宠窗口标题由 PetWindow 自行同步(主包不做桌宠检测,见 pet_bootstrap_isolation 测试)。
      useEffect(() => {
        const misc = t.uiPlatformMisc;
        if (!misc) return;
        document.title = misc.appTitle;
        if (misc.htmlLang) document.documentElement.lang = misc.htmlLang;
        window.PinvouHostFilePickerStrings = misc.hostFilePicker;
        // platform/web/bootstrap.js 的 invoke 拒绝错误文案（web bootstrap 内置中文兜底）。
        window.PinvouWebClientStrings = misc.webClientErrors;
      }, [t]);
      // 有可用新版 → 侧边栏设置图标亮红点（不弹窗不打断）
      const hasUpdate = !!(bs && bs.updateInfo && bs.updateInfo.available);
      const isWebAccessConnected = !!(bs && bs.webAccess && bs.webAccess.web_client_connected);
      function handleOpenWebAccess() {
        if (!can('webAccessAdmin')) return;
        setWebAccessOpen(true);
      }

      // 冷路径预取:该 modal 唯一入口在聊天「存入卡牌池」,此前 cardpool chunk
      // 可能从未加载;提前发起 import 避免打开动作撞上 chunk 冷启动/失败。
      // The reference must stay stable across renders: it is passed through to
      // the structurally compared memoized ConversationTurn; a fresh identity
      // on every render would invalidate that memo for the whole timeline
      // during streaming.
      const handleOpenPersonaEditor = useCallback((initial) => {
        prefetchView('cardpool');
        setPersonaEditor({ initial });
      }, []);

      // Sync from bridge state
      // One-shot bootstrap: backfill the search-config draft baseline when bridge settings first arrive, then use draft mode
      // (saved only on confirm) so the effect never overwrites unsaved local edits with old on-disk values.
      const initSearchConfigFromSettings = (settings) => {
        const search = settings.search || {};
        const credentials = search.credentials || {};
        const saved = {
          provider: search.provider || 'bing',
          apiKey: search.api_key || '',
          credentials,
          enabledProviders: Array.isArray(search.enabled_providers) && search.enabled_providers.length
            ? [...new Set(['bing', ...search.enabled_providers])]
            : ['bing', search.provider || 'bing'].filter(Boolean),
        };
        const drafts = {};
        const actions = {};
        SEARCH_KEY_PROVIDERS.forEach(p => {
          drafts[p] = '';
          actions[p] = 'keep_existing';
        });
        if (saved.apiKey && saved.provider !== 'bing') {
          drafts[saved.provider] = saved.apiKey;
          actions[saved.provider] = 'replace';
        }
        setSearchProvider(saved.provider);
        setEnabledSearchProviders(saved.enabledProviders);
        setSearchKeyDrafts(drafts);
        setSearchKeyActions(actions);
        searchConfigInitRef.current = true;
      };
      // One-shot bootstrap: restore persisted UI language/theme and notification prefs (desktop); on Web the language uses local storage.
      const initUiPrefsFromSettings = (settings) => {
        if (isWeb) {
          // On web the color-scheme preference lives in localStorage; with no
          // choice made (first visit / storage disabled) follow the system,
          // light when undeterminable.
          let storedScheme = null;
          try { storedScheme = window.localStorage.getItem(COLOR_SCHEME_STORAGE_KEY); } catch { /* silently degrade when WebView disables storage */ }
          setColorScheme(normalizeColorScheme(storedScheme));
        } else {
          const lang = TAG_TO_LANG[settings.language];
          // 落盘语言可能尚未装载(en/ja 惰性 chunk);ensure 后再切,失败停在系统语言
          if (lang && lang !== language) ensureLanguage(lang).then((ok) => { if (ok) setLanguage(lang); }).catch(() => {});
          // `color_scheme` (light/dark/system) is the authoritative preference;
          // fresh installs keep `system`. `theme` (genesis/liquid-light/liquid-dark)
          // is the legacy field: the backend derives color_scheme from it once for
          // old settings missing the key (prefs.rs), and the frontend no longer
          // reads `theme`, so the two cannot diverge.
          setColorScheme(normalizeColorScheme(settings.color_scheme));
        }
        const notifications = settings.notifications || {};
        setTaskCompletedNotif(notifications.task_completed !== false && notifications.enabled !== false);
        uiPrefsInitRef.current = true;
      };
      useEffect(() => {
        if (!bs) return;
        // activeChat 始终跟随 bridge(含 null:草稿态清掉近期列表高亮)。仅在物化成
        // 真实 session(非 null)时才强制切回 chat 视图——草稿态/删会话不该把用户从
        // monitor/settings 拽走。
        const nextSessionId = bs.activeSessionId;
        const bridgeTransition = browserBridgeSessionTransitionRef.current;
        const bridgeObservation = browserSessionCommandEchoGuard.observe(nextSessionId);
        const isCommandEcho = bridgeObservation.type === 'command-echo';
        // Intentional nullish check: an HMR-restored ref may be either null or undefined.
        let bridgeSessionNeedsSync = !isCommandEcho
          && bridgeTransition != null
          && bridgeTransition.sessionId !== nextSessionId;
        if (!isCommandEcho && bs.activeSessionId !== activeChat) {
          bridgeSessionNeedsSync = true;
        }
        if (bridgeSessionNeedsSync) {
          const publishSession = ({ isCurrent }) => {
            if (!isCurrent()) return false;
            setActiveChat(nextSessionId);
            const publishedView = currentViewRef.current;
            if (nextSessionId && publishedView !== 'codex' && publishedView !== 'monitor' && publishedView !== 'settings' && publishedView !== 'search' && publishedView !== 'scheduled' && publishedView !== 'browser') {
              // A normal bridge session must not inherit code-only sidebar/draft state.
              setCodeModeOn(false);
              setCurrentView('chat');
            }
            return true;
          };
          // Every distinct bridge target must enter the serialized gate even
          // while an older hide ACK is pending. Issuing the newer ticket makes
          // the older publication stale, and its independently owned hide lease
          // keeps the native surface hidden until the latest React commit. A
          // token (not just the session id) avoids an old B→C→B completion from
          // clearing the newest B request.
          if (bridgeTransition?.sessionId !== nextSessionId) {
            const transitionToken = { sessionId: nextSessionId };
            browserBridgeSessionTransitionRef.current = transitionToken;
            const transitionResult = runBrowserUiTransition(publishSession, {
              channel: 'session',
              hideMode: 'workspace',
              serialize: true,
              sessionSource: 'bridge',
            });
            void Promise.resolve(transitionResult).finally(() => {
              if (browserBridgeSessionTransitionRef.current === transitionToken) {
                browserBridgeSessionTransitionRef.current = null;
              }
            });
          }
        }
        if (bs.superPermEnabled !== superPerm) setSuperPerm(bs.superPermEnabled);
        if (bs.composerPrefill && bs.composerPrefill.id && bs.composerPrefill.id !== composerPrefillSeenRef.current) {
          composerPrefillSeenRef.current = bs.composerPrefill.id;
          setChatPrefill(bs.composerPrefill.text || '');
          setChatPrefillAppend(!!bs.composerPrefill.append);
          // A composer prefill lands on the normal chat input: same rule — exit code
          // mode before materializing the chat view.
          setCodeModeOn(false);
          setCurrentView('chat');
        }
        if (bs.scheduledTaskAutoOpenId && bs.scheduledTaskAutoOpenId !== scheduledTaskAutoOpenSeenRef.current) {
          scheduledTaskAutoOpenSeenRef.current = bs.scheduledTaskAutoOpenId;
          setCurrentView('scheduled');
        }
        // UI 语言/主题:启动时从落盘 settings 恢复一次；无语言配置时后端已按系统 locale 补齐。
        if (!uiPrefsInitRef.current && bs.settings) initUiPrefsFromSettings(bs.settings);
        // 搜索配置：只在第一次从后端加载初始值，后续走草稿模式（确认后才保存并重启）。
        if (!searchConfigInitRef.current && bs.settings) initSearchConfigFromSettings(bs.settings);
        // The effect subscribes to the bridge snapshot bs; one-shot bootstrap/init flags are guarded by internal refs,
        // and the remaining deps (activeChat/currentView/language, etc.) are render-state reads — including them would
        // rerun the whole sync logic on every UI change. sessionSyncEpoch intentionally
        // retriggers reconciliation after the serialized browser session gate settles.
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [bs, sessionSyncEpoch]);

      function searchProviderKeyAction(provider) {
        return searchKeyActions[provider] || 'keep_existing';
      }
      function buildSearchSettingsPayload() {
        const baseSearch = (bs && bs.settings && bs.settings.search) || {};
        const credentials = { ...baseSearch.credentials };
        SEARCH_KEY_PROVIDERS.forEach(provider => {
          const action = searchProviderKeyAction(provider);
          const draft = searchKeyDrafts[provider] || '';
          if (action === 'delete' || (action === 'replace' && draft.trim())) {
            credentials[provider] = {
              ...credentials[provider],
              api_key: action === 'replace' ? draft.trim() : '',
              credential_action: action,
            };
          }
        });
        return {
          ...baseSearch,
          provider: searchProvider,
          enabled_providers: [...new Set(['bing', ...enabledSearchProviders, searchProvider])],
          api_key: null,
          credentials,
        };
      }
      // Scheduled-run status copy (depends on the current language
      // dictionary); defined before the derived useMemos below so they can
      // depend on it.
      const scheduledRunLabel = useCallback((value) => {
        return (t.uiScheduled.runStatus[value] || value || t.uiScheduled.unknown);
      }, [t]);

      // App re-renders in full on every bridge notify (including local UI
      // state changes unrelated to the sidebar). Every O(sessions) derivation
      // below is a useMemo over the real data slices: bridge subscription
      // snapshots are persistent projections whose unchanged slices keep
      // their references (pure chat streaming tokens only touch the chat
      // domain and keep the sessions domain identical), so these memos are
      // what let the sidebar derivations and the RecentItem memo actually
      // skip recomputation.

      // Build chat history from sessions
      const bridgeSessions = bs && bs.sessions;
      const bridgeSessionBusy = bs && bs.sessionBusy;
      const chatHistory = useMemo(() => {
        const sessionBusy = bridgeSessionBusy || {};
        return bridgeSessions ? bridgeSessions.map(s => {
          const isPlaceholder = !s.title || isDefaultChatTitle(s.title);
          const titlePresentation = isPlaceholder
            ? { text: t.newChat, attachments: [] }
            : sessionTitlePresentation(s.title, s.title_attachment_names);
          return {
            id: s.id,
            // The backend default title is one of the trilingual sentinels
            // (see isDefaultChatTitle; the bridge uses it to decide whether to
            // auto-rename) — map it to the current language at the display layer
            title: sessionTitlePlainText(titlePresentation),
            titleContent: titlePresentation.attachments.length
              ? <SessionAttachmentTitle presentation={titlePresentation} />
              : null,
            date: formatSessionDate(s.updated_at || s.created_at, language),
            updatedAt: s.updated_at || s.created_at || '',
            pinned: !!s.pinned,
            pinnedAt: s.pinned_at || '',
            working: !!sessionBusy[s.id], // concurrent sessions: is this session generating in the background
            // #445 binding: a bound work session carries workspacePath/Kind;
            // project grouping follows binding (the same signal as the safety
            // posture). Unbound sessions leave both values empty and stay in
            // the date view.
            workspacePath: s.workspace_binding || '',
            // A standalone 'bound' kind: shares the three-tier grouping with
            // the code/ACP 'project' kind, but is not a disguised
            // project-kind (review #452 finding 5).
            workspaceKind: s.workspace_binding ? WORKSPACE_KIND_BOUND : '',
            leadingIcon: <PinvouLogo className="h-[18px] w-[18px]" />,
            testId: 'regular-sidebar-item',
            menuTestId: 'regular-sidebar-menu',
          };
        }) : [];
      }, [bridgeSessions, bridgeSessionBusy, t, language]);
      const codexHistory = useMemo(() => codexSessions.map(session => ({
        id: session.id,
        title: (!session.title || isDefaultChatTitle(session.title))
          ? t.newChat
          : session.title,
        subtitle: session.workspace_kind === 'project'
          ? workspaceDisplayName(session.workspace_path)
          : t.uiCodex.temporarySession,
        date: formatSessionDate(session.updated_at || session.created_at, language),
        updatedAt: session.updated_at || session.created_at || '',
        workspacePath: session.workspace_path || '',
        workspaceKind: session.workspace_kind || '',
        pinned: !!session.pinned,
        pinnedAt: session.pinned_at || '',
        working: !!codexBusyBySession[session.id],
        waitingInput: !!codexWaitingInputBySession[session.id],
        taskKind: 'codex',
        leadingIcon: <AcpAgentLogo agentId={session.agent_id} className="h-[18px] w-[18px]" title={session.agent_name || t.acpAgent} />,
        testId: 'codex-sidebar-item',
        menuTestId: 'codex-sidebar-menu',
        codexSession: session,
      })), [codexSessions, codexBusyBySession, codexWaitingInputBySession, t, language]);
      const pinnedChatHistory = useMemo(() => chatHistory
        .filter(chat => chat.pinned)
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt))), [chatHistory]);
      const bridgeScheduledTaskRecentRuns = bs && bs.scheduledTaskRecentRuns;
      // bridge.available is deliberately not a dependency: the flag is assigned
      // once when the bridge script installs window.TauriBridge and never
      // reassigned, so the preview branch below cannot go stale afterwards.
      const scheduledRunShortcuts = useMemo(() => (bridgeScheduledTaskRecentRuns && bridgeScheduledTaskRecentRuns.length)
        ? bridgeScheduledTaskRecentRuns
        : (bridge.available ? [] : PREVIEW_SCHEDULED_RUN_SHORTCUTS.map(run => ({ ...run, taskName: t[run.taskNameKey] || run.taskNameKey }))), [bridgeScheduledTaskRecentRuns, t]);
      const scheduledRunSessionIds = useMemo(() => new Set(
        scheduledRunShortcuts
          .map(run => run && run.sessionId)
          .filter(Boolean)
      ), [scheduledRunShortcuts]);
      const scheduledRunBySessionId = useMemo(() => {
        const byId = Object.create(null);
        scheduledRunShortcuts.forEach(run => {
          if (run && run.sessionId) byId[run.sessionId] = run;
        });
        return byId;
      }, [scheduledRunShortcuts]);
      const regularHistory = useMemo(() => chatHistory
        .filter(chat => !chat.pinned && !scheduledRunSessionIds.has(chat.id))
        .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt))), [chatHistory, scheduledRunSessionIds]);
      const scheduledRunItems = useMemo(() => scheduledRunShortcuts
        .filter(run => run && run.sessionId)
        .map(run => {
          // 定时运行会话不进 bs.sessions(list_sessions 隔离 sched-*),标题/置顶
          // 状态由后端 run DTO 直接携带。
          const rawTitle = run.sessionTitle || '';
          const title = (!rawTitle || isDefaultChatTitle(rawTitle))
            ? (run.taskName || t.scheduledPlans)
            : rawTitle;
          return {
            id: run.sessionId,
            title,
            updatedAt: run.createdAt || run.scheduledFor || '',
            pinned: !!run.pinned,
            pinnedAt: run.pinnedAt || '',
            working: run.status === 'running' || run.status === 'queued',
            subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
            date: '',
            leadingIcon: scheduledRunIcon(run, activeTheme),
            testId: 'scheduled-run-sidebar-item',
            menuTestId: 'scheduled-run-sidebar-menu',
            scheduledRun: run,
          };
        }), [scheduledRunShortcuts, scheduledRunLabel, t, language, activeTheme]);
      const scheduledRunHistory = useMemo(() => scheduledRunItems.filter(chat => !chat.pinned), [scheduledRunItems]);
      const pinnedHistory = useMemo(() => [...pinnedChatHistory, ...scheduledRunItems.filter(chat => chat.pinned)]
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt))), [pinnedChatHistory, scheduledRunItems]);

      const decorateScheduledRunChat = useCallback((chat, run) => {
        if (!run) return chat;
        const title = (!chat.title || isDefaultChatTitle(chat.title))
          ? (run.taskName || t.scheduledPlans)
          : chat.title;
        return Object.assign({}, chat, {
          title,
          subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
          leadingIcon: scheduledRunIcon(run, activeTheme),
          testId: 'scheduled-run-sidebar-item',
          menuTestId: 'scheduled-run-sidebar-menu',
          scheduledRun: run,
        });
      }, [t, language, activeTheme, scheduledRunLabel]);

      const [justInstalledTool, setJustInstalledTool] = useState(null);
      const [taskListFilter, setTaskListFilter] = useState('all');
      const [taskListSort, setTaskListSort] = useState('pinned_first');
      const [taskFilterOpen, setTaskFilterOpen] = useState(false);
      const taskFilterRef = useRef(null);
      // 日期组展开状态:未点过的组按默认值走(今天展开、以往折叠),点过后记住用户选择
      const [dateGroupOpen, setDateGroupOpen] = useState({});
      // Code-style sidebar: enabled by default in code mode (folder grouping +
      // collapsed primary nav); the 全部/代码 pill switches style explicitly.
      // null means the user never picked: standard list outside code mode, code
      // style inside code mode (the long-standing default). Once picked, the
      // choice is persisted and applies in every mode.
      const [sidebarCodeStyle, setSidebarCodeStyle] = useState(() => {
        try {
          const stored = localStorage.getItem('pinvou_sidebar_code_style');
          return stored === 'normal' || stored === 'code' ? stored : null;
        } catch {
          return null;
        }
      });
      // The 全部/代码 pill drives both state and the persisted choice in one place.
      const setSidebarCodeStylePersisted = useCallback((next) => {
        setSidebarCodeStyle(next);
        try {
          localStorage.setItem('pinvou_sidebar_code_style', next);
        } catch {
          // When the WebView disables storage, still allow switching for this window.
        }
      }, []);
      // Folder group expand state: all expanded by default; once toggled, remember the choice
      const [folderGroupOpen, setFolderGroupOpen] = useState({});
      // Primary-nav collapse is a global manual toggle, shared by every mode: nothing
      // auto-collapses it and no mode switch resets it; the choice persists per window
      // like the 全部/代码 style pill above.
      const [sidebarNavCollapsed, setSidebarNavCollapsed] = useState(() => {
        try {
          return localStorage.getItem('pinvou_sidebar_nav_collapsed') === '1';
        } catch {
          return false;
        }
      });
      const setSidebarNavCollapsedPersisted = useCallback((next) => {
        setSidebarNavCollapsed(next);
        try {
          localStorage.setItem('pinvou_sidebar_nav_collapsed', next ? '1' : '0');
        } catch {
          // When the WebView disables storage, still allow toggling for this window.
        }
      }, []);
      // Code mode is a mode, not a page: after entering, navigating to output/monitor
      // pages keeps code mode — the sidebar stays code-styled and New chat still creates
      // code sessions; only explicitly switching back to work, or opening a normal
      // chat session, exits it.
      const [codeModeOn, setCodeModeOn] = useState(false);
      // 任务列表的展示形态由 全部/代码 胶囊决定;未显式选择(null)时普通模式
      // 默认「全部」标准列表、code 模式默认 code 样式(沿用既有默认)。
      const sidebarCodeListActive = sidebarCodeStyle === null ? codeModeOn : sidebarCodeStyle === 'code';
      // code 形态下「代码会话」筛选等同「全部」、「定时任务」恒为空(菜单已隐藏这两项);
      // 进入 code 形态时若仍挂着这两个筛选,复位为「全部」,避免列表莫名变空。
      // 用 layout effect 在首帧绘制前完成复位,避免闪现一帧空的「暂无任务」列表。
      useLayoutEffect(() => {
        if (sidebarCodeListActive && (taskListFilter === 'code' || taskListFilter === 'scheduled')) {
          setTaskListFilter('all');
        }
      }, [sidebarCodeListActive, taskListFilter]);
      const [archiveConfirm, setArchiveConfirm] = useState(null);
      const [archiveToast, setArchiveToast] = useState(false);
      const [settingsToast, setSettingsToast] = useState('');
      const [projectOpsBusy, setProjectOpsBusy] = useState(false);
      const [moveToProjectSession, setMoveToProjectSession] = useState(null);
      const [moveToPresetProject, setMoveToPresetProject] = useState(null);
      // 拖拽高亮的唯一所有者:源行 dragend 无条件清除,webview 丢 dragleave
      // 事件时高亮也不会卡死(评审 #450 finding 5)。
      const [dropTargetGroupKey, setDropTargetGroupKey] = useState(null);
      // 稳定入口:RecentItem 的 memo 依赖 prop 引用稳定(NavigationComponents
      // 内注释),内联箭头会让每个 App 重渲染(每个流式 token 批次)重渲染
      // 全部 codex 侧栏行;identity 只在门控布尔翻转(项目从无到有/反之)时
      // 变化。RecentItem 自己传 chat,无需逐行捕获。
      // movePickerRestoreRef:移动成功的 regroup 会把出发行重新挂到新的分组
      // 容器下,原标签节点随之销毁,被动还原会因 isConnected 失败跳过——
      // 成功时按会话键解析新节点,交给 useDialogFocusRestore 的关闭时还原。
      // menu 打开时一并清陈旧拖拽预置,避免上一次落点残留到本次选择
      // (finding 7;setState 引用稳定,不影响本回调的 identity)。
      const movePickerRestoreRef = useRef(null);
      const openMovePicker = useCallback((target) => {
        movePickerRestoreRef.current = null;
        setMoveToPresetProject(null);
        setMoveToProjectSession(target);
      }, []);
      // 拖拽高亮清除必须引用稳定:行内箭头让每个 App 重渲染(每个流式
      // token 批次、tear-off 期间每次 pointermove 的 setDragAvatar)都新建
      // 引用,击穿 RecentItem 的 memo,重渲染全部侧栏行。
      const clearDropTarget = useCallback(() => setDropTargetGroupKey(null), []);
      const [rebindDraft, setRebindDraft] = useState(null);
      // 桥完成首次状态同步(bs 就绪)后拉一次项目快照;后续变更由
      // projects:list_changed 事件驱动桥内刷新(bridge/projects.js)。
      const projectsBootstrapReady = !!bs;
      useEffect(() => {
        if (projectsBootstrapReady && bridge.projects) bridge.projects.loadProjects();
      }, [projectsBootstrapReady]);
      // Set of session ids whose archive export is in flight: the handler
      // exits early to prevent concurrent duplicate exports, and the sidebar
      // hides the matching menu item as in-progress feedback.
      const [exportingSessionIds, setExportingSessionIds] = useState(() => new Set());
      // latest-ref mirror: the stable export callback reads the in-flight set
      // here instead of changing identity whenever the set changes.
      const exportingSessionIdsRef = useRef(exportingSessionIds);
      exportingSessionIdsRef.current = exportingSessionIds;

      // Expanded sidebar width: drag the right edge to adjust (220~480px), double-click
      // the handle to reset to default; the choice is persisted.
      const SIDEBAR_WIDTH_DEFAULT = 280;
      const SIDEBAR_WIDTH_MIN = 220;
      const SIDEBAR_WIDTH_MAX = 480;
      const clampSidebarWidth = (w) => Math.min(SIDEBAR_WIDTH_MAX, Math.max(SIDEBAR_WIDTH_MIN, w));
      const [sidebarWidth, setSidebarWidth] = useState(() => {
        try {
          const saved = Number(localStorage.getItem('pinvou_sidebar_width'));
          return Number.isFinite(saved) && saved > 0 ? clampSidebarWidth(saved) : SIDEBAR_WIDTH_DEFAULT;
        } catch {
          return SIDEBAR_WIDTH_DEFAULT;
        }
      });
      // Disable the width transition while dragging to avoid follow lag; the ref lets
      // pointerup read the latest width for persistence.
      const [sidebarResizing, setSidebarResizing] = useState(false);
      const sidebarWidthRef = useRef(sidebarWidth);
      const applySidebarWidth = (w) => {
        sidebarWidthRef.current = w;
        setSidebarWidth(w);
      };
      const beginSidebarResize = useCallback((event) => {
        event.preventDefault();
        const handle = event.currentTarget;
        const startX = event.clientX;
        const startWidth = sidebarWidthRef.current;
        setSidebarResizing(true);
        const onMove = (moveEvent) => {
          applySidebarWidth(clampSidebarWidth(startWidth + moveEvent.clientX - startX));
        };
        const onUp = () => {
          setSidebarResizing(false);
          window.removeEventListener('pointermove', onMove);
          window.removeEventListener('pointerup', onUp);
          window.removeEventListener('pointercancel', onUp);
          try {
            localStorage.setItem('pinvou_sidebar_width', String(sidebarWidthRef.current));
          } catch {
            // When the WebView disables storage, the width applies only this once.
          }
        };
        // Capture the pointer and listen for pointercancel so an interrupted drag
        // (window blur / touch taken over by a system gesture) still settles; otherwise
        // resizing sticks at true (transition permanently disabled) and listeners leak.
        try {
          handle.setPointerCapture(event.pointerId);
        } catch {
          // Fall back to window listeners when an old WebView does not support capture.
        }
        window.addEventListener('pointermove', onMove);
        window.addEventListener('pointerup', onUp);
        window.addEventListener('pointercancel', onUp);
      }, []);
      const resetSidebarWidth = useCallback(() => {
        applySidebarWidth(SIDEBAR_WIDTH_DEFAULT);
        try {
          localStorage.setItem('pinvou_sidebar_width', String(SIDEBAR_WIDTH_DEFAULT));
        } catch {
          // Same as above.
        }
      }, []);
      // Keyboard resizing per the WAI-ARIA Window Splitter pattern: arrows move by a
      // step (Shift widens it), Home/End jump to the bounds. Each adjustment goes through
      // the same clamping and persistence as the pointer flow.
      const SIDEBAR_RESIZE_KEY_STEP = 24;
      const keyboardSidebarResize = useCallback((event) => {
        const step = SIDEBAR_RESIZE_KEY_STEP * (event.shiftKey ? 4 : 1);
        let next;
        if (event.key === 'ArrowLeft') next = sidebarWidthRef.current - step;
        else if (event.key === 'ArrowRight') next = sidebarWidthRef.current + step;
        else if (event.key === 'Home') next = SIDEBAR_WIDTH_MIN;
        else if (event.key === 'End') next = SIDEBAR_WIDTH_MAX;
        else return;
        event.preventDefault();
        applySidebarWidth(clampSidebarWidth(next));
        try {
          localStorage.setItem('pinvou_sidebar_width', String(sidebarWidthRef.current));
        } catch {
          // Same as pointer resize: applies to this session only when storage is unavailable.
        }
      }, []);

      useEffect(() => {
        if (!taskFilterOpen) return;
        const closeOnPointerDown = (event) => {
          if (taskFilterRef.current && !taskFilterRef.current.contains(event.target)) {
            setTaskFilterOpen(false);
          }
        };
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            setTaskFilterOpen(false);
          }
        };
        document.addEventListener('pointerdown', closeOnPointerDown);
        window.addEventListener('keydown', closeOnEscape);
        return () => {
          document.removeEventListener('pointerdown', closeOnPointerDown);
          window.removeEventListener('keydown', closeOnEscape);
        };
      }, [taskFilterOpen]);

      const sidebarTaskFilterOptions = [
        { id: 'all', label: t.sidebarTaskFilterAll },
        { id: 'pinned', label: t.sidebarTaskFilterPinned },
        // In the project form (capsule set to "Projects") the list is always
        // code/bound sessions: the "Code sessions" filter is equivalent to
        // 「全部」、「定时任务」恒为空——两个选项都是死胡同,只在标准形态提供。
        ...(sidebarCodeListActive ? [] : [
          { id: 'code', label: t.sidebarTaskFilterCodeSessions },
          { id: 'scheduled', label: t.sidebarTaskFilterScheduled },
        ]),
      ];
      const sidebarTaskSortOptions = [
        { id: 'pinned_first', label: t.sidebarTaskSortPinnedFirst },
        { id: 'recent', label: t.sidebarTaskSortRecent },
      ];
      const allSidebarTasks = useMemo(() => [
        ...pinnedHistory.map((chat) => {
          const run = chat.scheduledRun || scheduledRunBySessionId[chat.id];
          const item = decorateScheduledRunChat(chat, run);
          return { ...item, taskKind: run ? 'scheduled' : 'regular' };
        }),
        ...regularHistory.map(chat => ({ ...chat, taskKind: 'regular' })),
        ...scheduledRunHistory.map(chat => ({ ...chat, taskKind: 'scheduled' })),
        ...codexHistory,
      ], [pinnedHistory, regularHistory, scheduledRunHistory, scheduledRunBySessionId, codexHistory, decorateScheduledRunChat]);
      // latest-ref mirror: handleArchiveSession (a stable useCallback) reads
      // the latest task list for the session title at call time, instead of
      // changing the callback identity per render just to read a value.
      const allSidebarTasksRef = useRef(allSidebarTasks);
      allSidebarTasksRef.current = allSidebarTasks;
      const sidebarTaskHistory = useMemo(() => (
        filterSessionsByTab(allSidebarTasks, taskListFilter).sort(sessionListComparator(taskListSort))
      ), [allSidebarTasks, taskListFilter, taskListSort]);

      // 任务列表按日期堆叠:今天默认展开、以往默认折叠;组内顺序沿用上面的筛选+排序结果,
      // 组间按日期倒序,无时间戳的落 'unknown' 组沉底。
      // 「置顶优先」排序下置顶项提升到所有日期组之上,否则旧会话会埋进默认折叠的以往分组,
      // 只剩置顶标志、没有置顶效果。
      const todayDateKey = localDateKey(Date.now());
      const sidebarPinnedHoisted = useMemo(() => (taskListSort === 'pinned_first'
        ? sidebarTaskHistory.filter(chat => !!chat.pinned)
        : []), [taskListSort, sidebarTaskHistory]);
      const sidebarTaskGroups = useMemo(() => groupSessionsByLocalDate(
        sidebarTaskHistory.filter(chat => !(sidebarPinnedHoisted.length && chat.pinned)),
        (chat) => localDateKey(chat.updatedAt || chat.pinnedAt),
      ), [sidebarTaskHistory, sidebarPinnedHoisted]);

      // Project view (formerly the "Code" form): every session bound to a
      // real directory — code/ACP sessions and #445 bound work sessions — is
      // grouped uniformly by the project layer's three tiers; unbound plain
      // sessions stay in the date view of "All". Grouping follows binding,
      // the same signal as the safety posture.
      // Note (aligned with the memo comment above): bridge snapshots are
      // persistent projections, so these memos skip recomputation when their
      // slices keep their references; tier-2 grouping remains
      // O(sessions × projects × roots) (#448 finding 8).
      const sidebarCodeTasks = useMemo(() => (sidebarCodeListActive
        ? sidebarTaskHistory.filter(chat => chat.taskKind === 'codex'
            // The bound-work-session branch is desktop-only, like the projects
            // slice it feeds: on web the backend degrades workspace_binding to
            // its last path component, so the value is a leaf name with no
            // project behind it and two same-named directories would collapse
            // into one tier-3 bucket (review #464 round-6 finding 8b).
            || (can('desktopChrome') && chat.taskKind === 'regular' && chat.workspacePath))
        : []), [sidebarCodeListActive, sidebarTaskHistory]);
      const sidebarFolderPinned = useMemo(() => (taskListSort === 'pinned_first'
        ? sidebarCodeTasks.filter(chat => !!chat.pinned)
        : []), [taskListSort, sidebarCodeTasks]);
      const sidebarUnpinnedCodeTasks = useMemo(() => sidebarCodeTasks.filter(chat => !(sidebarFolderPinned.length && chat.pinned)), [sidebarCodeTasks, sidebarFolderPinned]);
      const sidebarProjectsData = bs && bs.projectsList;
      // Sidebar group-header props per group kind (extracted so the render map
      // stays readable): the project channel (§9.9) and manage-panel (§4)
      // entries are desktop-only, gated by the projects bridge domain.
      // Single source for the ProjectGroupHeader domain props (review #484
      // MINOR: the mount site used to repeat onRename/onDelete/onRebind
      // before this spread, and the spread silently shadowed the gated
      // spellings with un-gated ones). Every bridge-backed entry
      // is gated here: the projects domain is desktop-only (§9.8), and a
      // web group must render no dead entries.
      const unavailableRootsOf = (group) => (group.roots || [])
        // Missing availability data (older host, stub) counts as available —
        // same default as manageFolderRows (review #484 round-6).
        .filter(root => !!(root && typeof root === 'object' ? root.available !== false : root))
        .map(root => String(typeof root === 'object' ? root.path : root));
      const projectGroupHeaderProps = (group) => ({
        // Tag-only projects (no roots left) have no directory to bind: the
        // "new session" entry must not render for them — the handler would
        // silently no-op on a missing primary root (review #484 M3).
        onNewSession: bridge.projects && pickerProjectRoots(group).length > 0 ? () => handleProjectNewSession(group.projectId) : undefined,
        onManage: bridge.projects ? () => setManageFoldersId(group.projectId) : undefined,
        onRename: bridge.projects ? (name) => handleRenameProject(group.projectId, name) : undefined,
        onDelete: bridge.projects ? () => handleDeleteProject(group.projectId) : undefined,
        unavailableRoots: unavailableRootsOf(group),
        onRebind: bridge.projects ? (rootPath) => startRebindWorkspace(rootPath) : undefined,
        onDropSession: bridge.projects ? (sessionId) => handleDropSessionOnProject(sessionId, group.projectId) : undefined,
      });
      const folderGroupHeaderProps = (group) => ({
        title: group.path,
        // bridge.projects exists on desktop only: no dead entries on web.
        onConvert: bridge.projects ? (name) => handleConvertFolderToProject(group.path, name) : undefined,
      });
      const sidebarGroupHeaderProps = (group) => {
        if (group.kind === 'project') return projectGroupHeaderProps(group);
        if (group.kind === 'folder') return folderGroupHeaderProps(group);
        return {};
      };
      // Picker inputs: the projects snapshot plus every directory-bound item
      // (code/ACP project sessions and #445 bound work sessions).
      const projectsListEntries = sidebarProjectsData && sidebarProjectsData.projects;
      const projectsListAssignments = sidebarProjectsData && sidebarProjectsData.assignments;
      // Hot-view inputs come from the UNFILTERED task pool: deriving from
      // sidebarCodeTasks (the sidebar-filtered slice) made a warm folder go
      // cold — or vanish from the picker — whenever the sidebar sat in
      // another filter/style, which has nothing to do with recency
      // (review #484 round-3 minor).
      const boundWorkspaceItems = useMemo(() => [
        ...allSidebarTasks
          .filter(s => s.taskKind === 'codex'
            || (can('desktopChrome') && s.taskKind === 'regular' && s.workspacePath))
          .map(s => ({ id: s.id, workspaceKind: s.workspaceKind, workspacePath: s.workspacePath, updatedAt: s.updatedAt || '' })),
        ...codexSessions
          .filter(s => s && s.workspace_kind === 'project' && s.workspace_path)
          .map(s => ({ id: s.id, workspaceKind: 'project', workspacePath: String(s.workspace_path), updatedAt: s.updated_at || '' })),
      ], [allSidebarTasks, codexSessions]);
      // Dedicated channel for the project row's "new session" (§9.9 project
      // channel): no picker detour — cwd = the project's remembered primary
      // root (written by the picker / manage panel), keychain = all of the
      // project's roots at that moment. The lane follows the current page
      // (code page → codex draft, anything else → chat draft).
      const handleProjectNewSession = async (projectId) => {
        const project = ((sidebarProjectsData && sidebarProjectsData.projects) || [])
          .find(entry => entry.id === projectId);
        if (!project) return;
        const primary = pickerPrimaryRoot(project);
        if (!primary) return; // tag-only project: no root to bind; the render side already keeps the button away
        const roots = pickerProjectRoots(project);
        // Grant-notice parity: this channel grants the project's whole root set
        // without a picker detour, so the mode-aware notice must still surface
        // at grant time (§9.4) — but only when the draft actually staged.
        if (await applyWorkspaceTarget({
          lane: currentView === 'codex' ? 'codex' : 'chat',
          path: primary,
          projectId: project.id,
          roots,
        })) {
          setSettingsToast(workspaceGrantNotice(activeLaneMode(), roots.length));
        }
      };

      // ── Manage-folders panel (§4) ───────────────────────────────────────
      // View/add/remove project roots, primary-root memory, rename, and the
      // exclusion list view/revoke. Removing a root writes explicit move-outs
      // for its auto members inside the backend's update_project; after
      // add/remove the bridge reloads projects on its own (event + active
      // refetch, belt and braces).
      const [manageFoldersId, setManageFoldersId] = useState(null);
      const manageFoldersProject = manageFoldersId
        ? (((sidebarProjectsData && sidebarProjectsData.projects) || [])
            .find(entry => entry.id === manageFoldersId) || null)
        : null;
      const closeManageFolders = () => setManageFoldersId(null);
      // Add folder: system folder picker → whole-set roots replacement (the
      // grant notice sits on the panel's "add" entry, the same weight as the
      // picker, §2). Paths already in the project are skipped with a notice.
      const handleManageAddFolder = async () => {
        if (!bridge.files || !bridge.files.pickFolders || !manageFoldersProject || projectOpsBusy) return;
        const picked = await bridge.files.pickFolders()
          .catch((error) => { console.warn('pick project folder failed', error); return null; });
        if (!picked) return;
        const folder = Array.isArray(picked) ? picked[0] : picked;
        if (!folder) return;
        if (rootAlreadyPresent(manageFoldersProject, folder)) {
          setSettingsToast(t.uiManageFolders.addDuplicate);
          return;
        }
        if (rootConflictsWithExisting(manageFoldersProject, folder)) {
          setSettingsToast(t.uiManageFolders.addNested);
          return;
        }
        setProjectOpsBusy(true);
        try {
          await bridge.projects.updateProjectRoots(
            manageFoldersProject.id,
            [...manageFoldersProject.roots.map(root => rootPathOf(root)), folder],
          );
        } catch (error) {
          console.warn('add project folder failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        } finally {
          setProjectOpsBusy(false);
        }
      };
      const handleManageRemoveRoot = async (root) => {
        if (!manageFoldersProject || projectOpsBusy) return;
        const plan = removeRootPlan(manageFoldersProject, root);
        if (!plan.removed || plan.needsNewPrimary) return; // primary-root removal is blocked inside the panel
        setProjectOpsBusy(true);
        try {
          await bridge.projects.updateProjectRoots(manageFoldersProject.id, plan.roots);
        } catch (error) {
          console.warn('remove project folder failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        } finally {
          setProjectOpsBusy(false);
        }
      };
      const handleManageSetPrimary = async (root) => {
        if (!manageFoldersProject || projectOpsBusy) return;
        setProjectOpsBusy(true);
        try {
          await bridge.projects.setPrimaryRoot(manageFoldersProject.id, root);
        } catch (error) {
          console.warn('set primary root failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        } finally {
          setProjectOpsBusy(false);
        }
      };
      // Exclusion list (§3): listed = no more auto-materialization
      // (reversible; projects/sessions untouched); unlisted = revoked.
      const handleSetNeverMaterialize = async (root, never) => {
        if (projectOpsBusy) return;
        setProjectOpsBusy(true);
        try {
          await bridge.projects.setNeverMaterialize(root, never);
          setSettingsToast(t.uiManageFolders.exclusionDone);
        } catch (error) {
          console.warn('set never-materialize failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        } finally {
          setProjectOpsBusy(false);
        }
      };

      // ── Unified "choose workspace" picker (single entry, §2/§3/§9.3/§9.4) ──
      // The picker is mounted by the host below and is pure display: every
      // consequential action (draft staging, ensure/materialization, codex
      // workspacePickerRequest) is handled here. The dialog only ever reads
      // hot-view rows + callbacks and never touches the backend.
      const [workspacePicker, setWorkspacePicker] = useState(null); // { lane, mode }
      const [pickerBusy, setPickerBusy] = useState(false);
      const [pickerExcluded, setPickerExcluded] = useState(null);
      const [pickerCodexRequest, setPickerCodexRequest] = useState(null);
      // The codex lane's effective mode is reported up by CodexAcpView so
      // sidebar surfaces opened while the code page is active show the codex
      // lane's mode-aware copy rather than the chat lane's.
      const [codexLaneMode, setCodexLaneMode] = useState(null);
      const workspacePickerRows = useMemo(() => computePickerRows({
        projects: projectsListEntries || [],
        items: boundWorkspaceItems,
        assignments: projectsListAssignments || {},
      }), [projectsListEntries, projectsListAssignments, boundWorkspaceItems]);
      const closeWorkspacePicker = () => { setWorkspacePicker(null); setPickerExcluded(null); };
      // Returns whether the draft actually staged (the codex lane's request
      // object always lands); callers surface grant notices only on a real
      // staging. Async: the chat lane may need to enter a fresh draft first.
      const applyWorkspaceTarget = async ({ lane, path, projectId, roots }) => {
        // Default false: a missing bridge surface means nothing was staged, so
        // callers must not claim the grant happened (review #484 n5).
        let applied = false;
        if (lane === 'codex') {
          setPickerCodexRequest({ epoch: Date.now(), path: path || null, projectId: projectId || null, roots: roots || [] });
          applied = true;
        } else if (bridge.sessions && bridge.sessions.setDraftWorkspace) {
          // The bridge stages a draft workspace only in draft state — with an
          // active session the call is a documented silent no-op, so the
          // project row "+" used to dead-end without any feedback on the most
          // common path (review #484 round-5 M2). Enter a fresh chat draft
          // first (the same path as handleNewChat), then stage; the
          // navigation makes the new draft visible instead of silently
          // discarding the click.
          if (bridge.activeSessionId && bridge.sessions.createNewSession) {
            setCodeModeOn(false);
            await bridge.sessions.createNewSession();
            setActiveChat(null);
            setCurrentView('chat');
          }
          applied = bridge.sessions.setDraftWorkspace(path || null, { projectId: projectId || null, workspaceRoots: roots || [] }) !== false;
        }
        closeWorkspacePicker();
        return applied;
      };
      const activeLaneMode = () => (currentView === 'codex'
        ? codexLaneMode
        : ((bs && bs.modeState && bs.modeState.mode) || null));
      // Same-weight grant notice for every entry that grants folder access
      // without the picker's expansion panel (§9.4).
      const workspaceGrantNotice = (mode, count) => (workspaceNoticeTone(mode) === 'restricted'
        ? t.uiWorkspacePicker.noticeRestricted(count)
        : t.uiWorkspacePicker.noticeVisibility(count));
      const pickerLane = () => (workspacePicker ? workspacePicker.lane : 'chat');
      // Project channel (§9.3): cwd = the picked root, keychain = the project's
      // full root set at that moment; the backend writes last_primary_root
      // inside create_session.
      const handlePickerSelectProject = (project, root) => {
        applyWorkspaceTarget({ lane: pickerLane(), path: root, projectId: project.id, roots: pickerProjectRoots(project) });
      };
      const handlePickerTemporary = () => {
        applyWorkspaceTarget({ lane: pickerLane(), path: null, projectId: null, roots: [] });
      };
      // Browse channel (§9.9 folder channel): system folder picker → ensure
      // (anchor reuse / materialize; the exclusion list skips) → start at the
      // picked folder with cwd = F, roots = [F], projectId = the anchored
      // project's id. The assignment is required: without it, tier-2 nested
      // grouping adopts the session into a broader project whose root covers
      // F (e.g. a Desktop-rooted project), but the user picked F itself —
      // the decision is "group only on an exact primary-root match, else
      // materialize a new project and belong to it".
      const handlePickerBrowse = async () => {
        if (!bridge.files || !bridge.files.pickFolders || pickerBusy) return;
        // Capture the lane synchronously: the system folder dialog can outlive
        // the picker (closed meanwhile), and reading pickerLane() after the
        // await would mistarget the chat lane (review #484 m1).
        const lane = pickerLane();
        const picked = await bridge.files.pickFolders()
          .catch((error) => { console.warn('pick workspace folder failed', error); return null; });
        if (!picked) return;
        const folder = Array.isArray(picked) ? picked[0] : picked;
        if (!folder) return;
        let materialized = false;
        // Only an ensure that actually ran may interpret "no outcome" as the
        // exclusion list; a missing bridge entry proves nothing about the
        // folder (review #484 round-6: the panel's copy asserts the list, so
        // it must not render on an unverified claim). An IPC-level rejection
        // is neither an outcome nor an exclusion either — the folder's
        // exclusion status stays unverified.
        let ensured = false;
        let errored = false;
        let ensuredProjectId = null;
        if (bridge.projects && bridge.projects.ensureFolderProjects) {
          ensured = true;
          setPickerBusy(true);
          try {
            const outcomes = await bridge.projects.ensureFolderProjects([folder]);
            const list = Array.isArray(outcomes) ? outcomes : [];
            materialized = list.some(o => o && (o.status === 'created' || o.status === 'covered'));
            // The picked folder now owns an anchored project (created or
            // reused): the conversation must be assigned to THAT project, or
            // tier-2 nested grouping adopts it into a broader project whose
            // root covers the folder (e.g. a Desktop-rooted project) — the
            // user picked the subfolder, not its ancestor (review decision:
            // group only on an exact primary-root match, else materialize).
            const hit = list.find(o => o && (o.status === 'created' || o.status === 'covered'));
            ensuredProjectId = hit
              ? (hit.status === 'created' ? (hit.project && hit.project.id) : hit.project_id) || null
              : null;
            // A failed root is NOT the exclusion list: it gets the failure
            // toast and stops here — the excluded-folder panel is only for
            // the "skipped without an outcome" case (review #484 MINOR).
            if (!materialized && list.some(o => o && o.status === 'failed')) {
              setSettingsToast(t.uiProjects.opFailed);
              return;
            }
          } catch (error) {
            // Log and start the plain folder conversation — browse promises a
            // conversation at the picked folder, and the failed-outcome branch
            // above is the stop surface for a real backend refusal.
            console.warn('ensure folder project failed', error);
            errored = true;
          } finally {
            setPickerBusy(false);
          }
        }
        if (!materialized && ensured && !errored) {
          // Exclusion list (§3): the backend skipped it → no outcome; the
          // picker says so honestly and still allows starting as a plain
          // folder (no projectId).
          setPickerExcluded(folder);
          return;
        }
        applyWorkspaceTarget({ lane, path: folder, projectId: ensuredProjectId, roots: [folder] });
      };

      const sidebarFolderGroups = useMemo(() => (sidebarCodeListActive
        ? groupSessionsWithProjects(
            sidebarUnpinnedCodeTasks,
            sidebarProjectsData ? sidebarProjectsData.projects : [],
            sidebarProjectsData ? sidebarProjectsData.assignments : {},
          )
        : []), [sidebarCodeListActive, sidebarUnpinnedCodeTasks, sidebarProjectsData]);
      // 置顶提升会把成员从组 rows 里摘走,但组头计数(含删除确认)要按提升前
      // 的全量成员算,否则成员全置顶的组确认删除时显示 (0)(评审 finding 24)。
      // 置顶项通常很少,单独对它们跑一遍分组拿到每组被摘走的数量即可。
      const sidebarGroupPinnedCounts = useMemo(() => {
        if (!sidebarCodeListActive || sidebarFolderPinned.length === 0) return {};
        const counts = {};
        groupSessionsWithProjects(
          sidebarFolderPinned,
          sidebarProjectsData ? sidebarProjectsData.projects : [],
          sidebarProjectsData ? sidebarProjectsData.assignments : {},
        ).forEach((group) => { counts[group.key] = group.rows.length; });
        return counts;
      }, [sidebarCodeListActive, sidebarFolderPinned, sidebarProjectsData]);

      // latest-ref mirror: the pet-snapshot broadcast effect only subscribes to bs.sessions/sessionBusy/language,
      // while snapshot contents (id/title/working) are read via refs to reduce effect resubscription.
      petSnapshotRef.current = useMemo(() => chatHistory.map(chat => ({
        id: chat.id,
        title: chat.title,
        working: chat.working,
      })), [chatHistory]);
      const petSessions = bs && bs.sessions;
      const petSessionBusy = bs && bs.sessionBusy;
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return;
        let disposed = false;
        let unlisten = null;
        // 内容指纹:多会话并发时 sessionBusy/sessions 每次 notify 都换新引用,
        // effect 重跑导致快照风暴,桌宠窗口被高频快照淹没。只有会话集合
        // (id/title/working)真实变化才广播,同一内容的重跑直接跳过。
        const fingerprint = () => JSON.stringify(
          (petSnapshotRef.current || []).map(s => [s.id, s.title, !!s.working]),
        );
        const broadcast = (force = false) => {
          if (typeof ev.emit !== 'function') return Promise.resolve();
          const next = fingerprint();
          if (!force && next === petSnapshotFingerprintRef.current) {
            return Promise.resolve(false);
          }
          petSnapshotFingerprintRef.current = next;
          return ev.emit('pet:activity_snapshot', {
            sequence: ++petSnapshotSequenceRef.current,
            sessions: petSnapshotRef.current,
          }).catch(() => {});
        };
        broadcast();
        // 桌宠窗口冷启动/重连时的主动请求必须无条件应答,不能用指纹挡掉。
        ev.listen('pet:request_snapshot', () => broadcast(true)).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        return () => {
          disposed = true;
          if (unlisten) unlisten();
        };
      }, [petSessions, petSessionBusy, language]);

      const closeMobileSidebar = useCallback(() => {
        if (!isWeb || typeof window === 'undefined') return;
        if (window.matchMedia && window.matchMedia('(max-width: 639px)').matches) {
          setIsSidebarOpen(false);
        }
      }, []);

      // Stable useCallback: the sidebar NavItem memo depends on this callback
      // identity. bs is read through a latest-ref — a click sees the most
      // recently rendered snapshot, matching closure-capture semantics.
      const navigateFromScheduledRun = useCallback(async (nextView, beforeNavigate) => {
        const bs = bsRef.current;
        const context = browserSurfaceTransitionContextRef.current;
        const keepsDesktopBrowserVisible = !context.compact && (
          nextView === 'chat'
          || (nextView === 'scheduled' && context.scheduledRunChat)
        );
        return runBrowserUiTransition(async ({ isCurrent }) => {
          if (bs && bs.scheduledRunContext && bridge.available && bridge.scheduled.exitScheduledRunChat) {
            const exited = await bridge.scheduled.exitScheduledRunChat();
            if (!exited || !isCurrent()) return false;
          }
          if (beforeNavigate) beforeNavigate();
          if (nextView === 'chat') setCodeModeOn(false);
          setCurrentView(nextView);
          if (bs && bs.scheduledRunContext) setActiveChat(bridge.activeSessionId || null);
          closeMobileSidebar();
          return true;
        }, {
          channel: 'view',
          hideMode: bs && bs.scheduledRunContext
            ? 'workspace'
            : keepsDesktopBrowserVisible ? 'none' : 'visible',
        });
      }, [closeMobileSidebar, runBrowserUiTransition, setCurrentView]);

      function openSettingsSection(section = 'general') {
        // 记录进入设置前的页面（代码页齿轮等深链入口），关闭设置时原路返回，
        // 而不是一律回工作页。
        if (currentView !== 'settings') settingsReturnViewRef.current = currentView;
        setSettingsInitialSection(section);
        return navigateFromScheduledRun('settings');
      }

      // Stable useCallbacks: the LazySearchView/RecentItem memos depend on
      // these callback identities; rebuilding them per render would defeat
      // the memo (the dependencies are the real semantic dependencies).
      const handleOpenScheduledRunShortcut = useCallback(async (run) => {
        if (!run || !run.sessionId) return;
        // A scheduled-run session is a normal chat: both the fallback and the
        // successful-open branches land on the scheduled view, so each branch
        // exits code mode right before navigating. Clearing once at the entry
        // would also clear it when the open fails, leaving an active code
        // session behind the standard sidebar and New chat creating plain drafts.
        return runBrowserUiTransition(async ({ isCurrent }) => {
          if (!bridge.available || !bridge.scheduled.openScheduledRunChat) {
            setCodeModeOn(false);
            setCurrentView('scheduled');
            closeMobileSidebar();
            return true;
          }
          const task = {
            id: run.automationId,
            name: run.taskName || t.scheduledPlans,
            model: run.taskModel || null,
          };
          const opened = await bridge.scheduled.openScheduledRunChat(run, task);
          if (!opened || !isCurrent()) return false;
          setCodeModeOn(false);
          setActiveChat(run.sessionId);
          setCurrentView('scheduled');
          return true;
        }, {
          channel: 'session',
          hideMode: 'workspace',
          serialize: true,
          sessionTarget: run.sessionId,
        });
      }, [t, closeMobileSidebar, runBrowserUiTransition, setCurrentView]);

      // Stable useCallback: the sidebar "new chat" NavItems memo depends on
      // its identity (wrapped in handleNavNewChat).
      const handleNewChat = useCallback((installedToolId, forceMode) => {
        // 类型守卫:installedToolId 必须是字符串 toolId。侧边栏按钮 onClick={() => handleNewChat()}
        // 本不传参,但若哪天有调用点写成 onClick={handleNewChat},React 会把事件对象当首参塞进来——
        // 那是 truthy 的 SyntheticEvent,会被当成 toolId 置进 welcomeToolId → ToolWelcomeCard 查不到
        // 工具渲染 null → 欢迎语整块空白。守卫挡住这条暗坑。
        const toolIntentId = typeof installedToolId === 'string' && installedToolId
          ? installedToolId
          : null;
        // Follow code mode rather than the current page: in code mode, even on
        // output/monitor tool pages, New chat still creates a code session draft;
        // forceMode serves call sites that must land on a normal chat, such as AI card
        // creation.
        // Calls carrying a tool intent (tool store "new chat with this tool") must also
        // land on a normal chat: the tool welcome card is only consumed by ChatView, so a
        // codex draft would silently drop the intent and leak it into the next session.
        const hasToolIntent = !!toolIntentId;
        const wantCode = forceMode
          ? forceMode === 'code'
          : !hasToolIntent && codeModeOn;
        return runBrowserUiTransition(async ({ isCurrent }) => {
          if (!isCurrent()) return false;
          if (wantCode && codexAcpSupported) {
            setCodeModeOn(true);
            updateActiveCodexSession(null);
            setCodexDraftEpoch(value => value + 1);
            setCurrentView('codex');
          } else {
            // Every landing here is a normal chat (tool intent, forceMode='chat',
            // code mode off, or codex unsupported on this host). createNewSession
            // nulls activeSessionId, so the bridge sync guard cannot heal a stale
            // codeModeOn afterwards — clear it here.
            setCodeModeOn(false);
            if (bridge.available) await bridge.sessions.createNewSession();
            if (!isCurrent()) return false;
            if (toolIntentId) setJustInstalledTool(toolIntentId);
            setActiveChat(null);
            setCurrentView('chat');
          }
          closeMobileSidebar();
          return true;
        }, {
          channel: 'session',
          hideMode: 'workspace',
          serialize: true,
          sessionTarget: null,
        });
      }, [codeModeOn, codexAcpSupported, closeMobileSidebar, runBrowserUiTransition, updateActiveCodexSession, setCurrentView]);

      function handleSwitchHomeMode(mode) {
        if (mode === 'code' && codexAcpSupported) {
          setCodeModeOn(true);
          updateActiveCodexSession(null);
          setCodexDraftEpoch(value => value + 1);
          setCurrentView('codex');
        } else if (mode === 'work') {
          setCodeModeOn(false);
          // Only the draft state (no active session) starts a new session:
          // when switching back from the code page, bridge's activeSessionId
          // is still the original work session; forcing createNewSession
          // would create a new plain session (default Yolo) and clobber the
          // user's chosen Plan — surfacing as "switch back from code to work
          // and the approval mode reverts to Yolo". Keep the original
          // session; ChatView shows its measured mode after mounting.
          const scopeKey = bridge.activeSessionId
            ? createPinvouModeScopeKey(bridge.activeSessionId)
            : undefined;
          savePinvouModeState({ mode: 'work' }, undefined, scopeKey);
          if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();
          // While on the code page the original work session's mode may have
          // changed (the code page has its own chain), so pull a fresh value
          // before switching back — ChatView must not mount with a stale
          // modeState.
          if (bridge.available && bridge.activeSessionId) {
            bridge.interaction.syncModeState().catch(() => {});
          }
          setCurrentView('chat');
        }
        closeMobileSidebar();
      }

      // AI 造卡:新对话 + 加持「卡牌制造专家」+ 一条 iOS 引导卡 → 用户在空输入框描述需求,复用 persona-card 草稿流程入库
      async function startAICard() {
        const created = await handleNewChat(null, 'chat');
        if (!created) return;
        if (!bridge.available) return;
        const card = await bridge.personas.equipPersona('pinvou-card-creator'); // 先加持(落新 session + 加持气泡)
        if (card) bridge.personas.postCardCreatorIntro();                     // 加持成功才追加引导卡(持久化,切会话/重启不丢);失败则放弃后续,避免错投(二审补充)
      }

      const handleSwitchSession = useCallback(async (id) => {
        if (!bridge.available) return;
        return runBrowserUiTransition(async ({ isCurrent }) => {
          // Web RPC can cross a public relay. Close the drawer and enter chat before loading.
          setCodeModeOn(false);
          setCurrentView('chat');
          closeMobileSidebar();
          const switched = await bridge.sessions.switchToSession(id);
          if (!switched || !isCurrent()) return false;
          setActiveChat(id);
          return true;
        }, {
          channel: 'session',
          hideMode: 'workspace',
          serialize: true,
          sessionTarget: id,
        });
      }, [closeMobileSidebar, runBrowserUiTransition, setCurrentView]);

      async function handleSearchSelect(id) {
        await handleSwitchSession(id);
        setSearchOverlayOpen(false);
      }

      // Stable useCallback: passed directly as the RecentItem memo's onSelect
      // (codex branch).
      const handleSwitchCodexSession = useCallback((id) => {
        setCodeModeOn(true);
        updateActiveCodexSession(id);
        setCurrentView('codex');
        closeMobileSidebar();
      }, [updateActiveCodexSession, closeMobileSidebar, setCurrentView]);

      // 用户在主窗口里亲眼看着完成的会话，公仔的活动卡属于冗余提醒——
      // 完成瞬间若该会话正处于前台聊天视图且窗口有焦点，直接标记已读，
      // 卡片自动消失，不需要用户再去点。
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return;
        let disposed = false;
        const unlisteners = [];
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
        ev.listen('chat:done', (event) => {
          if (disposed) return;
          const payload = event.payload || {};
          const sid = payload.session_id || payload.sessionId;
          if (!sid) return;
          if (typeof document.hasFocus === 'function' && !document.hasFocus()) return;
          if (currentViewRef.current !== 'chat') return;
          if (String(activeChatRef.current) !== String(sid)) return;
          emitToPet('pet:session_viewed', {
            session_id: sid,
            completed: true,
          }).catch(() => {});
        }).then((unlisten) => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        return () => {
          disposed = true;
          unlisteners.forEach((fn) => { try { fn(); } catch { /* listener teardown failure is ignorable */ } });
        };
      }, [handleSwitchSession, runBrowserUiTransition, setCurrentView]);

      // 用户从侧栏切进一个已经完成的会话时，也立即收掉对应完成气泡。
      // 运行中的卡不会被 markSessionViewed 删除；等它完成时，上面的
      // chat:done 监听会再次确认当前画面并完成收尾。
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev || currentView !== 'chat' || !activeChat) return;
        if (typeof document.hasFocus === 'function' && !document.hasFocus()) return;
        const emit = emitPetEvent(ev, 'pet:session_viewed', { session_id: activeChat });
        emit.catch(() => {});
      }, [currentView, activeChat]);

      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        const core = isTauriAvailable() ? tauriCommands : null;
        if (!ev || !core) return;
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
        let disposed = false;
        let consuming = false;
        const unlisteners = [];
        const consumePetNavigation = async () => {
          if (disposed || consuming) return;
          consuming = true;
          try {
            const request = await core.invoke('take_pet_navigation');
            if (!request || disposed) return;
            const scheduledRun = request.scheduled_run || request.scheduledRun;
            if (scheduledRun) {
              const automationId = scheduledRun.automationId || scheduledRun.automation_id;
              const runId = scheduledRun.runId || scheduledRun.run_id;
              const sessionId = scheduledRun.sessionId || scheduledRun.session_id;
              const taskName = scheduledRun.taskName || scheduledRun.task_name;
              const endedAt = scheduledRun.endedAt || scheduledRun.ended_at;
              if (!bridge.available || !bridge.scheduled.openScheduledRunChat) {
                emitToPet('pet:scheduled_notice_open_failed', { run_id: runId }).catch(() => {});
                return;
              }
              let opened = false;
              try {
                const published = await runBrowserUiTransition(async ({ isCurrent }) => {
                  opened = await bridge.scheduled.openScheduledRunChat({
                    id: runId,
                    automationId,
                    sessionId,
                    status: 'completed',
                    endedAt,
                    unread: true,
                  }, {
                    id: automationId,
                    name: taskName,
                  });
                  if (!opened || !isCurrent()) return false;
                  setCodeModeOn(false);
                  setActiveChat(sessionId);
                  setCurrentView('scheduled');
                  return true;
                }, {
                  channel: 'session',
                  hideMode: 'workspace',
                  serialize: true,
                  sessionTarget: sessionId,
                });
                if (!published) opened = false;
              } catch (error) {
                console.error('[pet scheduled navigation] open failed', error);
              }
              if (!opened) {
                emitToPet('pet:scheduled_notice_open_failed', { run_id: runId }).catch(() => {});
                return;
              }
              emitToPet('pet:scheduled_notice_opened', { run_id: runId }).catch(() => {});
              return;
            }
            const sid = request.session_id || request.sessionId;
            if (!sid) {
              setCodeModeOn(false);
              setCurrentView('chat');
              setPetFocusComposerTick(value => value + 1);
              return;
            }
            if (!bridge.available) return;
            const sessionExists = petSnapshotRef.current.some((session) => String(session.id) === String(sid));
            if (!sessionExists) {
              emitToPet('pet:session_unavailable', { session_id: sid }).catch(() => {});
              setCodeModeOn(false);
              setCurrentView('chat');
              setPetFocusComposerTick(value => value + 1);
              return;
            }
            const switched = await handleSwitchSession(sid);
            if (!switched) {
              emitToPet('pet:session_unavailable', { session_id: sid }).catch(() => {});
              return;
            }
            setPetFocusComposerTick(value => value + 1);
            emitToPet('pet:session_viewed', { session_id: sid }).catch(() => {});
          } catch (error) {
            console.error('[pet navigation] consume failed', error);
          } finally {
            consuming = false;
          }
        };
        const subscriptions = [ev.listen('pet:navigation_pending', consumePetNavigation)];
        window.addEventListener('focus', consumePetNavigation);
        void consumePetNavigation();
        Promise.all(subscriptions).then((items) => {
          if (disposed) items.forEach(fn => { fn(); });
          else unlisteners.push(...items);
        }).catch(() => {});
        return () => {
          disposed = true;
          window.removeEventListener('focus', consumePetNavigation);
          unlisteners.forEach(fn => { try { fn(); } catch { /* listener teardown failure is ignorable */ } });
        };
      }, [handleSwitchSession, runBrowserUiTransition, setCurrentView]);

      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        const core = isTauriAvailable() ? tauriCommands : null;
        if (!ev || !core || !bridge.available || !bridge.chat.sendMessageToSession) return;
        let disposed = false;
        let consuming = false;
        let rerun = false;
        let unlisten = null;
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
        // The consumption loop registers once on mount; error copy must follow the current UI language, read via latest-ref.
        const petTextRef = petI18nTextRef;
        const petSessionMissingText = () => (petTextRef.current && petTextRef.current.petSessionMissing) || '';
        const petTaskStartFailedText = () => (petTextRef.current && petTextRef.current.petTaskStartFailed) || '';
        const consume = async () => {
          if (disposed) return;
          if (consuming) {
            rerun = true;
            return;
          }
          consuming = true;
          try {
            if (typeof bridge.lifecycle.init === 'function') await bridge.lifecycle.init();
            // disposed is only flipped in this effect's cleanup, immutable inside the loop;
            // continue/return branches after each request decide whether to exit.
            for (;;) {
              if (disposed) break;
              const request = await core.invoke('take_pet_reply');
              if (!request) break;
              const requestId = request.request_id || request.requestId;
              const sid = request.session_id || request.sessionId;
              const text = String(request.text || '').trim();
              const liveSessions = bridge.state
                ? (bridge.state.get('sessions').sessions || [])
                : [];
              const sessionExists = petSnapshotRef.current.some(
                session => String(session.id) === String(sid),
              ) || liveSessions.some(session => String(session.id) === String(sid));
              if (!sessionExists) {
                emitToPet('pet:reply_failed', {
                  request_id: requestId,
                  session_id: sid,
                  error: petSessionMissingText(),
                  unavailable: true,
                }).catch(() => {});
                continue;
              }
              try {
                const result = await bridge.chat.sendMessageToSession(sid, text);
                emitToPet('pet:reply_accepted', {
                  request_id: requestId,
                  session_id: sid,
                }).catch(() => {});
                if (result?.completion) {
                  result.completion.then((outcome) => {
                    if (outcome?.ok) return;
                    return emitToPet('pet:reply_failed', {
                      request_id: requestId,
                      session_id: sid,
                      error: String(outcome?.error?.message || outcome?.error || petTaskStartFailedText()),
                    }).catch(() => {});
                  });
                }
              } catch (error) {
                emitToPet('pet:reply_failed', {
                  request_id: requestId,
                  session_id: sid,
                  error: String(error && error.message ? error.message : error),
                }).catch(() => {});
              }
            }
          } catch (error) {
            console.error('[pet reply] consume failed', error);
          } finally {
            consuming = false;
            if (rerun && !disposed) {
              rerun = false;
              void consume();
            }
          }
        };
        ev.listen('pet:reply_pending', consume).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        void consume();
        // The effect intentionally registers the pet-reply consumption loop once on mount; error copy reads the current language via ref,
        // avoiding repeated listener reattachment on language switches.
        return () => {
          disposed = true;
          if (unlisten) unlisten();
        };
      }, []);

      // The four session-action callbacks below are all stable useCallbacks:
      // the RecentItem/search management memos depend on their identities and
      // their dependency arrays list exactly the state they read.
      const handleDeleteSession = useCallback(async (id) => {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.deleteSession(id);
        if (isCodexSession) {
          if (activeCodexId === id) updateActiveCodexSession(null);
          await refreshCodexSessions().catch(() => {});
        }
      }, [codexSessions, activeCodexId, updateActiveCodexSession, refreshCodexSessions]);

      const handleRenameSession = useCallback(async (id, title) => {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.renameSession(id, title);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }, [codexSessions, refreshCodexSessions]);

      // One-click full session log export (.tar.xz, full-fidelity context).
      // The backend opens the native save dialog: cancellation resolves to
      // null; the success toast carries the save path and failures surface
      // through the settings-page toast. Sessions already exporting exit
      // early (preventing concurrent duplicate exports) and their sidebar
      // menu items are hidden at the same time as in-progress feedback. The
      // default file name carries the session title (truncated by code
      // points so surrogate pairs are not split) and a short id; the final
      // name is sanitized by the backend (against path traversal and invalid
      // characters). The task list is read via allSidebarTasksRef and the
      // in-flight set via exportingSessionIdsRef so the callback identity
      // stays stable (RecentItem is memoized).
      const handleExportSessionArchive = useCallback(async (id) => {
        if (!bridge.available || !bridge.sessions.exportSessionArchive) return;
        if (exportingSessionIdsRef.current.has(id)) return;
        const chat = (allSidebarTasksRef.current || []).find(c => c.id === id);
        const title = ((chat && chat.title) || 'session').trim() || 'session';
        const stem = [...title].slice(0, 30).join('');
        const defaultName = `pinvou-session-${stem}-${id.slice(0, 8)}.tar.xz`;
        setExportingSessionIds(prev => new Set(prev).add(id));
        try {
          const result = await bridge.sessions.exportSessionArchive(id, defaultName);
          if (!result) return;
          setSettingsToast(t.exportSessionDone(result.path));
        } catch (error) {
          console.warn('export session archive failed', error);
          setSettingsToast(t.exportSessionFailed);
        } finally {
          setExportingSessionIds(prev => {
            const next = new Set(prev);
            next.delete(id);
            return next;
          });
        }
      }, [t]);

      const handleToggleSessionPinned = useCallback(async (id, pinned) => {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.toggleSessionPinned(id, pinned);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }, [codexSessions, refreshCodexSessions]);

      // The archive confirmation needs the session title: read through a
      // latest-ref so the callback itself stays stable and the RecentItem
      // memo is not defeated by this prop on every render.
      const handleArchiveSession = useCallback((id) => {
        const chat = (allSidebarTasksRef.current || []).find(c => c.id === id);
        setArchiveConfirm(chat || { id, title: t.newChat });
      }, [t]);

      // "Open session folder": shared by RecentItem and the conversation
      // management page; the bridge is a module singleton, so its dependency
      // is constant.
      const handleRevealSessionFolder = useCallback((id) => {
        if (bridge.artifacts.revealSessionFolder) bridge.artifacts.revealSessionFolder(id);
      }, []);

      async function confirmArchiveSession() {
        const id = archiveConfirm && archiveConfirm.id;
        const isCodexSession = archiveConfirm && archiveConfirm.taskKind === 'codex';
        setArchiveConfirm(null);
        if (id && bridge.available) {
          try {
            const archived = await bridge.sessions.archiveSession(id);
            if (archived === false) {
              setSettingsToast(t.sessionBatchFailed(1));
              return;
            }
            if (isCodexSession) {
              if (activeCodexId === id) updateActiveCodexSession(null);
              await refreshCodexSessions().catch(() => {});
            }
            setArchiveToast(true);
          } catch (error) {
            console.warn('archive session failed', error);
            setSettingsToast(t.sessionBatchFailed(1));
          }
        }
      }

      async function handleRestoreArchivedSession(id) {
        if (bridge.available) await bridge.sessions.restoreArchivedSession(id);
        await refreshCodexSessions().catch(() => {});
      }

      // ── 项目层:分组归档是纯逻辑层操作,永不触碰会话的工作目录绑定。──
      // 失败走专用的 opFailed toast(借用会话批处理文案会让报错指向错误
      // 的操作对象);bridge.projects 仅桌面存在。
      async function runProjectOp(op) {
        if (!bridge.available || !bridge.projects || projectOpsBusy) return;
        setProjectOpsBusy(true);
        try {
          await op(bridge.projects);
        } catch (error) {
          console.warn('project operation failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        } finally {
          setProjectOpsBusy(false);
        }
      }
      const handleConvertFolderToProject = (path, name) => runProjectOp(p => p.createProject(name, [path]));
      const handleRenameProject = (projectId, name) => runProjectOp(p => p.renameProject(projectId, name));
      const handleDeleteProject = (projectId) => runProjectOp(p => p.deleteProject(projectId));
      // 移动归属:纯归档操作(工作目录绑定不动)。目标 root 不覆盖会话目录时
      // 由选择器先弹"仅移动"确认,确认后也只移动、不带 add_workspace_root。
      // 成功 toast 只在 store 真返回 added_root 时带路径(当前移动语义下是
      // 防御分支,见 commands 侧契约);失败走 runProjectOp 的 opFailed。
      // 成功只关"这一笔"的弹窗:异步落地期间用户可能已把选择器换到另一个
      // 会话上,无条件清 slot 会把别人的弹窗关掉。
      const handleMoveSessionToProject = (sessionId, projectId, addWorkspaceRoot) => runProjectOp(async (p) => {
        // The picker holds the snapshot it was opened with; if the session
        // vanished meanwhile (e.g. deleted from another window), the store
        // rejects every attempt and the confirm panel retries a doomed op
        // forever — the dead-target loop the pendingProject derivation
        // already prevents for projects. Retire the picker instead.
        if ((allSidebarTasksRef.current || []).every(task => task.id !== sessionId)) {
          setMoveToProjectSession(current => (current && current.id === sessionId) ? null : current);
          return;
        }
        const outcome = await p.moveSessionToProject(sessionId, projectId, addWorkspaceRoot);
        // The bridge notifies before this op resolves, but the sidebar
        // regroup that notification triggers is an asynchronous React commit
        // — querying the DOM right here would still see the pre-regroup row.
        // Passive cleanup of the dialog unmount runs after that commit's DOM
        // mutations, so store a resolver and look the moved row's new node up
        // at close time (see useDialogFocusRestore). The row container is
        // role="presentation" and cannot take focus, so resolve its label
        // button — the same focusable element the context menu hands off to.
        movePickerRestoreRef.current = () => {
          const row = document.querySelector(
            `[data-session-key="${CSS.escape(String(sessionId))}"]`,
          );
          return row ? row.querySelector('button[data-drag-surface]') : null;
        };
        setMoveToProjectSession(current => (current && current.id === sessionId) ? null : current);
        // 提交后一并清预置目标,避免残留状态泄漏到下一次打开(finding 7)。
        setMoveToPresetProject(null);
        setSettingsToast(
          outcome && outcome.added_root
            ? t.uiProjects.movedNoticeWithFolder(outcome.added_root)
            : t.uiProjects.movedNotice,
        );
      });
      // 拖拽落点:root 已覆盖的直接移动;未覆盖的带着预置目标打开选择器,
      // 进入"仅移动"确认(刻意 move-only:绝不带 add_workspace_root,面板
      // 文案已说明文件夹留在项目外;menu 路径则不带预置)。判定用共享的
      // needsAddFolderConfirm,与选择器的初始化器/选择路径保持同源。
      const handleDropSessionOnProject = (sessionId, projectId) => {
        // busy 在最外层统一静默忽略,两条路径一致:与侧栏其他拖拽反馈
        // 相同不额外打断(runProjectOp 内部同样有守卫),也避免落点挂出
        // 一个 busy 全禁用、无法关闭的预置确认面板。
        if (projectOpsBusy) return;
        const chat = sidebarTaskHistory.find(c => c.id === sessionId);
        if (!chat) return;
        const projects = sidebarProjectsData ? sidebarProjectsData.projects : [];
        const target = (projects || []).find(p => p && p.id === projectId);
        if (!target) return;
        // 拖回当前所属项目 = 选择器里禁用当前项的同一语义,直接忽略。
        if (resolveSessionProjectId(chat, projects, sidebarProjectsData ? sidebarProjectsData.assignments : {}) === projectId) return;
        if (needsAddFolderConfirm(chat, target)) {
          setMoveToPresetProject(projectId);
          setMoveToProjectSession(chat);
          return;
        }
        handleMoveSessionToProject(sessionId, projectId, false);
      };
      // Folder rebind (repairs the broken link): click "Rebind" on a project
      // header with an unavailable root → system folder picker → confirmation
      // dialog. Two-phase confirmation: the first call omits confirmExisting,
      // the backend rejects when the old folder still exists, and the dialog
      // escalates to the strong warning for the user to confirm again.
      const startRebindWorkspace = async (fromPath) => {
        // Do not reopen while rebindDraft is already open: with focus left on
        // the badge, pressing Enter re-triggers onRebind (review #463 minor),
        // and the projectOpsBusy guard does not cover that window.
        if (!bridge.files || !bridge.files.pickRebindFolder || projectOpsBusy || rebindDraft) return;
        try {
          // Single folder, with a title matching the rebind semantics
          // (review #463 Minor 6): no longer borrowing KB's multi-select
          // import picker.
          const to = await bridge.files.pickRebindFolder();
          if (!to) return;
          // No session count: the command actually rebinds every session
          // under `from`; the sidebar group's rendered count is only a
          // subset, so a numeric promise would not match the
          // RebindWorkspaceReport (finding 10).
          setRebindDraft({ from: fromPath, to, warnExisting: false });
        } catch (error) {
          // Picker rejection must be user-visible (review #463 minor), not
          // console-only; the generic opFailed copy covers this failure
          // class, and the warn keeps the detail available for diagnostics.
          console.warn('pick rebind folder failed', error);
          setSettingsToast(t.uiProjects.opFailed);
        }
      };
      const confirmRebindWorkspace = async (confirmExisting) => {
        if (!bridge.projects || !rebindDraft || projectOpsBusy) return;
        setProjectOpsBusy(true);
        // Clear the previous attempt's inline error/busy hint so it does
        // not stack with this run's result.
        setRebindDraft(prev => prev && { ...prev, error: null, busySessionIds: null });
        // Feed the previous report's post-busy ids back (review #463
        // F-Major): a session an earlier run moved and reported post-busy is
        // routed by the backend into the same to-lane retry population as a
        // healthy session, so without the feed-back a busy-refused carryover
        // session would appear in NO report field and the dialog would close
        // claiming full success while its old-cwd runtime stays resident.
        // The backend honors only the intersection with its own retry
        // population, so this list can never widen the eviction set.
        const previousPostBusySessionIds = (rebindDraft.partial && rebindDraft.partial.postBusyIds) || [];
        try {
          const report = await bridge.projects.rebindWorkspaceRoot(
            rebindDraft.from, rebindDraft.to, confirmExisting, previousPostBusySessionIds);
          const rebound = (report && report.rebound_session_ids) ? report.rebound_session_ids.length : 0;
          const failed = (report && report.failed_session_ids) ? report.failed_session_ids.length : 0;
          const postBusy = (report && report.post_busy_session_ids) ? report.post_busy_session_ids.length : 0;
          // The dialog stays open whenever the report still has something the
          // user must act on — failed sessions to retry, or sessions whose
          // runtime the idle gate refused (round-8 MAJOR-2: closing on the
          // post-busy-only case left "retry once when idle" with no entry
          // point, because the unavailable-root badge disappears once the root
          // has moved). Rerunning the backend with the same from/to converges
          // (the snapshot includes unsynced sessions; already-rebound ones are
          // no-ops). The post-busy ids are kept for the next retry's
          // feed-back (F-Major).
          if (failed > 0 || postBusy > 0) {
            setRebindDraft(prev => prev && {
              ...prev,
              partial: {
                rebound,
                failed,
                failedIds: (report && report.failed_session_ids) || [],
                postBusy,
                postBusyIds: (report && report.post_busy_session_ids) || [],
              },
            });
          } else {
            setRebindDraft(null);
            if (rebound > 0) {
              setSettingsToast(t.uiProjects.rebindSuccess(rebound));
            } else {
              // A retry after everything already converged (or a root with
              // no sessions at all) returns an empty report; "Rebound 0"
              // would read as a failure (review #463 minor).
              setSettingsToast(t.uiProjects.rebindUpToDate);
            }
          }
          await refreshCodexSessions().catch((error) => {
            // Failure is not swallowed: the session list self-heals via the
            // session:list_changed event, but a silent gap after an explicit
            // failure must stay visible for troubleshooting
            // (review #463 minor).
            console.warn('refresh sessions after rebind failed', error);
          });
        } catch (error) {
          // Typed-marker matching (finding 11 / Minor 7 / round-8 M4): the
          // backend prefixes every user-reachable outcome with a stable ASCII
          // marker and we match only that prefix, never human copy. The mapping
          // lives in a pure helper so both halves of the contract are unit
          // tested (review #463 round-8 minor 10).
          const classified = classifyRebindError(error, t);
          if (classified.kind === 'old-root-exists') {
            setRebindDraft(prev => prev && { ...prev, warnExisting: true, error: null });
          } else if (classified.kind === 'sessions-busy') {
            // Busy rejection is the fence's high-frequency happy path
            // (Minor 7): map it to i18n copy; only session ids follow the
            // marker, and they are shown verbatim for troubleshooting.
            setRebindDraft(prev => prev && {
              ...prev,
              busySessionIds: classified.busySessionIds,
              error: null,
            });
          } else if (classified.kind === 'copy') {
            setRebindDraft(prev => prev && { ...prev, error: classified.message });
          } else {
            console.warn('rebind workspace failed', error);
            // On failure keep the dialog open with the error inline
            // (review #463 M7): in-place display persists and sits next to
            // the retry; an unmapped backend error is shown verbatim as a
            // diagnostic detail rather than guessed at.
            setRebindDraft(prev => prev && { ...prev, error: classified.message });
          }
        } finally {
          setProjectOpsBusy(false);
        }
      };

      function sessionRowsForIds(ids) {
        const byId = new Map(allSidebarTasks.map(item => [item.id, item]));
        return (ids || []).map(id => byId.get(id) || { id });
      }

      function reportBatchFailures(result) {
        if (result.failed > 0) setSettingsToast(t.sessionBatchFailed(result.failed));
      }

      // 对话管理页批量操作:按会话类型分流并等待全部结果,避免未执行完成就误报成功。
      async function handleBatchArchiveSessions(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(sessionRowsForIds(ids), 'archive', {
          archive: id => bridge.sessions.archiveSession(id),
          archiveCodex: id => bridge.sessions.archiveSession(id),
        });
        const nextCodexSessions = await refreshCodexSessions().catch(() => null);
        if (activeCodexId && Array.isArray(nextCodexSessions) && nextCodexSessions.every(session => session.id !== activeCodexId)) {
          updateActiveCodexSession(null);
        }
        if (result.succeeded > 0) setArchiveToast(true);
        reportBatchFailures(result);
        return result;
      }

      async function handleBatchDeleteSessions(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(sessionRowsForIds(ids), 'delete', {
          delete: id => bridge.sessions.deleteSession(id),
        });
        const nextCodexSessions = await refreshCodexSessions().catch(() => null);
        if (activeCodexId && Array.isArray(nextCodexSessions) && nextCodexSessions.every(session => session.id !== activeCodexId)) {
          updateActiveCodexSession(null);
        }
        reportBatchFailures(result);
        return result;
      }

      async function handleBatchRestoreArchived(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(ids.map(id => ({ id })), 'restore', {
          restore: id => bridge.sessions.restoreArchivedSession(id),
        });
        await refreshCodexSessions().catch(() => {});
        reportBatchFailures(result);
        return result;
      }

      useEffect(() => {
        if (!archiveToast) return;
        const timer = setTimeout(() => setArchiveToast(false), 3500);
        return () => clearTimeout(timer);
      }, [archiveToast]);

      useEffect(() => {
        if (!settingsToast) return;
        const timer = setTimeout(() => setSettingsToast(''), 3000);
        return () => clearTimeout(timer);
      }, [settingsToast]);

      async function handleToggleSuperPerm() {
        const target = !superPerm;
        if (!bridge.available) {
          setSuperPerm(target);
          return;
        }
        setSuperPerm(target);
        try {
          const result = await bridge.interaction.toggleSuperPerm();
          if (!result || result.ok === false) {
            setSuperPerm(!!(result && result.enabled));
            setSettingsToast((result && result.error) || t.uiMainApp.superPermFailed);
          }
        } catch (error) {
          setSuperPerm(!target);
          setSettingsToast(String(error || t.uiMainApp.superPermFailed));
        }
      }

      function handleSetTheme(scheme) {
        setColorScheme(scheme);
        if (isWeb) {
          try { window.localStorage.setItem(COLOR_SCHEME_STORAGE_KEY, scheme); } catch { /* silently degrade when WebView disables storage */ }
          return;
        }
        if (bridge.available) {
          // `color_scheme` is the authoritative preference (system/light/dark);
          // `theme` mirrors the resolved value so consumers that only know the
          // legacy field (e.g. an older build running after a downgrade) keep rendering.
          bridge.settings.saveSettings({
            theme: resolveTheme(scheme, systemDark) === 'dark' ? 'genesis' : 'liquid-light',
            color_scheme: scheme,
          });
        }
      }

      function handleSetSearchProvider(p) {
        if (p === searchProvider) return;
        setEnabledSearchProviders(prev => [...new Set(['bing', ...prev, p])]);
        setSearchProvider(p);
      }

      function handleAddSearchProvider(p) {
        setEnabledSearchProviders(prev => [...new Set(['bing', ...prev, p])]);
        handleSetSearchProvider(p);
      }

      function handleDeleteSearchProvider(p) {
        if (p === 'bing') return;
        setEnabledSearchProviders(prev => {
          const next = prev.filter(x => x !== p);
          return next.length ? next : ['bing'];
        });
        setSearchKeyDrafts(prev => ({ ...prev, [p]: '' }));
        setSearchKeyActions(prev => ({ ...prev, [p]: 'delete' }));
        if (searchProvider === p) handleSetSearchProvider('bing');
      }

      function handleSetSearchApiKey(k, providerOverride) {
        const targetProvider = providerOverride || searchProvider;
        setSearchKeyDrafts(prev => ({ ...prev, [targetProvider]: k }));
        setSearchKeyActions(prev => ({ ...prev, [targetProvider]: k.trim() ? 'replace' : 'keep_existing' }));
      }

      async function handleConfirmSearchConfig() {
        if (!bridge.available) return;
        const search = buildSearchSettingsPayload();
        // 浏览器宿主没有重启桌面进程的权限；只保存，待桌面端下次重启后生效。
        const saved = isWeb
          ? await bridge.settings.saveSearchSettings(search)
          : await bridge.settings.saveSearchSettingsAndRestart(search);
        if (saved === false) setSettingsToast(t.uiMainApp.searchSaveFailed);
      }

      async function handleSaveSearchConfig() {
        if (!bridge.available) return true;
        const search = buildSearchSettingsPayload();
        const saved = await bridge.settings.saveSearchSettings(search);
        if (saved === false) {
          setSettingsToast(t.uiMainApp.searchSaveFailed);
          return false;
        }
        return true;
      }

      function handleSetLanguage(lang) {
        // en/ja 是惰性词典 chunk:先装载再切状态/广播,辅助窗口(桌宠/阅读器)
        // 收到 ui:language_changed 时词典必须已在本窗就位(各入口首帧引导只保证
        // 初始语言)。装载失败(资源损坏)保持原语言,不产生半翻译界面。
        // 经「最新选择胜出」门落地:ja chunk 静态依赖 en chunk,先选 ja 再选
        // en 时旧 ja 请求可能后完成并覆盖新选择(见 createLatestLanguageGate)。
        switchToLanguage(lang, () => {
          setLanguage(lang);
          if (isWeb) {
            try { window.localStorage.setItem('pinvou.web.language', lang); } catch { /* silently degrade when WebView disables storage */ }
            return;
          }
          if (isTauriAvailable()) {
            tauriEvents.emit('ui:language_changed', { language: lang }).catch(() => {});
          }
          if (bridge.available) {
            bridge.settings.saveSettings({ language: LANG_TO_TAG[lang] || 'zh-Hans' });
          }
        });
      }

      function handleSetMemoryEnabled(enabled) {
        if (bridge.available) {
          const memoryAvailable = (LANG_TO_TAG[language] || 'zh-Hans') === 'zh-Hans';
          bridge.settings.saveSettings({ memory_enabled: memoryAvailable && !!enabled });
        }
      }

      function handleSetPetEnabled(enabled) {
        if (!can('pet') || !bridge.available) return;
        // 单一路径:set_pet_enabled 负责持久化 + 窗口显隐 + 广播
        // pet:enabled_changed(bridge 听到后刷新 settings 副本,防旧值回写)。
        invokeTauri('set_pet_enabled', { enabled: !!enabled }).catch(() => {});
      }

      async function handleSetTaskCompletedNotif(enabled) {
        const nextEnabled = !!enabled;
        const previousEnabled = taskCompletedNotif;
        setTaskCompletedNotif(nextEnabled);
        if (bridge.available) {
          const saved = await bridge.settings.saveSettings({
            notifications: { enabled: nextEnabled, task_completed: nextEnabled },
          });
          if (saved === false) {
            setTaskCompletedNotif(previousEnabled);
          }
        }
      }

      // 侧栏任务列表「按日期折叠」开关:纯 UI 偏好,写 settings.sidebar.date_grouping
      function handleSetSidebarDateGrouping(enabled) {
        if (bridge.available) bridge.settings.saveSettings({ sidebar: { date_grouping: !!enabled } });
      }

      // 移动壳层派生数据：顶栏标题跟随当前视图（对话态显示会话标题）；
      // 未读红点与侧栏入口同源，避免两套提醒逻辑漂移。
      const scheduledUnread = !!(bs && ((bs.scheduledTasks || []).some(task => task.hasUnreadRuns)
        || (bs.scheduledTaskRecentRuns || []).some(run => run && run.unread)));
      const mobileTitle = currentView === 'chat'
        ? ((((chatHistory || []).find(c => c.id === activeChat)) || {}).title || 'PINVOU')
        : currentView === 'codex'
          ? ((((codexHistory || []).find(c => c.id === activeCodexId)) || {}).title || t.uiCodex.untitledSession)
        : ({ search: t.searchChats, scheduled: t.scheduledPlans, monitor: t.monitor, cardpool: t.cardPool, toolStore: t.toolStore, outputs: t.outputs, knowledge: t.knowledge, settings: t.settings, browser: t.browser }[currentView] || 'PINVOU');
      const mobileNavigate = (view, beforeNavigate) => {
        setMobileMoreOpen(false);
        navigateFromScheduledRun(view, beforeNavigate);
      };
      const mobileMoreViews = ['search', 'outputs', 'knowledge', 'toolStore', 'settings', 'browser'];
      const mobileMoreActive = mobileMoreViews.includes(currentView)
        || (currentView === 'scheduled' && !(bs && bs.scheduledRunContext));

      // 侧栏任务列表按日期折叠(默认开;settings.sidebar.date_grouping === false 时平铺)
      const sidebarDateGrouping = !bs || !bs.settings || !bs.settings.sidebar || bs.settings.sidebar.date_grouping !== false;
      // 一键折叠/展开「任务列表」下的全部分组:直接写入各组的展开 map,
      // 不引入总开关变量;按钮状态由当前可见组的真实聚合推导——
      // 全部展开显示「折叠」,其余(含全折叠/混合)显示「展开」,
      // 手动逐组操作后按钮也不会与实际状态脱节。
      const visibleTaskGroupOpens = sidebarCodeListActive
        ? sidebarFolderGroups.map(g => folderGroupOpen[g.key] ?? true)
        : (sidebarDateGrouping ? sidebarTaskGroups.map(g => dateGroupOpen[g.key] ?? (g.key === todayDateKey)) : []);
      const allTaskGroupsExpanded = visibleTaskGroupOpens.length > 0 && visibleTaskGroupOpens.every(Boolean);
      const setAllTaskGroups = (open) => {
        if (sidebarCodeListActive) {
          setFolderGroupOpen(prev => {
            const next = { ...prev };
            for (const g of sidebarFolderGroups) next[g.key] = open;
            return next;
          });
        } else if (sidebarDateGrouping) {
          setDateGroupOpen(prev => {
            const next = { ...prev };
            for (const g of sidebarTaskGroups) next[g.key] = open;
            return next;
          });
        }
      };
      // Task-item renderer shared by the date-grouped and flat layouts.
      const renderSidebarTaskItem = (chat) => {
        const detachKind = chat.taskKind === 'codex' ? 'codex-session' : 'session';
        // One availability gate for both dragging and the "move to project"
        // menu item: with an empty project list (bootstrap windows, users
        // with zero projects) the row is not draggable,
        // avoiding a dead gesture with zero reachable drop targets. #445
        // bound work sessions (taskKind regular + workspacePath) have the
        // same rights as code sessions — grouping follows binding, and the
        // move entry point follows too (same signal as review #452
        // finding 5).
        const projectMovesAvailable = (chat.taskKind === 'codex' || !!chat.workspacePath) && bridge.projects && !!sidebarProjectsData?.projects?.length;
        return (
          <RecentItem
            key={chat.taskKind === 'scheduled' ? `${chat.scheduledRun?.automationId || ''}:${chat.scheduledRun?.id || chat.id}` : `${chat.taskKind}:${chat.id}`}
            chat={chat}
            theme={activeTheme}
            t={t}
            active={chat.taskKind === 'codex'
              ? activeCodexId === chat.id && currentView === 'codex'
              : chat.scheduledRun
                ? !!(bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === chat.id)
                : activeChat === chat.id && currentView === 'chat'}
            personaTarget={chat.taskKind !== 'codex' && !chat.scheduledRun && activeChat === chat.id && currentView === 'cardpool'}
            onSelect={chat.taskKind === 'codex'
              ? handleSwitchCodexSession
              : chat.scheduledRun
                ? cachedItemCallback(sidebarScheduledSelectCallbacks, chat, (c) => () => handleOpenScheduledRunShortcut(c.scheduledRun))
                : handleSwitchSession}
            onRename={handleRenameSession}
            onDelete={handleDeleteSession}
            onTogglePinned={handleToggleSessionPinned}
            onOpenFolder={can('externalSystemOpen') ? handleRevealSessionFolder : undefined}
            onExportArchive={chat.taskKind !== 'codex' && !exportingSessionIds.has(chat.id) && bridge.sessions.exportSessionArchive ? handleExportSessionArchive : undefined}
            onArchive={handleArchiveSession}
            onMoveToProject={projectMovesAvailable ? openMovePicker : undefined}
            dndPayload={projectMovesAvailable && sidebarCodeListActive
              ? cachedItemCallback(sidebarDndPayloads, chat, (c) => ({ sessionId: c.id }))
              : undefined}
            dndDisabled={!!dragAvatar}
            onDragEnd={clearDropTarget}
            dragKind={detachKind}
            dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === `${detachKind}:${chat.id}`}
            onPickUp={canDetachWindows
              ? cachedItemCallback(sidebarPickUpCallbacks, chat, (c) => (geom) => beginTearOff(detachKind, c.id, c.title, geom))
              : undefined}
          />
        );
      };

      // Sidebar main nav callback set (stable references): with NavItem
      // memoized, onClick/onPickUp must be reference-stable or the memo never
      // hits. Navigation goes through navigateFromScheduledRun (reads bsRef
      // internally); the tear-off closure depends only on beginTearOff
      // (stable) and the current language dictionary t.
      const navNavigateHandlers = useMemo(() => ({
        scheduled: () => navigateFromScheduledRun('scheduled'),
        outputs: () => navigateFromScheduledRun('outputs'),
        monitor: () => navigateFromScheduledRun('monitor', () => {
          const liveBridge = window.TauriBridge || bridge;
          if (liveBridge?.monitor && typeof liveBridge.monitor.startMonitorPolling === 'function') liveBridge.monitor.startMonitorPolling();
        }),
        toolStore: () => navigateFromScheduledRun('toolStore'),
        cardpool: () => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false)),
        knowledge: () => navigateFromScheduledRun('knowledge'),
        chat: () => navigateFromScheduledRun('chat'),
      }), [navigateFromScheduledRun]);
      const navPickUpHandlers = useMemo(() => ({
        outputs: (geom) => beginTearOff('outputs', undefined, t.outputs, geom),
        monitor: (geom) => beginTearOff('monitor', undefined, t.monitor, geom),
        toolstore: (geom) => beginTearOff('toolstore', undefined, t.toolStore, geom),
        cardpool: (geom) => beginTearOff('cardpool', undefined, t.cardPool, geom),
        knowledge: (geom) => beginTearOff('knowledge', undefined, t.knowledge, geom),
      }), [t, beginTearOff]);
      const openSearchOverlay = useCallback(() => setSearchOverlayOpen(true), []);
      const handleNavNewChat = useCallback(() => { handleNewChat(); }, [handleNewChat]);
      const apiKeyGateOpen = shouldShowApiKeyGate(bs, currentView, bridge.available);
      const vllmSetupModalOpen = !!(
        can('localModelSetup')
        && bs
        && bs.vllmSetup
        && bs.vllmSetup.eligible
        && !bs.vllmSetupDismissed
      );
      const browserOverlayIntent = [
        archiveConfirm ? 'archive-confirm' : '',
        searchOverlayOpen ? 'search' : '',
        personaEditor ? 'persona-editor' : '',
        savedConfirm ? 'saved-confirm' : '',
        can('webAccessAdmin') && webAccessOpen ? 'web-access' : '',
        apiKeyGateOpen ? 'api-key' : '',
        vllmSetupModalOpen ? 'vllm-setup' : '',
        bs && bs.pinvouModal ? 'pinvou-review' : '',
        isCompactShell && isSidebarOpen ? 'mobile-sidebar' : '',
        isCompactShell && mobileMoreOpen ? 'mobile-more' : '',
        moveToProjectSession ? 'move-picker' : '',
      ].filter(Boolean).join('|');
      const browserOverlayPublicationReady = !!browserOverlayIntent
        && publishedBrowserOverlayIntent === browserOverlayIntent;
      // Keep the surface hidden while one already-published overlay hands off to
      // another. The replacement itself is still withheld until its own barrier
      // attempt has settled.
      const browserOverlayOpen = !!publishedBrowserOverlayIntent;
      const browserBlockingLayerOpen = !browserPaneAllowed || browserOverlayOpen;
      const browserSurfaceSuspended = browserResizeActive
        || browserDocumentHidden
        || rightDockOcclusionPublications.length > 0
        || rightDockState.occluded
        || rightDockState.activePanelId !== 'browser'
        || browserBlockingLayerOpen;
      const compactBrowserSurfaceSuspended = browserDocumentHidden
        || browserOverlayOpen
        || currentView !== 'browser';
      const browserNativeSurfaceVisible = isCompactShell
        ? browserActive && currentView === 'browser' && !compactBrowserSurfaceSuspended
        : browserActive
          && browserPaneOpen
          && browserPaneSelected
          && !browserSurfaceSuspended;
      useLayoutEffect(() => {
        browserSurfaceTransitionContextRef.current = {
          sessionId: browserViewSessionId,
          hasWorkspace: browserActive && !!browserViewSessionId,
          visible: browserNativeSurfaceVisible,
          compact: isCompactShell,
          scheduledRunChat: !!(bs && bs.scheduledRunContext),
        };
      }, [
        browserActive,
        browserNativeSurfaceVisible,
        browserViewSessionId,
        bs,
        isCompactShell,
      ]);
      useLayoutEffect(() => {
        let disposed = false;
        if (!browserOverlayIntent) {
          browserUiTransitionGateRef.current.invalidate('overlay');
          setPublishedBrowserOverlayIntent('');
          return () => { disposed = true; };
        }
        void runBrowserUiTransition(() => {
          if (disposed) return false;
          setPublishedBrowserOverlayIntent(browserOverlayIntent);
          return true;
        }, {
          channel: 'overlay',
          hideMode: 'visible',
        });
        return () => { disposed = true; };
      }, [browserOverlayIntent, runBrowserUiTransition]);

      // The 11 props identical across ChatView's two mount points (main chat / scheduled-run chat) are
      // consolidated into one block; each mount point writes only its differing props (prefill / focus tick / code-mode entry),
      // so two long prop lists cannot silently drift after copy-paste.
      // The props below are NOT consolidated and stay as JSX literals (source-string contracts: tests regex-assert
      // that main.jsx contains these literals; see the individual test files):
      // - onBackScheduledRun：scheduled_tasks_unit.test.js
      // - browserDockAvailable / rightDockActivePanelId /
      //   onRightDockPanelSelectionChange：browser_native_surface.test.mjs
      const chatViewBaseProps = {
        theme: activeTheme,
        onOpenWorkspacePicker: ({ lane, mode }) => { setPickerExcluded(null); setWorkspacePicker({ lane, mode }); },
        t,
        bs,
        onOpenEditor: handleOpenPersonaEditor,
        justInstalledTool,
        setJustInstalledTool,
        onGotoSettings: () => openSettingsSection('general'),
        onGotoModelSettings: () => openSettingsSection('model'),
        onGotoTools: () => navigateFromScheduledRun('toolStore'),
        browserDockOpen: browserPaneOpen,
        onOpenBrowserDock: openBrowserDock,
        // Align feedback (and other keychain notices) surface as the host
        // toast: without this prop every align outcome is silent in the chat
        // lane (review #484 M4).
        onNotify: setSettingsToast,
      };
      // The three byte-identical empty states in the sidebar task list (task groups / date groups / flat list) share one node.
      const sidebarTaskEmptyNode = (
        <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
          {t.sidebarTaskEmpty}
        </div>
      );

      return (
        <div data-testid="app-root" data-current-view={currentView} data-platform={isWeb ? 'web' : 'desktop'}
          className={`flex flex-col h-screen font-sans overflow-hidden antialiased transition-colors duration-300 ${activeTheme === 'dark' ? 'bg-[#131314] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}
          style={isWeb ? {
            // inset shorthand expanded to physical properties: Safari 14.0 (iOS 14.0 web) cannot parse the shorthand.
            ...(isCompactShell ? { position: 'fixed', top: 0, right: 0, bottom: 0, left: 0, width: '100%' } : {}),
            height: visualViewportHeight ? `${visualViewportHeight}px` : '100dvh',
            paddingTop: 'env(safe-area-inset-top)',
            paddingRight: 'env(safe-area-inset-right)',
            paddingBottom: 'env(safe-area-inset-bottom)',
            paddingLeft: 'env(safe-area-inset-left)',
          } : undefined}>

          <VoiceShortcutRouter />
          <WebConnectionStatus theme={activeTheme} t={t} />

          {/* 撕离拖拽 avatar:被拎起的标签,跟随光标(DOM 实现,丝滑跟手、不选中文字) */}
          {dragAvatar && (
            <div style={{ position:'fixed', left: dragAvatar.x, top: dragAvatar.y, width: dragAvatar.w, height: dragAvatar.h,
              pointerEvents:'none', zIndex:9999, borderRadius:14, overflow:'hidden', whiteSpace:'nowrap',
              display:'flex', alignItems:'center', padding:'0 16px', fontWeight:600, fontSize:15,
              background: activeTheme === 'dark' ? '#A8C7FA' : '#0B57D0', color: activeTheme === 'dark' ? '#041E49' : '#ffffff',
              boxShadow:'0 14px 34px rgba(0,0,0,.5)', transform:'scale(1.03)', opacity:0.96 }}>
              {dragAvatar.label}
            </div>
          )}

          {archiveConfirm && browserOverlayPublicationReady && createPortal(
            <ArchiveConfirmDialog
              theme={activeTheme}
              t={t}
              onCancel={() => setArchiveConfirm(null)}
              onConfirm={confirmArchiveSession}
            />,
            document.body
          )}

          {archiveToast && createPortal(
            <ArchiveToast
              theme={activeTheme}
              t={t}
              onClose={() => setArchiveToast(false)}
              onView={() => {
                setArchiveToast(false);
                setSearchShowArchived(true);
                navigateFromScheduledRun('search');
              }}
            />,
            document.body
          )}

          {settingsToast && createPortal(
            // Layer sits above modal overlays (picker backdrop is z-[200]) so a
            // failure raised under an open dialog stays visible; a filesystem
            // path in the message must not push the pill past the viewport.
            <div className="fixed left-1/2 bottom-8 z-[210] -translate-x-1/2 max-w-[80vw] truncate rounded-full bg-black/80 px-4 py-2 text-[13px] font-medium text-white shadow-2xl">
              {settingsToast}
            </div>,
            document.body
          )}

          {manageFoldersProject && (
            <ManageProjectFoldersDialog
              open
              project={manageFoldersProject}
              neverRoots={(sidebarProjectsData && sidebarProjectsData.neverMaterializeRoots) || []}
              mode={activeLaneMode()}
              busy={projectOpsBusy}
              t={t}
              onClose={closeManageFolders}
              onAddFolder={handleManageAddFolder}
              onRemoveRoot={handleManageRemoveRoot}
              onSetPrimary={handleManageSetPrimary}
              onRename={(name) => handleRenameProject(manageFoldersProject.id, name)}
              onExcludeRoot={(root) => handleSetNeverMaterialize(root, true)}
              onRevokeExclusion={(root) => handleSetNeverMaterialize(root, false)}
            />
          )}

          {workspacePicker && (
          // Conditional mount (the ManageProjectFoldersDialog idiom): the query
          // and the expanded row must not leak across opens — remounting resets
          // them, so a leftover filter can never render a false empty state.
          <WorkspacePickerDialog
            open
            rows={workspacePickerRows}
            mode={workspacePicker.mode}
            language={language}
            busy={pickerBusy}
            webOnly={!can('desktopChrome') || !bridge.projects}
            excludedFolder={pickerExcluded}
            t={t}
            onClose={closeWorkspacePicker}
            onSelectProject={handlePickerSelectProject}
            onTemporary={handlePickerTemporary}
            onBrowse={handlePickerBrowse}
            onBrowseExcluded={(folder) => applyWorkspaceTarget({ lane: pickerLane(), path: folder, projectId: null, roots: [folder] })}
            onDismissExcluded={() => setPickerExcluded(null)}
          />
          )}

          {rebindDraft && (
            <RebindFolderDialog
              from={rebindDraft.from}
              to={rebindDraft.to}
              warnExisting={rebindDraft.warnExisting}
              errorMessage={rebindDraft.error}
              partial={rebindDraft.partial || null}
              busySessionIds={rebindDraft.busySessionIds || null}
              t={t}
              busy={projectOpsBusy}
              onCancel={() => setRebindDraft(null)}
              onConfirm={confirmRebindWorkspace}
            />
          )}

          {moveToProjectSession && browserOverlayPublicationReady && (
            <MoveToProjectDialog
              session={moveToProjectSession}
              projects={sidebarProjectsData ? sidebarProjectsData.projects : []}
              currentProjectId={resolveSessionProjectId(
                moveToProjectSession,
                sidebarProjectsData ? sidebarProjectsData.projects : [],
                sidebarProjectsData ? sidebarProjectsData.assignments : {},
              )}
              presetProjectId={moveToPresetProject}
              t={t}
              busy={projectOpsBusy}
              restoreTargetRef={movePickerRestoreRef}
              onClose={() => { setMoveToPresetProject(null); setMoveToProjectSession(null); }}
              onMove={(projectId, addWorkspaceRoot) => handleMoveSessionToProject(
                moveToProjectSession.id, projectId, addWorkspaceRoot)}
            />
          )}

          {searchOverlayOpen && browserOverlayPublicationReady && createPortal(
            <SearchOverlay
              theme={activeTheme}
              history={chatHistory}
              t={t}
              onSelect={handleSearchSelect}
              onClose={() => setSearchOverlayOpen(false)}
            />,
            document.body
          )}

          {can('desktopChrome') && <TitleBar t={t} sidebarOpen={isSidebarOpen} />}

          {isCompactShell && (
            <MobileTopBar theme={activeTheme} t={t} title={mobileTitle}
              onMenu={() => setIsSidebarOpen(true)}
              onNewChat={currentView === 'chat' || currentView === 'codex' ? () => handleNewChat() : undefined} />
          )}

          <SidePanelLayoutProvider onPresenceChange={setOpenSidePanelCount}>
          <RightDockProvider
            onStateChange={handleRightDockStateChange}
            onBeforeOcclusionPublish={publishRightDockOcclusion}
            onOcclusionRelease={releaseRightDockOcclusion}
          >
          <div className={`flex flex-1 min-h-0 ${activeTheme === 'dark' ? (isSidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]') : 'bg-[#F0F4F9]'}`}>

          {isWeb && isSidebarOpen && browserOverlayPublicationReady && (
            <button
              type="button"
              data-testid="mobile-navigation-close"
              aria-label={t.uiMainApp.closeNavigation}
              onClick={() => setIsSidebarOpen(false)}
              className="fixed inset-0 z-30 hidden bg-black/40 max-sm:block"
            />
          )}

          {/* ================= Sidebar (Gemini Style) ================= */}
          <div
            id="app-sidebar"
            data-testid="app-sidebar"
            style={{
              // The compact-shell drawer does not inherit the persisted desktop width:
              // the drawer has no drag handle, and a width beyond the viewport would cover
              // the tap-on-backdrop-to-dismiss channel (the z-30 backdrop sits below the
              // z-40 sidebar).
              width: isSidebarOpen && !isCompactShell ? sidebarWidth : undefined,
              ...(isCompactShell ? {
                display: isSidebarOpen && browserOverlayPublicationReady ? 'flex' : 'none',
                position: 'fixed',
                left: 0,
                top: 48,
                bottom: 56,
              } : {}),
            }}
            className={`${isSidebarOpen ? (isCompactShell ? 'w-[280px]' : '') : 'w-[68px]'} relative shrink-0 flex flex-col z-40 ${sidebarResizing ? '' : 'transition-all duration-300'} ${
              activeTheme === 'light'
                ? 'bg-[#F0F4F9]'
                : (isSidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]')
            }`}>

            {/* Header / Logo */}
            <div className={`px-4 py-3 max-sm:px-3 max-sm:py-0 flex items-center ${isSidebarOpen ? 'gap-3' : 'justify-center'} overflow-hidden`}>
              <button type="button"
                data-sidebar-toggle
                onClick={() => setIsSidebarOpen(!isSidebarOpen)}
                title={isSidebarOpen ? t.sidebarCollapse : t.sidebarExpand}
                className={`w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
              >
                <Menu size={20} className={activeTheme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#444746]'} />
              </button>
              <span className={`text-[18px] font-medium tracking-wide flex items-center gap-2 whitespace-nowrap transition-opacity duration-200 ${isSidebarOpen ? 'opacity-100' : 'opacity-0 w-0'}`}>
                PINVOU
              </span>
              {isSidebarOpen && !isCompactShell && (
                <button
                  type="button"
                  onClick={() => setSearchOverlayOpen(true)}
                  title={t.searchChats}
                  aria-label={t.searchChats}
                  className={`ml-auto w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                    searchOverlayOpen
                      ? (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#0B57D0]')
                      : (activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]')
                  }`}
                >
                  <Search size={19} />
                </button>
              )}
            </div>

            {/* Navigation — shrink-0 keeps it from scrolling; no matter how long the
                list is, it never squeezes the nav. The nav folds to a single expand row
                only through the manual collapse button at the bottom of the list; the
                choice persists and applies in every mode. The phone drawer (compact
                shell) always keeps the full nav so the task list keeps its vertical
                room (see web-ui.smoke's drawer-height contract). */}
            <div data-testid="sidebar-primary-nav" className={`shrink-0 flex flex-col gap-0.5 mt-1.5 max-sm:gap-0 max-sm:mt-1 ${isSidebarOpen ? 'px-3' : 'px-2 items-center'}`}>
              <NavItem
                icon={NAV_ICON_NEW_CHAT} label={t.newChat}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={handleNavNewChat}
              />
              {/* On the compact shell search is only reachable from the nav, so it must
                  stay pinned even when collapsed */}
              {(!isSidebarOpen || isCompactShell) && (
                <NavItem
                  icon={NAV_ICON_SEARCH} label={t.searchChats}
                  active={searchOverlayOpen}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={openSearchOverlay}
                />
              )}
              {isSidebarOpen && !isCompactShell && sidebarNavCollapsed ? (
                <button
                  type="button"
                  data-testid="sidebar-primary-nav-expand"
                  onClick={() => setSidebarNavCollapsedPersisted(false)}
                  title={t.sidebarNavExpand}
                  className={`w-full h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                >
                  <span className="truncate">{t.sidebarNavExpand}</span>
                  <ChevronDown size={14} className="shrink-0" />
                </button>
              ) : (
              <>
              <NavItem
                icon={NAV_ICON_SCHEDULED} label={t.scheduledPlans}
                active={currentView === 'scheduled'}
                unread={!!(bs && ((bs.scheduledTasks || []).some(task => task.hasUnreadRuns) || (bs.scheduledTaskRecentRuns || []).some(run => run && run.unread)))}
                theme={activeTheme}
                t={t}
                isSidebarOpen={isSidebarOpen}
                onClick={navNavigateHandlers.scheduled}
                onPointerEnter={NAV_PREFETCH.scheduled} onFocus={NAV_PREFETCH.scheduled}
              />
              <NavItem
                icon={NAV_ICON_OUTPUTS} label={t.outputs}
                active={currentView === 'outputs'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={navNavigateHandlers.outputs}
                onPointerEnter={NAV_PREFETCH.knowledge} onFocus={NAV_PREFETCH.knowledge}
                dragKind={canDetachWindows ? 'outputs' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'outputs:'} onPickUp={canDetachWindows ? navPickUpHandlers.outputs : undefined}
              />
              <NavItem
                icon={NAV_ICON_MONITOR} label={t.monitor}
                active={currentView === 'monitor'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onPointerEnter={NAV_PREFETCH.monitor} onFocus={NAV_PREFETCH.monitor}
                onClick={navNavigateHandlers.monitor}
                dragKind={canDetachWindows ? 'monitor' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'monitor:'} onPickUp={canDetachWindows ? navPickUpHandlers.monitor : undefined}
              />
              <NavItem
                icon={NAV_ICON_TOOL_STORE} label={t.toolStore}
                active={currentView === 'toolStore'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={navNavigateHandlers.toolStore}
                onPointerEnter={NAV_PREFETCH.toolStore} onFocus={NAV_PREFETCH.toolStore}
                dragKind={canDetachWindows ? 'toolstore' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'toolstore:'} onPickUp={canDetachWindows ? navPickUpHandlers.toolstore : undefined}
              />
              <NavItem
                icon={NAV_ICON_CARD_POOL} label={t.cardPool}
                active={currentView === 'cardpool'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={navNavigateHandlers.cardpool}
                onPointerEnter={NAV_PREFETCH.cardpool} onFocus={NAV_PREFETCH.cardpool}
                dragKind={canDetachWindows ? 'cardpool' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'cardpool:'} onPickUp={canDetachWindows ? navPickUpHandlers.cardpool : undefined}
              />
              <NavItem
                icon={NAV_ICON_KNOWLEDGE} label={t.knowledge}
                active={currentView === 'knowledge'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={navNavigateHandlers.knowledge}
                onPointerEnter={NAV_PREFETCH.knowledge} onFocus={NAV_PREFETCH.knowledge}
                dragKind={canDetachWindows ? 'knowledge' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'knowledge:'} onPickUp={canDetachWindows ? navPickUpHandlers.knowledge : undefined}
              />
              {/* 收起态专属:展开态近期列表的高亮项就是回会话入口,不重复渲染 */}
              {!isSidebarOpen && (
                <NavItem
                  icon={NAV_ICON_CURRENT_CHAT} label={t.currentChat}
                  active={currentView === 'chat'}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={navNavigateHandlers.chat}
                />
              )}
              {isSidebarOpen && !isCompactShell && (
                <button
                  type="button"
                  data-testid="sidebar-primary-nav-collapse"
                  onClick={() => setSidebarNavCollapsedPersisted(true)}
                  title={t.sidebarNavCollapse}
                  className={`w-full h-7 px-4 flex items-center justify-between rounded-full text-[12px] transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                >
                  <span className="truncate">{t.sidebarNavCollapse}</span>
                  <ChevronDown size={14} className="shrink-0 rotate-180" />
                </button>
              )}
              </>
              )}
            </div>

            {/* Recents — 独立 flex-1 + overflow-y-auto,只在展开态显示。
                min-h-0 关键:flex 子项默认 min-height: auto 会阻止 overflow,
                显式压成 0 才允许内容溢出触发滚动条。
                nav / list 分隔:「近期」label sticky top-0 + 实色背景,滚动时常驻顶端
                遮住下滑的列表项,避免首项与上方 nav 贴死("重合")。 */}
            {isSidebarOpen && (
              <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-3 flex flex-col">
                <div data-testid="sidebar-recents" className="pt-5 pb-2 max-sm:pt-2">
                  <div ref={taskFilterRef} className="relative mb-2">
                    {/* 第一行:「任务列表」标题 + 查看全部/筛选按钮;
                        第二行:全部/代码 胶囊 + 一键折叠(分组)按钮。
                        胶囊选择任务列表展示形态(标准列表 / code 样式按文件夹分组),
                        与是否处于 code 模式无关;折叠按钮切换下方任务分组
                        (日期组 / 文件夹组)的整体展开状态。 */}
                    <div className={`group flex flex-col gap-1 rounded-2xl text-[13px] font-semibold ${
                      activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'
                    }`}>
                      <div className="flex items-center justify-between gap-2">
                        <span className="h-8 px-1 min-w-0 truncate flex items-center">
                          {t.sidebarTaskList} ({sidebarCodeListActive ? sidebarCodeTasks.length : sidebarTaskHistory.length})
                        </span>
                        <span className="flex items-center shrink-0">
                        {/* 对话管理页入口:悬停任务列表行显现(触屏常显),替代原搜索入口 */}
                        <button
                          type="button"
                          onClick={() => navigateFromScheduledRun('search')}
                          className={`ml-1 h-6 px-2 shrink-0 rounded-full text-[12px] font-normal transition-opacity opacity-0 group-hover:opacity-100 max-sm:opacity-100 ${activeTheme === 'dark' ? 'text-[#A8C7FA] hover:bg-[#282A2C]' : 'text-[#0B57D0] hover:bg-[#E1E5EA]'}`}
                        >
                          {t.sidebarViewAll}
                        </button>
                        <button
                          type="button"
                          data-testid="sidebar-task-filter"
                          onClick={() => setTaskFilterOpen(v => !v)}
                          title={t.sidebarTaskFilter}
                          className={`w-7 h-7 -mr-2 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                            taskFilterOpen
                              ? (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#444746]')
                              : (activeTheme === 'dark' ? 'hover:bg-[#282A2C]' : 'hover:bg-[#E1E5EA]')
                          }`}
                        >
                          <Filter size={15} />
                        </button>
                        </span>
                      </div>
                      {/* 全部/代码 胶囊 + 一键折叠(分组)按钮:位于「任务列表」标题下方。
                          flex-wrap 兜底:ja 等语言在 220px 最小宽度下此行已无富余
                          (实测正好占满),字体渲染偏宽的环境让折叠按钮换行而非溢出。 */}
                      <div className="flex flex-wrap items-center justify-between gap-2 px-1">
                        {/* biome-ignore lint/a11y/useSemanticElements: toggle-button pair in an ARIA group, not form controls; a <fieldset> would need its default styles reset */}
                        <div className="flex items-center gap-0.5" role="group" aria-label={t.sidebarTaskStyle}>
                        <button
                          type="button"
                          data-testid="sidebar-task-pill-all"
                          aria-pressed={!sidebarCodeListActive}
                          onClick={() => { setSidebarCodeStylePersisted('normal'); setTaskFilterOpen(false); }}
                          className={`h-6 px-2.5 rounded-full text-[12px] font-normal transition-colors ${
                            sidebarCodeListActive
                              ? (activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]')
                              : (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#0B57D0]')
                          }`}
                        >
                          {t.sidebarTaskFilterAll}
                        </button>
                        <button
                          type="button"
                          data-testid="sidebar-task-pill-code"
                          aria-pressed={sidebarCodeListActive}
                          onClick={() => { setSidebarCodeStylePersisted('code'); setTaskFilterOpen(false); }}
                          className={`h-6 px-2.5 rounded-full text-[12px] font-normal transition-colors ${
                            sidebarCodeListActive
                              ? (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#0B57D0]')
                              : (activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]')
                          }`}
                        >
                          {t.sidebarTaskFilterCode}
                        </button>
                        </div>
                        {/* 一键折叠/展开全部任务分组(日期组或文件夹组);
                            无可分组列表(平铺/空列表)时不渲染,避免空操作 */}
                        {visibleTaskGroupOpens.length > 0 && (
                        <button
                          type="button"
                          data-testid="sidebar-collapse-all-groups"
                          onClick={() => setAllTaskGroups(!allTaskGroupsExpanded)}
                          title={allTaskGroupsExpanded ? t.sidebarCollapseAll : t.sidebarExpandAll}
                          aria-label={allTaskGroupsExpanded ? t.sidebarCollapseAll : t.sidebarExpandAll}
                          className={`h-6 px-2 shrink-0 whitespace-nowrap rounded-full text-[12px] font-normal transition-colors ${
                            activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'
                          }`}
                        >
                          <ChevronDown size={13} className={`inline -mt-0.5 transition-transform ${allTaskGroupsExpanded ? '' : 'rotate-180'}`} />
                          {allTaskGroupsExpanded ? t.sidebarCollapseAll : t.sidebarExpandAll}
                        </button>
                        )}
                      </div>
                    </div>
                    {taskFilterOpen && (
                      <div
                        data-testid="sidebar-task-filter-menu"
                        className={`absolute right-0 top-16 z-50 w-44 overflow-hidden rounded-2xl border p-1.5 shadow-xl ${
                          activeTheme === 'dark' ? 'border-white/10 bg-[#202124]' : 'border-black/10 bg-white'
                        }`}
                      >
                        <div className={`px-2.5 pb-1 pt-1 text-[11px] font-semibold ${activeTheme === 'dark' ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>
                          {t.sidebarTaskFilter}
                        </div>
                        {sidebarTaskFilterOptions.map(option => (
                          <button
                            key={option.id}
                            type="button"
                            onClick={() => setTaskListFilter(option.id)}
                            className={`w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`}
                          >
                            <span className="w-4 shrink-0">{taskListFilter === option.id && <Check size={13} />}</span>
                            <span className="truncate">{option.label}</span>
                          </button>
                        ))}
                        <div className={`my-1 h-px ${activeTheme === 'dark' ? 'bg-white/10' : 'bg-black/10'}`} />
                        <div className={`px-2.5 pb-1 pt-1 text-[11px] font-semibold ${activeTheme === 'dark' ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>
                          {t.sidebarTaskSort}
                        </div>
                        {sidebarTaskSortOptions.map(option => (
                          <button
                            key={option.id}
                            type="button"
                            onClick={() => setTaskListSort(option.id)}
                            className={`w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`}
                          >
                            <span className="w-4 shrink-0">{taskListSort === option.id && <Check size={13} />}</span>
                            <span className="truncate">{option.label}</span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="space-y-1">
                    {sidebarCodeListActive ? (
                      (sidebarFolderPinned.length > 0 || sidebarFolderGroups.length > 0) ? (
                        <>
                          {sidebarFolderPinned.length > 0 && (
                            <div className="space-y-0.5">
                              {sidebarFolderPinned.map(renderSidebarTaskItem)}
                            </div>
                          )}
                          {sidebarFolderGroups.map((group) => {
                            const isOpen = folderGroupOpen[group.key] ?? true;
                            const label = group.kind === 'project'
                              ? group.name
                              : group.kind === 'temporary'
                                ? t.uiCodex.temporarySession
                                : workspaceDisplayName(group.path);
                            return (
                              <div key={group.key}>
                                <ProjectGroupHeader
                                  label={label}
                                  kind={group.kind}
                                  count={group.rows.length + (sidebarGroupPinnedCounts[group.key] || 0)}
                                  isOpen={isOpen}
                                  onToggle={() => setFolderGroupOpen(prev => ({ ...prev, [group.key]: !isOpen }))}
                                  theme={activeTheme}
                                  t={t}
                                  busy={projectOpsBusy}
                                  testId="sidebar-folder-group"
                                  {...sidebarGroupHeaderProps(group)}
                                  dropActive={dropTargetGroupKey === group.key}
                                  onDropActive={(active) => setDropTargetGroupKey(active ? group.key : null)}
                                />
                                {isOpen && (
                                  <div className="mt-1 space-y-0.5">
                                    {group.rows.map(renderSidebarTaskItem)}
                                  </div>
                                )}
                              </div>
                            );
                          })}
                        </>
                      ) : (
                        sidebarTaskEmptyNode
                      )
                    ) : sidebarDateGrouping ? (sidebarPinnedHoisted.length > 0 || sidebarTaskGroups.length > 0) ? (
                      <>
                        {sidebarPinnedHoisted.length > 0 && (
                          <div className="space-y-0.5">
                            {sidebarPinnedHoisted.map(renderSidebarTaskItem)}
                          </div>
                        )}
                        {sidebarTaskGroups.map((group) => {
                      const isOpen = dateGroupOpen[group.key] ?? (group.key === todayDateKey);
                      return (
                        <div key={group.key}>
                          <button
                            type="button"
                            onClick={() => setDateGroupOpen(prev => ({ ...prev, [group.key]: !isOpen }))}
                            className={`w-full h-7 px-4 flex items-center justify-between rounded-full text-[12px] transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                          >
                            <span className="truncate">{formatDateGroupLabel(group.key, language)} ({group.rows.length})</span>
                            <ChevronDown size={14} className={`shrink-0 transition-transform ${isOpen ? '' : '-rotate-90'}`} />
                          </button>
                          {isOpen && (
                            <div className="mt-1 space-y-0.5">
                              {group.rows.map(renderSidebarTaskItem)}
                            </div>
                          )}
                        </div>
                      );
                        })}
                      </>
                    ) : (
                      sidebarTaskEmptyNode
                    ) : (
                      <div className="space-y-0.5">
                        {sidebarTaskHistory.length > 0 ? sidebarTaskHistory.map(renderSidebarTaskItem) : (
                          sidebarTaskEmptyNode
                        )}
                      </div>
                    )}
                  </div>
                </div>
              </div>
            )}

            {/* Footer Profile */}
            <div className={`p-3 mt-auto ${isSidebarOpen ? 'space-y-2' : 'flex flex-col items-center gap-3 pb-6'}`}>
              <div className={`${isSidebarOpen ? 'flex items-center justify-between gap-2' : 'flex flex-col items-center gap-3'}`}>
                {!isSidebarOpen && (
                  <>
                    {can('webAccessAdmin') && <button type="button"
                      onClick={handleOpenWebAccess}
                      title={t.uiRemote.title}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Smartphone size={18} />
                      {isWebAccessConnected && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                    </button>}
                    {can('pet') && <button type="button"
                      onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                      title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? t.uiPet.hide : t.uiMainApp.petSummon}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                    >
                      <PetPawIcon />
                    </button>}
                    <button type="button"
                      data-testid="nav-settings"
                      onClick={() => openSettingsSection('general')}
                      title={t.settings}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Settings size={18} />
                      {hasUpdate && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
                    </button>
                  </>
                )}
                {isSidebarOpen && (
                  <div className="flex items-center gap-1">
                    {can('webAccessAdmin') && <button type="button"
                      onClick={handleOpenWebAccess}
                      title={t.uiRemote.title}
                      className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Smartphone size={18} />
                      {isWebAccessConnected && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                    </button>}
                    {can('pet') && <button type="button"
                      onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                      title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? t.uiPet.hide : t.uiMainApp.petSummon}
                      className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#C4C7C5]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                    >
                      <PetPawIcon />
                    </button>}
                    <button type="button"
                      data-testid="nav-settings"
                      onClick={() => navigateFromScheduledRun('settings')}
                      title={t.settings}
                      className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Settings size={18} />
                      {hasUpdate && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
                    </button>
                  </div>
                )}
              </div>
            </div>

            {/* Right-edge drag to resize: only offered on the expanded desktop shell;
                double-click resets to the default width. Focusable separator semantics
                (tabIndex + value range + arrow keys) per the WAI-ARIA Window Splitter
                pattern; the controlled pane is the sidebar itself. */}
            {isSidebarOpen && !isCompactShell && (
              <hr
                data-testid="sidebar-resize-handle"
                aria-orientation="vertical"
                tabIndex={0}
                aria-valuenow={sidebarWidth}
                aria-valuemin={SIDEBAR_WIDTH_MIN}
                aria-valuemax={SIDEBAR_WIDTH_MAX}
                aria-label={t.sidebarResize}
                aria-controls="app-sidebar"
                title={t.sidebarResize}
                onPointerDown={beginSidebarResize}
                onDoubleClick={resetSidebarWidth}
                onKeyDown={keyboardSidebarResize}
                className={`absolute top-0 bottom-0 right-0 w-[6px] border-0 cursor-col-resize z-50 touch-none transition-colors focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[#0B57D0] ${
                  sidebarResizing
                    ? 'bg-[#0B57D0]/40'
                    : (activeTheme === 'dark' ? 'hover:bg-[#A8C7FA]/30' : 'hover:bg-[#0B57D0]/25')
                }`}
              />
            )}
          </div>

          {/* ================= Main Content ================= */}
          <div className={`flex-1 flex relative min-w-0 overflow-hidden ${activeTheme === 'dark' ? 'bg-[#131314]' : 'bg-white'} ${isCompactShell ? '' : 'rounded-tl-[28px]'}`}>
            <div className="relative flex min-w-0 flex-1 flex-col overflow-hidden">

            {/* Gemini Style Background Glow */}
            {(currentView === 'chat'
              || currentView === 'codex'
              || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && (
              activeTheme === 'light' ? (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[1200px] h-[800px] bg-[radial-gradient(ellipse_at_center,_rgba(232,240,254,0.8)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              ) : (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-[40%] w-[1400px] h-[900px] bg-[radial-gradient(ellipse_at_center,_rgba(168,199,250,0.25)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              )
            )}

            {/* 单一常驻 Suspense 边界:React 19 下切视图时旧视图在本边界内被替换,
                chunk 未就绪(未被预取覆盖的入口)保持旧视图不闪 fallback;失败由
                ViewErrorBoundary 兜底(reload 重试绕开 React.lazy 的失败缓存)。 */}
            <ViewErrorBoundary t={t}>
              <Suspense fallback={<ViewFallback />}>
            {currentView === 'monitor' && <LazyMonitorView theme={activeTheme} t={t} bs={bs} />}
            {currentView === 'settings' && (
              <ViewErrorBoundary heading={t.uiSettingsDetail.settingsLoadFailed} t={t}>
                <LazySettingsView
                  activeTheme={activeTheme} colorScheme={colorScheme} onColorSchemeChange={handleSetTheme}
                  language={language} setLanguage={handleSetLanguage}
                  superPerm={superPerm} setSuperPerm={handleToggleSuperPerm}
                  taskCompletedNotif={taskCompletedNotif} setTaskCompletedNotif={handleSetTaskCompletedNotif}
                  searchProvider={searchProvider} setSearchProvider={handleSetSearchProvider}
                  enabledSearchProviders={enabledSearchProviders}
                  onAddSearchProvider={handleAddSearchProvider}
                  onDeleteSearchProvider={handleDeleteSearchProvider}
                  setSearchApiKey={handleSetSearchApiKey}
                  savedModels={(bs && bs.savedModels) || []}
                  activeModelId={bs && bs.activeModelId}
                  onSaveModel={(m) => bridge.available && bridge.models.saveModel(m)}
                  onDeleteModel={(m) => { if (bridge.available) bridge.models.deleteModel(m.id); }}
                  onSetActiveModel={(id) => bridge.available && bridge.models.setActiveModel(id)}
                  onSaveSearchConfig={handleSaveSearchConfig}
                  onConfirmSearchConfig={handleConfirmSearchConfig}
                  onMemoryEnabledChange={handleSetMemoryEnabled}
                  onPetEnabledChange={handleSetPetEnabled}
                  bs={bs}
                  t={t}
                  sidebarDateGrouping={sidebarDateGrouping}
                  onSidebarDateGroupingChange={handleSetSidebarDateGrouping}
                  updateFocusTick={settingsUpdateFocusTick}
                  initialSection={settingsInitialSection}
                  onCloseSettings={() => navigateFromScheduledRun(settingsReturnViewRef.current || 'chat')}
                />
              </ViewErrorBoundary>
            )}
            {isCompactShell && browserActive && currentView === 'browser' && (
              <BrowserView
                key={browserViewSessionId}
                theme={activeTheme}
                t={t}
                sessionId={browserViewSessionId}
                nativeSurfaceSuspended={compactBrowserSurfaceSuspended}
              />
            )}
            {currentView === 'toolStore' && <LazyToolStoreView t={t} onNewChat={handleNewChat} />}
            {currentView === 'cardpool' && <LazyCardPoolView theme={activeTheme} t={t} bs={bs} onAICreate={startAICard} initialMyOnly={poolMyOnly} />}
            {currentView === 'chat' && (
              <ChatView
                {...chatViewBaseProps}
                prefill={chatPrefill}
                prefillAppend={chatPrefillAppend}
                focusComposerTick={petFocusComposerTick}
                onPrefillConsumed={() => { setChatPrefill(''); setChatPrefillAppend(false); }}
                onBackScheduledRun={() => navigateFromScheduledRun('scheduled')}
                codeModeAvailable={codexAcpSupported}
                onSwitchHomeMode={handleSwitchHomeMode}
                browserDockAvailable={browserDockAvailable}
                rightDockActivePanelId={browserDockSelectedPanelId}
                onRightDockPanelSelectionChange={selectRightDockPanel}
              />
            )}
            {codexAcpSupported && currentView === 'codex' && (
              <CodexAcpView
                theme={activeTheme}
                t={t}
                sessions={codexSessions}
                activeId={activeCodexId}
                draftEpoch={codexDraftEpoch}
                onActiveSessionChange={updateActiveCodexSession}
                onSessionsChange={setCodexSessions}
                onSwitchHomeMode={handleSwitchHomeMode}
                onOpenSettingsSection={openSettingsSection}
                onOpenWorkspacePicker={({ mode }) => { setPickerExcluded(null); setWorkspacePicker({ lane: 'codex', mode }); }}
                workspacePickerRequest={pickerCodexRequest}
                onWorkspacePickerRequestConsumed={() => setPickerCodexRequest(null)}
                onLaneModeChange={setCodexLaneMode}
                bs={bs}
                onGotoModelSettings={() => openSettingsSection('model')}
                onGotoSettings={() => openSettingsSection('general')}
                onGotoTools={() => navigateFromScheduledRun('toolStore')}
                onNotify={setSettingsToast}
              />
            )}
            {currentView === 'scheduled' && (
              bs && bs.scheduledRunContext ? (
                <ChatView {...chatViewBaseProps} prefill="" onPrefillConsumed={() => {}} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} browserDockAvailable={browserDockAvailable} rightDockActivePanelId={browserDockSelectedPanelId} onRightDockPanelSelectionChange={selectRightDockPanel} />
              ) : (
                <LazyScheduledTasksView theme={activeTheme} t={t} onOpenChat={() => { setCodeModeOn(false); setCurrentView('chat'); }} onGotoModelSettings={() => openSettingsSection('model')} />
              )
            )}
            {/* 草稿态(无 session)也渲染挂件,但强制空态——让欢迎页保留「＋加持卡牌」入口。
                点它跳卡牌池,选卡时 equipPersona 会先物化 session(lazy session)。 */}
            {(currentView === 'chat' || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && bs && (
              <Lanyard persona={bs.activeSessionId ? (bs.activePersona || null) : null} isDark={activeTheme === 'dark'} t={t}
                onRemove={() => bridge.available && bridge.personas.unequipPersona()}
                onOpenPicker={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))} />
            )}
            {currentView === 'search' && (
              <LazySearchView
                theme={activeTheme} history={allSidebarTasks} t={t} language={language}
                archived={(bs && bs.archivedSessions) || []}
                showArchived={searchShowArchived}
                onShowArchivedConsumed={() => setSearchShowArchived(false)}
                onSelect={handleSwitchSession}
                onOpenCodex={handleSwitchCodexSession}
                onOpenScheduledRun={handleOpenScheduledRunShortcut}
                onRename={handleRenameSession}
                onDelete={handleDeleteSession}
                onTogglePinned={handleToggleSessionPinned}
                onOpenFolder={can('externalSystemOpen') ? handleRevealSessionFolder : undefined}
                onExportArchive={bridge.sessions.exportSessionArchive ? handleExportSessionArchive : undefined}
                onArchive={handleArchiveSession}
                onArchiveMany={handleBatchArchiveSessions}
                onDeleteMany={handleBatchDeleteSessions}
                onRestoreArchived={handleRestoreArchivedSession}
                onRestoreMany={handleBatchRestoreArchived}
              />
            )}
            {currentView === 'outputs' && <LazyKnowledgeView theme={activeTheme} t={t} mode="outputs" />}
            {currentView === 'knowledge' && <LazyKnowledgeView theme={activeTheme} t={t} />}
              </Suspense>
            </ViewErrorBoundary>

            {can('webAccessAdmin') && webAccessOpen && browserOverlayPublicationReady && (
              <ViewErrorBoundary t={t}>
              <Suspense fallback={null}>
              <LazyWebAccessModal bs={bs} t={t} onClose={() => setWebAccessOpen(false)} />
            </Suspense>
            </ViewErrorBoundary>
            )}

            {/* App 级自创卡编辑器: 聊天里「存入卡牌池」草稿走这条。错误边界与
                WebAccessModal 同款:lazy chunk 拉取失败不能卸载整个应用窗口。 */}
            {personaEditor && browserOverlayPublicationReady && (
              <ViewErrorBoundary t={t}>
              <Suspense fallback={null}>
              <LazyPersonaEditorModal initial={personaEditor.initial} t={t}
                onClose={() => setPersonaEditor(null)}
                onSaved={(sum) => { const isEdit = personaEditor.initial && personaEditor.initial.id; setPersonaEditor(null); if (!isEdit) setSavedConfirm({ name: sum && sum.name }); }}
                onDeleted={() => setPersonaEditor(null)} />
              </Suspense>
              </ViewErrorBoundary>
            )}

            {/* 存入成功 → iOS 确认窗:去查看我的卡牌 / 暂不 */}
            {savedConfirm && browserOverlayPublicationReady && (
              // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users close the dialog through its real buttons
              // biome-ignore lint/a11y/noStaticElementInteractions: this is a pointer-only backdrop around an accessible dialog card
              <div className="fixed inset-0 z-[80] flex items-center justify-center p-4" style={{ background:'rgba(0,0,0,.4)' }} onClick={() => setSavedConfirm(null)}>
                {/* biome-ignore lint/a11y/useKeyWithClickEvents: background click-to-close layer; keyboard path handled by real buttons inside the card */}
                {/* biome-ignore lint/a11y/noStaticElementInteractions: background click-to-close layer; non-interactive container */}
                <div onClick={(e) => e.stopPropagation()} className="w-[270px] rounded-[14px] overflow-hidden text-center"
                  style={{ background: activeTheme === 'dark' ? 'rgba(44,44,46,.95)' : 'rgba(250,250,250,.95)', backdropFilter:'blur(20px)', WebkitBackdropFilter:'blur(20px)', fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  <div className="px-4 pt-5 pb-4">
                    <div className="text-[17px] font-semibold" style={{ color: activeTheme === 'dark' ? '#fff' : '#000' }}>{t.cpSavedTitle}</div>
                    <div className="text-[13px] mt-1.5" style={{ color: activeTheme === 'dark' ? 'rgba(235,235,245,.6)' : 'rgba(60,60,67,.6)' }}>{t.cpSavedDesc(savedConfirm.name || '')}</div>
                  </div>
                  <div className="flex" style={{ borderTop: '0.5px solid ' + (activeTheme === 'dark' ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)') }}>
                    <button type="button" onClick={() => setSavedConfirm(null)} className="flex-1 h-11 text-[17px]" style={{ color: activeTheme === 'dark' ? '#0A84FF' : '#007AFF' }}>{t.cpSavedLater}</button>
                    <div style={{ width:'0.5px', background: activeTheme === 'dark' ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)' }} />
                    <button type="button" onClick={() => { setPoolMyOnly(true); setSavedConfirm(null); setCurrentView('cardpool'); }} className="flex-1 h-11 text-[17px] font-semibold" style={{ color: activeTheme === 'dark' ? '#0A84FF' : '#007AFF' }}>{t.cpSavedView}</button>
                  </div>
                </div>
              </div>
            )}

            {/* API Key 拦截遮罩 —— 云端模型未配 key 时只盖住聊天界面,强制先配置。
                根因:此前前后端都无 key gate,空 key 打云端 → 401 静默无回应。
                设置页必须保持可操作,否则“去配置”后遮罩仍在,用户反而无法录入 Key。
                条件:credential_state 为 missing 或 unavailable 且非本地模型。本地 vLLM
                和 loopback OpenAI-compatible 端点允许无鉴权。unavailable 同样需拦截:macOS 上用户在 Keychain
                授权弹窗点"拒绝"时 credential_state 变 unavailable(见 prefs.rs:785),
                此时不盖遮罩用户仍可发消息 → 命中 Keychain 错误,与 missing 同等后果。 */}
            {apiKeyGateOpen && browserOverlayPublicationReady && (
              <div className="fixed inset-0 z-[57] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}>
                <div className="w-full max-w-[400px] rounded-2xl p-6 ts-modal-in"
                     style={{ background: activeTheme === 'dark' ? '#1E1F20' : '#FFFFFF', color: activeTheme === 'dark' ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
                  <div className="flex items-center gap-2 mb-3">
                    <PinvouLogo className="h-[22px] w-[22px] select-none" />
                    <div className="text-[17px] font-semibold">{t.apiKeyGateTitle}</div>
                  </div>
                  <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.apiKeyGateDesc}</div>
                  <div className="flex justify-end">
                    <button type="button" onClick={() => openSettingsSection('model')}
                      className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.apiKeyGateBtn}</button>
                  </div>
                </div>
              </div>
            )}

            {/* 厂商预装本地大模型一键引导 —— 全局首屏弹窗;引导中禁止背景关窗 */}
            {vllmSetupModalOpen && browserOverlayPublicationReady && (
              // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users close the dialog through its real buttons
              // biome-ignore lint/a11y/noStaticElementInteractions: this is a pointer-only backdrop around an accessible dialog card
              <div className="fixed inset-0 z-[56] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}
                   onClick={() => { if (!bs.vllmBootstrapping) bridge.vllm.dismissVllmSetup(); }}>
                {/* biome-ignore lint/a11y/useKeyWithClickEvents: background click-to-close layer; keyboard path handled by real buttons inside the dialog */}
                {/* biome-ignore lint/a11y/noStaticElementInteractions: background click-to-close layer; non-interactive container */}
                <div className="w-full max-w-[440px] rounded-2xl p-6 ts-modal-in" onClick={(e) => e.stopPropagation()}
                     style={{ background: activeTheme === 'dark' ? '#1E1F20' : '#FFFFFF', color: activeTheme === 'dark' ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
                  <div className="flex items-center gap-2 mb-3">
                    <PinvouLogo className="h-[22px] w-[22px] select-none" />
                    <div className="text-[17px] font-semibold">{vllmDeclineConfirm && !bs.vllmBootstrapping && !bs.vllmBootstrapDone && !bs.vllmBootstrapError ? t.vllmDeclineTitle : t.vllmSetupTitle}</div>
                  </div>
                  {bs.vllmBootstrapping ? (
                    <VllmSetupProgress phase={bs.vllmSetupPhase} attempt={bs.vllmSetupAttempt} isDark={activeTheme === 'dark'} t={t} />
                  ) : bs.vllmBootstrapDone ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4">{t.vllmSetupDone}</div>
                      <div className="flex justify-end">
                        <button type="button" onClick={() => bridge.available && bridge.updater.restartApp()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.restartNow}</button>
                      </div>
                    </div>
                  ) : bs.vllmBootstrapError ? (
                    <div>
                      <div className="text-[14px] font-medium mb-1" style={{ color: '#E5484D' }}>{t.vllmSetupFailed}</div>
                      <div className="text-[13px] leading-relaxed mb-4 break-words" style={{ opacity: .75 }}>{bs.vllmBootstrapError}</div>
                      <div className="flex justify-end gap-2">
                        <button type="button" onClick={() => bridge.vllm.dismissVllmSetup()}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                        <button type="button" onClick={() => bridge.vllm.bootstrapLocalVllm()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupRetry}</button>
                      </div>
                    </div>
                  ) : vllmDeclineConfirm ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.vllmDeclineDesc}</div>
                      <div className="flex justify-end gap-2">
                        <button type="button" onClick={() => setVllmDeclineConfirm(false)}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmDeclineReconsider}</button>
                        <button type="button" onClick={() => { setVllmDeclineConfirm(false); bridge.vllm.declineVllmSetup(); }}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#E5484D' }}>{t.vllmDeclineConfirm}</button>
                      </div>
                    </div>
                  ) : (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.vllmSetupDesc}</div>
                      <div className="flex items-center justify-between gap-2">
                        <button type="button" onClick={() => setVllmDeclineConfirm(true)}
                          className="h-9 px-3 rounded-lg text-[13px] hover:underline" style={{ color: activeTheme === 'dark' ? '#8E8E8E' : '#757575' }}>{t.vllmSetupNever}</button>
                        <div className="flex gap-2">
                          <button type="button" onClick={() => bridge.vllm.dismissVllmSetup()}
                            className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                          <button type="button" onClick={() => bridge.vllm.bootstrapLocalVllm()}
                            className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupEnable}</button>
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* Pinvou 检阅弹窗(品/悟) —— 居中弹窗 + 毛玻璃背景(虚化身后 app);全局,任何视图都能弹;点背景或卡内「跳过」关闭 */}
            {bs && bs.pinvouModal && browserOverlayPublicationReady && (
              // biome-ignore lint/a11y/useKeyWithClickEvents: keyboard users close the dialog through its real close button
              // biome-ignore lint/a11y/noStaticElementInteractions: this is a pointer-only backdrop around an accessible dialog card
              <div className="fixed inset-0 z-[55] flex items-center justify-center p-6"
                   style={{ background: activeTheme === 'dark' ? 'rgba(0,0,0,.45)' : 'rgba(255,255,255,.35)', backdropFilter: 'blur(20px) saturate(140%)', WebkitBackdropFilter: 'blur(20px) saturate(140%)' }}
                   onClick={() => { if (!bs.pinvouModal.loading) bridge.interaction.dismissPinvouReview(); }}>
                {/* loading 期间禁止背景点击关窗:召唤(直连 vLLM,5-30s)仍在后台跑、守卫仍 held,
                    点背景误关会表现为"闪一下没反应、要等一会才能再点"。锁住后 spinner 全程可见,
                    出结果/错误后才可点背景关。 */}
                {/* biome-ignore lint/a11y/useKeyWithClickEvents: background click-to-close layer; keyboard path handled by the top-right close button (a real button below) */}
                {/* biome-ignore lint/a11y/noStaticElementInteractions: background click-to-close layer; non-interactive container */}
                <div className="relative w-full max-w-[720px] overflow-hidden bg-white dark:bg-[#1C1C1E] rounded-[20px] shadow-[0_20px_60px_rgba(0,0,0,0.28)] ts-modal-in"
                     onClick={(e) => e.stopPropagation()}
                     style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  {/* 关闭按钮：所有状态(含 loading)常驻;loading 时点它=取消等待并关窗,in-flight 结果由守卫丢弃 */}
                  <button type="button" onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} aria-label={t.pvSkip}
                    className="absolute top-3.5 right-3.5 z-10 w-7 h-7 flex items-center justify-center rounded-full bg-black/[0.06] dark:bg-white/10 text-[#8E8E93] hover:bg-black/10 dark:hover:bg-white/15 active:scale-90 transition-colors">
                    <X size={16} />
                  </button>
                  <div className="max-h-[90vh] overflow-y-auto custom-scrollbar px-5 pt-5 pb-6">
                    <PinvouSummonCard item={bs.pinvouModal} theme={activeTheme} t={t} isLocal={activeModelIsLocal(bs)} />
                  </div>
                </div>
              </div>
            )}

            </div>
            {browserDockAvailable && browserPaneOpen
              && (browserActive || browserWorkspaceStarting || browserWorkspaceError) && (
              <RightDockPanel
                panelId="browser"
                visible={browserPaneAllowed && browserPaneSelected}
                activationKey={browserDockActivationKey}
                className={`overflow-hidden border-l ${
                  activeTheme === 'dark' ? 'border-[#2A2B2E] bg-[#101113]' : 'border-[#E5E7EB] bg-white'
                }`}
                dataTestId="browser-side-pane"
              >
                <div
                  className={`flex h-9 shrink-0 items-center justify-between border-b px-3 text-[13px] ${
                    activeTheme === 'dark' ? 'border-[#2A2B2E] text-[#E8E8E8]' : 'border-[#E5E7EB] text-[#222]'
                  }`}
                >
                  <span className="truncate">{t.browser}</span>
                  <div className="flex items-center gap-1">
                    <div
                      ref={setBrowserOwnershipSlot}
                      className="contents"
                      data-browser-control-slot="ownership"
                    />
                    <button
                      type="button"
                      className={`rounded p-1 ${activeTheme === 'dark' ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}
                      title={t.browserPaneClose}
                      aria-label={t.browserPaneClose}
                      onClick={() => {
                        const selectedSessionId = browserSessionIdRef.current;
                        closeBrowserDock(selectedSessionId);
                      }}
                    >
                      <XIcon size={15} />
                    </button>
                  </div>
                </div>
                <div className="min-h-0 flex-1">
                  {browserActive ? (
                    <BrowserView
                      key={browserViewSessionId}
                      theme={activeTheme}
                      t={t}
                      sessionId={browserViewSessionId}
                      nativeSurfaceSuspended={browserSurfaceSuspended}
                      ownershipSlot={browserOwnershipSlot}
                    />
                  ) : (
                    <div
                      className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-[13px]"
                      data-testid="browser-workspace-starting"
                      style={{ color: activeTheme === 'dark' ? '#B8B8B8' : '#555' }}
                    >
                      <Globe size={28} style={{ opacity: 0.45 }} />
                      <div>{browserWorkspaceStarting ? t.browserLoading : t.browserError}</div>
                      {browserWorkspaceError && (
                        <>
                          <div style={{ opacity: 0.7, wordBreak: 'break-word' }}>{browserWorkspaceError}</div>
                          <button
                            type="button"
                            onClick={openBrowserDock}
                            className={`rounded-full px-4 py-2 font-medium ${
                              activeTheme === 'dark'
                                ? 'bg-[#A8C7FA] text-[#062E6F] hover:bg-[#B8D2FA]'
                                : 'bg-[#0B57D0] text-white hover:bg-[#0842A0]'
                            }`}
                          >
                            {t.browserRetry}
                          </button>
                        </>
                      )}
                    </div>
                  )}
                </div>
              </RightDockPanel>
            )}
            <RightDockHost
              resizeLabel={t.uiMultiAgent.panelResize}
              resizeHint={t.uiMultiAgent.panelResizeHint}
              onResizeActiveChange={setBrowserResizeActive}
            />
          </div>
          </div>
          </RightDockProvider>
          </SidePanelLayoutProvider>

          {isCompactShell && (
            <MobileTabBar theme={activeTheme} tabs={[
              { key: 'chat', label: t.currentChat, icon: <MessageSquare size={18} />,
                active: currentView === 'chat' || !!(currentView === 'scheduled' && bs && bs.scheduledRunContext),
                onClick: () => mobileNavigate('chat') },
              { key: 'cardpool', label: t.cardPool, icon: <Layers size={18} />,
                active: currentView === 'cardpool', onClick: () => mobileNavigate('cardpool', () => setPoolMyOnly(false)) },
              { key: 'monitor', label: t.monitor, icon: <BarChart2 size={18} />,
                active: currentView === 'monitor',
                onClick: () => mobileNavigate('monitor', () => {
                  const liveBridge = window.TauriBridge || bridge;
                  if (liveBridge?.monitor && typeof liveBridge.monitor.startMonitorPolling === 'function') liveBridge.monitor.startMonitorPolling();
                }) },
              { key: 'more', label: t.mobileMore, icon: <MoreHorizontal size={18} />,
                active: mobileMoreActive, dot: hasUpdate || scheduledUnread,
                onClick: () => setMobileMoreOpen(true) },
            ]} />
          )}

          {isCompactShell && mobileMoreOpen && browserOverlayPublicationReady && (
            <MobileMoreSheet theme={activeTheme} title={t.mobileMore} onClose={() => setMobileMoreOpen(false)} items={[
              { key: 'search', label: t.searchChats, icon: <Search size={18} />,
                active: currentView === 'search', onClick: () => mobileNavigate('search') },
              ...(browserActive ? [{ key: 'browser', label: t.browser, icon: <Globe size={18} />,
                active: currentView === 'browser', onClick: () => mobileNavigate('browser') }] : []),
              { key: 'scheduled', label: t.scheduledPlans, icon: <Clock size={18} />,
                active: currentView === 'scheduled', dot: scheduledUnread,
                onClick: () => mobileNavigate('scheduled') },
              { key: 'outputs', label: t.outputs, icon: <Package size={18} />,
                active: currentView === 'outputs', onClick: () => mobileNavigate('outputs') },
              { key: 'knowledge', label: t.knowledge, icon: <BookOpen size={18} />,
                active: currentView === 'knowledge', onClick: () => mobileNavigate('knowledge') },
              { key: 'toolStore', label: t.toolStore, icon: <Puzzle size={18} />,
                active: currentView === 'toolStore', onClick: () => mobileNavigate('toolStore') },
              { key: 'settings', label: t.settings, icon: <Settings size={18} />,
                active: currentView === 'settings', dot: hasUpdate, onClick: () => mobileNavigate('settings') },
            ]} />
          )}

          <UpdateNoticeButton
            theme={activeTheme}
            bs={bs}
            t={t}
            onShowChangelog={() => {
              setSettingsInitialSection('update');
              setCurrentView('settings');
              setSettingsUpdateFocusTick(v => v + 1);
            }}
          />
        </div>
      );
    };

    // ==========================================
    // 长按撕离:按住 ~350ms 不动 → onPickUp(info)(DOM avatar 浮起跟手 + begin_detach_drag 原生判落点);
    // 长按达成前移动 >10px = 视为滚动/取消;长按达成后吞掉随之而来的 click(避免又切视图);
    // 按在内部按钮/输入框上不起手(让它们自理)。按下即禁选,防止长按选中下方文字。
    window.__PINVOU_STARTUP__.mark('react:create_root_start');
    const root = createRoot(document.querySelector('#root'));
    window.__PINVOU_STARTUP__.mark('react:create_root_done');
    const __q = new URLSearchParams(window.location.search);
    // 首帧语言引导:zh 词典内嵌(Promise 已 resolve,仅一个微任务),en/ja 系统
    // 用户先取惰性词典 chunk 再首渲染,保证 t = dict[language] 首帧即有效。
    // 装载失败(资源损坏)按 zh 兜底渲染,不空白。
    const __initialLang = initialSystemLanguage();
    const __storedLang = (() => {
      if (!isWeb) return null;
      try {
        const value = window.localStorage.getItem('pinvou.web.language');
        return value && ['zh', 'en', 'ja'].includes(value) ? value : null;
      } catch { return null; }
    })();
    ensureLanguage(__storedLang || __initialLang).catch(() => {}).then(function () {
      if (__q.get('detached') === '1') {
        window.__PINVOU_DETACHED__ = true;
        root.render(
          <Suspense fallback={<div className="p-6 text-sm opacity-60">…</div>}>
            <LazyDetachedShell kind={__q.get('kind') || 'monitor'} id={__q.get('id') || ''} />
          </Suspense>
        );
      } else {
        window.__PINVOU_STARTUP__.mark('react:render_call');
        root.render(<App />);
      }
    });
