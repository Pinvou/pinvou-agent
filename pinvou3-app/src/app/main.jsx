import { lazy, startTransition as scheduleViewTransition, Suspense, useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import { Edit2, BarChart2, Settings, Smartphone, Clock, Package, Search, ChevronDown, Menu, MoreHorizontal, Check, Filter, Layers, MessageSquare, X, XIcon, Globe, BookOpen, Puzzle, PetPawIcon, FolderOpen, IconList } from '../components/icons.jsx';
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
import { DEFAULT_CHAT_TITLES, dict, createLatestLanguageGate, ensureLanguage, LANG_TO_TAG, initialSystemLanguage, SEARCH_KEY_PROVIDERS, TAG_TO_LANG } from '../shared/i18n.js';
import { formatSessionDate, localDateKey, formatDateGroupLabel } from '../shared/date-utils.js';
import { TEMPORARY_GROUP_KEY, groupSessionsByFolder } from '../shared/sidebar-grouping.js';
import { runSessionBatch } from '../shared/session-management.js';
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
import { SettingsErrorBoundary } from '../features/settings/SettingsErrorBoundary.jsx';
import { ViewErrorBoundary } from '../shared/ViewErrorBoundary.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { createPinvouModeScopeKey, savePinvouModeState } from '../features/chat/pinvou-mode-state.js';
import { WebConnectionStatus } from '../features/web/WebConnectionStatus.jsx';
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

// 定时任务创建与运行链路已恢复，展示入口并允许自动跳转。
const SCHEDULED_TASKS_ENTRY_ENABLED = true;

// 后端默认会话标题哨兵集合(bridge 按当前语言生成三语兜底标题,并据此判断是否自动改名)——
// 显示层把任意一种哨兵标题映射成当前语言的「新对话」文案。哨兵是跨语言的后端
// 契约而非当前 UI 文案,直接使用 shared/i18n.js 的静态集合,与词典装载进度无关
// (zh 主用户不会装载 en/ja chunk,不能从 dict 派生)。
function isDefaultChatTitle(title) {
  return DEFAULT_CHAT_TITLES.has(title);
}
// Static regression anchor: SCHEDULED_TASKS_ENTRY_ENABLED && (<NavItem icon={<Clock size={18} />} label={t.scheduledPlans} unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))} />)
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
  'memory', 'remoteControl', 'updater', 'dependencies',
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

// 当前平台是否支持本地 vLLM。macOS/Windows 后端已 cfg 掉本地 vLLM 命令(discover_local_vllm /
// detect_local_vllm_setup 等),前端默认预设与探测入口都据此守卫,避免新用户首启落在
// 127.0.0.1:8000 永远连不上、或调用不存在的后端命令报错。与 bridge prefs::ModelPreset::default() 对齐。
function defaultModelPresetForCapabilities(capabilities) {
  return capabilities && capabilities.localVllmSupported ? 'local_vllm' : 'deepseek';
}

function workspaceDisplayName(path) {
  const parts = String(path || '').split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || String(path || '');
}

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
      const [activeTheme, setActiveTheme] = useState('dark');
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
      const showMegacubeSite = !!platformCapabilities.showMegacubeSite;
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
      // MegaCube(GB10) 首屏检测:仅启动一次,检测「预装但未启用」本地大模型环境(后端短路保证普通机零开销)。
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
      const [searchApiKey, setSearchApiKey] = useState('');
      const [searchKeyDrafts, setSearchKeyDrafts] = useState({});
      const [searchKeyActions, setSearchKeyActions] = useState({});
      // 模型配置（动态适配）——草稿模式，确认后才保存
      // 默认预设平台感知:macOS/Windows 无本地 vLLM(后端命令已 cfg 掉),默认 DeepSeek;
      // Linux 保持 local_vllm(麒麟环境默认有本地大模型)。与 bridge prefs::ModelPreset::default() 对齐。
      // Keep only the setter: draft values are currently managed inside the settings page; here we backfill once during startup bootstrap.
      const [, setModelPreset] = useState(() => defaultModelPresetForCapabilities(platformCapabilities));
      const [, setCustomModelName] = useState('');
      const [, setCustomBaseUrl] = useState('');
      const [, setCustomApiKey] = useState('');
      const [, setModelProfiles] = useState({});
      const modelConfigInitRef = useRef(false);
      const searchConfigInitRef = useRef(false);
      const uiPrefsInitRef = useRef(false);
      // engine 启动时生效的语言(= 进程启动时的 settings.language)。语言只写盘不重启
      // engine,LLM 的 locale_tag 要重启 app 才更新 —— 草稿偏离此基线就提示「需重启」。
      const bootedLanguageRef = useRef(null);
      // dirty 基线:已保存的模型配置(默认值填充后) / 已保存的搜索源配置。
      // 草稿偏离基线才显示「保存并重启」操作条。
      const savedModelConfigRef = useRef(null);
      const savedSearchConfigRef = useRef(null);

      // 各厂商默认配置（前端自动填充用，与 bridge/mod.rs 对齐）
      const PRESET_DEFAULTS = {
        local_vllm:  { baseUrl: 'http://127.0.0.1:8000/v1',                model: 'qwen36_35b_256k' },
        deepseek:    { baseUrl: 'https://api.deepseek.com',                model: 'deepseek-v4-pro' },
        kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k3' },
        openai_compatible: { baseUrl: 'https://api.openai.com/v1',        model: 'gpt-5.6-terra' },
        qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen3.7-plus' },
        doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seed-evolving' },
        minimax:     { baseUrl: 'https://api.minimaxi.com/v1',            model: 'MiniMax-M3' },
        glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-5.2' },
        mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.5-pro' },
      };
      function normalizedModelProfile(name, baseUrl, apiKey) {
        const modelName = (name || '').trim();
        const endpoint = (baseUrl || '').trim();
        const key = (apiKey || '').trim();
        return {
          model_name: modelName || null,
          base_url: endpoint || null,
          api_key: key || null,
        };
      }
      function modelDraftForPreset(preset, profiles, fallback) {
        const defs = PRESET_DEFAULTS[preset] || PRESET_DEFAULTS[defaultModelPresetForCapabilities(platformCapabilities)];
        const profile = (profiles && profiles[preset]) || {};
        return {
          preset,
          name: profile.model_name || (fallback && fallback.name) || defs.model,
          baseUrl: profile.base_url || (fallback && fallback.baseUrl) || defs.baseUrl,
          apiKey: profile.api_key || (fallback && fallback.apiKey) || '',
        };
      }
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
      const beginTearOff = (kind, id, label, info) => {
        const inv = isTauriAvailable() ? invokeTauri : null;
        if (!inv || !info) return;
        inv('begin_detach_drag', { kind, id: id == null ? null : id });
        dragOffsetRef.current = { dx: info.dx, dy: info.dy };
        setDragAvatar({
          key: kind + ':' + (id == null ? '' : id), label: label || kind,
          w: info.w, h: info.h, x: info.startX - info.dx, y: info.startY - info.dy,
        });
        if (window.getSelection) { const s = window.getSelection(); if (s && s.removeAllRanges) s.removeAllRanges(); }
      };
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
        let un;
        tauriEvents.listen('detach:drag-ended', () => setDragAvatar(null)).then(f => { un = f; });
        return () => { if (un) un(); };
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
      function handleOpenPersonaEditor(initial) {
        prefetchView('cardpool');
        setPersonaEditor({ initial });
      }

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
        setSearchApiKey(drafts[saved.provider] || '');
        setSearchKeyDrafts(drafts);
        setSearchKeyActions(actions);
        savedSearchConfigRef.current = saved;
        searchConfigInitRef.current = true;
      };
      // One-shot bootstrap: backfill the model-config draft baseline. When custom_* is null, fill real values from
      // PRESET_DEFAULTS — inputs show the effective config instead of a gray placeholder masquerading as one.
      const initModelConfigFromSettings = (settings, effectiveModelConfig) => {
        const adv = settings.advanced || {};
        const effective = effectiveModelConfig || {};
        const preset = effective.preset || adv.model_preset || defaultModelPresetForCapabilities(platformCapabilities);
        const profiles = { ...adv.model_profiles };
        const fallback = {
          name: effective.model || adv.custom_model_name || '',
          baseUrl: effective.base_url || adv.custom_base_url || '',
          apiKey: '',
        };
        const saved = modelDraftForPreset(preset, profiles, fallback);
        profiles[preset] = normalizedModelProfile(saved.name, saved.baseUrl, saved.apiKey);
        setModelProfiles(profiles);
        setModelPreset(saved.preset);
        setCustomModelName(saved.name);
        setCustomBaseUrl(saved.baseUrl);
        setCustomApiKey(saved.apiKey);
        savedModelConfigRef.current = saved;
        modelConfigInitRef.current = true;
      };
      // One-shot bootstrap: restore persisted UI language/theme and notification prefs (desktop); on Web the language uses local storage.
      const initUiPrefsFromSettings = (settings) => {
        if (isWeb) {
          bootedLanguageRef.current = language;
        } else {
          const lang = TAG_TO_LANG[settings.language];
          // 落盘语言可能尚未装载(en/ja 惰性 chunk);ensure 后再切,失败停在系统语言
          if (lang && lang !== language) ensureLanguage(lang).then((ok) => { if (ok) setLanguage(lang); }).catch(() => {});
          // engine 已用此语言启动,作为「需重启」基线(切语言不重启 engine,见 commands.rs)
          bootedLanguageRef.current = lang || language;
          // 后端 Theme 枚举(prefs.rs)只认 genesis/liquid-light/liquid-dark;深色=genesis,浅色=liquid-light
          const th = settings.theme === 'liquid-light' ? 'light' : 'dark';
          if (th !== activeTheme) setActiveTheme(th);
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
        if (SCHEDULED_TASKS_ENTRY_ENABLED && bs.scheduledTaskAutoOpenId && bs.scheduledTaskAutoOpenId !== scheduledTaskAutoOpenSeenRef.current) {
          scheduledTaskAutoOpenSeenRef.current = bs.scheduledTaskAutoOpenId;
          setCurrentView('scheduled');
        }
        // UI 语言/主题:启动时从落盘 settings 恢复一次；无语言配置时后端已按系统 locale 补齐。
        if (!uiPrefsInitRef.current && bs.settings) initUiPrefsFromSettings(bs.settings);
        // 搜索配置：只在第一次从后端加载初始值，后续走草稿模式（确认后才保存并重启）。
        if (!searchConfigInitRef.current && bs.settings) initSearchConfigFromSettings(bs.settings);
        // 模型配置：只在第一次从后端加载初始值，后续走草稿模式（确认后才保存），
        if (!modelConfigInitRef.current && bs.settings) initModelConfigFromSettings(bs.settings, bs.effectiveModelConfig);
        // The effect subscribes to the bridge snapshot bs; one-shot bootstrap/init flags are guarded by internal refs,
        // and the remaining deps (activeChat/currentView/language, etc.) are render-state reads — including them would
        // rerun the whole sync logic on every UI change. sessionSyncEpoch intentionally
        // retriggers reconciliation after the serialized browser session gate settles.
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [bs, sessionSyncEpoch]);

      // HMR or legacy frontend state may retain a retired route; return to a valid view.
      useEffect(() => {
        if (!SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled') {
          setCodeModeOn(false);
          setCurrentView('chat');
        }
      }, [currentView, setCurrentView]);

      function searchCredentialForProvider(provider) {
        const saved = savedSearchConfigRef.current;
        return (saved && saved.credentials && saved.credentials[provider]) || {};
      }
      function searchHasSavedKey(provider) {
        const credential = searchCredentialForProvider(provider);
        const state = credential.credential_state || (credential.has_secret ? 'configured' : 'missing');
        return !!credential.has_secret || state === 'configured' || state === 'env_override';
      }
      function searchProviderKeyAction(provider) {
        return searchKeyActions[provider] || 'keep_existing';
      }
      function searchProviderCredentialDirty(provider) {
        const action = searchProviderKeyAction(provider);
        const draft = searchKeyDrafts[provider] || '';
        return action === 'delete' || (action === 'replace' && !!draft.trim());
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
      // 搜索配置也影响 EngineConfig,需保存后重启进程才生效。
      const savedSearch = savedSearchConfigRef.current;
      const searchCredentialDirty = SEARCH_KEY_PROVIDERS.some(searchProviderCredentialDirty);
      const providerSetKey = (providers) => JSON.stringify([...new Set(providers)].sort((a, b) => a.localeCompare(b)));
      const searchNeedsRestart = !!savedSearch && (
        searchProvider !== savedSearch.provider ||
        providerSetKey(enabledSearchProviders) !== providerSetKey(savedSearch.enabledProviders || ['bing']) ||
        searchCredentialDirty
      );
      // 语言已即时写盘+切 UI,但 LLM 的 locale_tag 要重启 engine 才生效 → 偏离启动语言就提示。
      const languageNeedsRestart = !!bootedLanguageRef.current && language !== bootedLanguageRef.current;

      // Build chat history from sessions
      const sessionBusy = (bs && bs.sessionBusy) || {};
      const chatHistory = bs && bs.sessions ? bs.sessions.map(s => {
        const isPlaceholder = !s.title || isDefaultChatTitle(s.title);
        const titlePresentation = isPlaceholder
          ? { text: t.newChat, attachments: [] }
          : sessionTitlePresentation(s.title, s.title_attachment_names);
        return {
          id: s.id,
          // 后端默认标题是三语哨兵之一(见 isDefaultChatTitle;bridge 以此判断是否自动改名)——显示层映射成当前语言
          title: sessionTitlePlainText(titlePresentation),
          titleContent: titlePresentation.attachments.length
            ? <SessionAttachmentTitle presentation={titlePresentation} />
            : null,
          date: formatSessionDate(s.updated_at || s.created_at, language),
          updatedAt: s.updated_at || s.created_at || '',
          pinned: !!s.pinned,
          pinnedAt: s.pinned_at || '',
          working: !!sessionBusy[s.id], // 多 session 并发:该 session 是否正在后台生成
          leadingIcon: <PinvouLogo className="h-[18px] w-[18px]" />,
          testId: 'regular-sidebar-item',
          menuTestId: 'regular-sidebar-menu',
        };
      }) : [];
      const codexHistory = codexSessions.map(session => ({
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
      }));
      const pinnedChatHistory = chatHistory
        .filter(chat => chat.pinned)
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt)));
      const scheduledRunShortcuts = (bs && bs.scheduledTaskRecentRuns && bs.scheduledTaskRecentRuns.length)
        ? bs.scheduledTaskRecentRuns
        : (bridge.available ? [] : PREVIEW_SCHEDULED_RUN_SHORTCUTS.map(run => ({ ...run, taskName: t[run.taskNameKey] || run.taskNameKey })));
      const scheduledRunSessionIds = new Set(
        scheduledRunShortcuts
          .map(run => run && run.sessionId)
          .filter(Boolean)
      );
      const scheduledRunBySessionId = Object.create(null);
      scheduledRunShortcuts.forEach(run => {
        if (run && run.sessionId) scheduledRunBySessionId[run.sessionId] = run;
      });
      const regularHistory = chatHistory
        .filter(chat => !chat.pinned && !scheduledRunSessionIds.has(chat.id))
        .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)));
      const scheduledRunItems = scheduledRunShortcuts
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
            leadingIcon: (
              <span className="relative inline-flex h-5 w-5 items-center justify-center">
                <Clock size={18} />
                {run.unread && (
                  <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full border-2"
                    style={{ background: '#0B57D0', borderColor: activeTheme === 'dark' ? '#1E1F20' : '#F0F4F9' }} />
                )}
              </span>
            ),
            testId: 'scheduled-run-sidebar-item',
            menuTestId: 'scheduled-run-sidebar-menu',
            scheduledRun: run,
          };
        });
      const scheduledRunHistory = scheduledRunItems.filter(chat => !chat.pinned);
      const pinnedHistory = [...pinnedChatHistory, ...scheduledRunItems.filter(chat => chat.pinned)]
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt)));

      function decorateScheduledRunChat(chat, run) {
        if (!run) return chat;
        const title = (!chat.title || isDefaultChatTitle(chat.title))
          ? (run.taskName || t.scheduledPlans)
          : chat.title;
        return Object.assign({}, chat, {
          title,
          subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
          leadingIcon: (
            <span className="relative inline-flex h-5 w-5 items-center justify-center">
              <Clock size={18} />
              {run.unread && (
                <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full border-2"
                  style={{ background: '#0B57D0', borderColor: activeTheme === 'dark' ? '#1E1F20' : '#F0F4F9' }} />
              )}
            </span>
          ),
          testId: 'scheduled-run-sidebar-item',
          menuTestId: 'scheduled-run-sidebar-menu',
          scheduledRun: run,
        });
      }

      const [justInstalledTool, setJustInstalledTool] = useState(null);
      const [taskListFilter, setTaskListFilter] = useState('all');
      const [taskListSort, setTaskListSort] = useState('pinned_first');
      const [taskFilterOpen, setTaskFilterOpen] = useState(false);
      const taskFilterRef = useRef(null);
      // 日期组展开状态:未点过的组按默认值走(今天展开、以往折叠),点过后记住用户选择
      const [dateGroupOpen, setDateGroupOpen] = useState({});
      // Code-style sidebar: enabled by default in code mode (folder grouping +
      // collapsed primary nav); the bottom-right button switches back to the standard
      // style. The choice is persisted and survives re-entering code mode.
      const [sidebarCodeStyle, setSidebarCodeStyle] = useState(() => {
        try {
          return localStorage.getItem('pinvou_sidebar_code_style') === 'normal' ? 'normal' : 'code';
        } catch {
          return 'code';
        }
      });
      const toggleSidebarCodeStyle = useCallback(() => {
        setSidebarCodeStyle(prev => {
          const next = prev === 'code' ? 'normal' : 'code';
          try {
            localStorage.setItem('pinvou_sidebar_code_style', next);
          } catch {
            // When the WebView disables storage, still allow switching for this window.
          }
          return next;
        });
      }, []);
      // Folder group expand state: all expanded by default; once toggled, remember the choice
      const [folderGroupOpen, setFolderGroupOpen] = useState({});
      // In code style the primary nav collapses to a single row by default; expanding is
      // remembered for the session (reset when code mode exits)
      const [codeNavExpanded, setCodeNavExpanded] = useState(false);
      // Code mode is a mode, not a page: after entering, navigating to output/monitor
      // pages keeps code mode — the sidebar stays code-styled and New chat still creates
      // code sessions; only explicitly switching back to work/design, or opening a normal
      // chat session, exits it.
      const [codeModeOn, setCodeModeOn] = useState(false);
      const codeStyleActive = codeModeOn && sidebarCodeStyle === 'code';
      // Exiting code mode resets the primary-nav collapse bar, so the next entry starts
      // from the default collapsed form.
      useEffect(() => {
        if (!codeModeOn) setCodeNavExpanded(false);
      }, [codeModeOn]);
      const [archiveConfirm, setArchiveConfirm] = useState(null);
      const [archiveToast, setArchiveToast] = useState(false);
      const [settingsToast, setSettingsToast] = useState('');

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
        { id: 'code', label: t.sidebarTaskFilterCode },
        { id: 'scheduled', label: t.sidebarTaskFilterScheduled },
      ];
      const sidebarTaskSortOptions = [
        { id: 'pinned_first', label: t.sidebarTaskSortPinnedFirst },
        { id: 'recent', label: t.sidebarTaskSortRecent },
      ];
      const allSidebarTasks = [
        ...pinnedHistory.map((chat) => {
          const run = chat.scheduledRun || scheduledRunBySessionId[chat.id];
          const item = decorateScheduledRunChat(chat, run);
          return { ...item, taskKind: run ? 'scheduled' : 'regular' };
        }),
        ...regularHistory.map(chat => ({ ...chat, taskKind: 'regular' })),
        ...scheduledRunHistory.map(chat => ({ ...chat, taskKind: 'scheduled' })),
        ...codexHistory,
      ];
      const sidebarTaskHistory = allSidebarTasks
        .filter((chat) => {
          if (taskListFilter === 'pinned') return !!chat.pinned;
          if (taskListFilter === 'code') return chat.taskKind === 'codex';
          if (taskListFilter === 'scheduled') return chat.taskKind === 'scheduled';
          return true;
        })
        .sort((a, b) => {
          if (taskListSort === 'pinned_first' && !!a.pinned !== !!b.pinned) {
            return a.pinned ? -1 : 1;
          }
          const aTime = (taskListSort === 'pinned_first' && a.pinned)
            ? (a.pinnedAt || a.updatedAt)
            : (a.updatedAt || a.pinnedAt);
          const bTime = (taskListSort === 'pinned_first' && b.pinned)
            ? (b.pinnedAt || b.updatedAt)
            : (b.updatedAt || b.pinnedAt);
          return String(bTime || '').localeCompare(String(aTime || ''));
        });

      // 任务列表按日期堆叠:今天默认展开、以往默认折叠;组内顺序沿用上面的筛选+排序结果,
      // 组间按日期倒序,无时间戳的落 'unknown' 组沉底。
      // 「置顶优先」排序下置顶项提升到所有日期组之上,否则旧会话会埋进默认折叠的以往分组,
      // 只剩置顶标志、没有置顶效果。
      const todayDateKey = localDateKey(Date.now());
      const sidebarPinnedHoisted = taskListSort === 'pinned_first'
        ? sidebarTaskHistory.filter(chat => !!chat.pinned)
        : [];
      const sidebarTaskGroups = [];
      {
        const byDate = new Map();
        sidebarTaskHistory.forEach(chat => {
          if (sidebarPinnedHoisted.length && chat.pinned) return;
          const key = localDateKey(chat.updatedAt || chat.pinnedAt);
          if (!byDate.has(key)) byDate.set(key, []);
          byDate.get(key).push(chat);
        });
        byDate.forEach((rows, key) => { sidebarTaskGroups.push({ key, rows }); });
        sidebarTaskGroups.sort((a, b) => {
          if (a.key === 'unknown') return 1;
          if (b.key === 'unknown') return -1;
          return b.key.localeCompare(a.key);
        });
      }

      // Code-style sidebar: lists only code sessions, grouped by folder (workspace);
      // groups and rows both sort by latest activity descending, temporary sessions merge
      // into one bottom group; with "pinned first", pinned code sessions hoist above the
      // folder groups.
      const sidebarCodeTasks = codeStyleActive
        ? sidebarTaskHistory.filter(chat => chat.taskKind === 'codex')
        : [];
      const sidebarFolderPinned = taskListSort === 'pinned_first'
        ? sidebarCodeTasks.filter(chat => !!chat.pinned)
        : [];
      const sidebarFolderGroups = codeStyleActive
        ? groupSessionsByFolder(
            sidebarCodeTasks.filter(chat => !(sidebarFolderPinned.length && chat.pinned)))
        : [];

      // latest-ref mirror: the pet-snapshot broadcast effect only subscribes to bs.sessions/sessionBusy/language,
      // while snapshot contents (id/title/working) are read via refs to reduce effect resubscription.
      petSnapshotRef.current = chatHistory.map(chat => ({
        id: chat.id,
        title: chat.title,
        working: chat.working,
      }));
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

      async function navigateFromScheduledRun(nextView, beforeNavigate) {
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
      }

      function openSettingsSection(section = 'general') {
        // 记录进入设置前的页面（代码页齿轮等深链入口），关闭设置时原路返回，
        // 而不是一律回工作页。
        if (currentView !== 'settings') settingsReturnViewRef.current = currentView;
        setSettingsInitialSection(section);
        return navigateFromScheduledRun('settings');
      }

      const closeMobileSidebar = useCallback(() => {
        if (!isWeb || typeof window === 'undefined') return;
        if (window.matchMedia && window.matchMedia('(max-width: 639px)').matches) {
          setIsSidebarOpen(false);
        }
      }, []);

      function scheduledRunLabel(value) {
        return (t.uiScheduled.runStatus[value] || value || t.uiScheduled.unknown);
      }

      async function handleOpenScheduledRunShortcut(run) {
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
      }

      function handleNewChat(installedToolId, forceMode) {
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
      }

      function handleSwitchHomeMode(mode) {
        if (mode === 'code' && codexAcpSupported) {
          setCodeModeOn(true);
          updateActiveCodexSession(null);
          setCodexDraftEpoch(value => value + 1);
          setCurrentView('codex');
        } else if (mode === 'design') {
          setCodeModeOn(false);
          // 仅草稿态（无活跃会话）才开新会话：从 code 页切回时 bridge 的
          // activeSessionId 仍是原工作会话，强制 createNewSession 会新建一个
          // plain 会话（默认 Yolo），把用户切过的 Plan 顶掉——表现为「从代码
          // 切回工作/设计，审批模式变回 Yolo」。保留原会话，ChatView 挂载后
          // 显示其实测 mode。与 ChatView 内 work↔design 本地切换（不建会话）
          // 行为保持一致。
          const scopeKey = bridge.activeSessionId
            ? createPinvouModeScopeKey(bridge.activeSessionId)
            : undefined;
          savePinvouModeState({ mode: 'design' }, undefined, scopeKey);
          if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();
          // code 页期间原工作会话的 mode 可能已被修改（code 页独立链路），
          // 切回前拉一次实测值，避免 ChatView 挂载后显示旧 modeState。
          if (bridge.available && bridge.activeSessionId) {
            bridge.interaction.syncModeState().catch(() => {});
          }
          setCurrentView('chat');
        } else if (mode === 'work') {
          setCodeModeOn(false);
          const scopeKey = bridge.activeSessionId
            ? createPinvouModeScopeKey(bridge.activeSessionId)
            : undefined;
          savePinvouModeState({ mode: 'work' }, undefined, scopeKey);
          if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();
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

      function handleSwitchCodexSession(id) {
        setCodeModeOn(true);
        updateActiveCodexSession(id);
        setCurrentView('codex');
        closeMobileSidebar();
      }

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

      async function handleDeleteSession(id) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.deleteSession(id);
        if (isCodexSession) {
          if (activeCodexId === id) updateActiveCodexSession(null);
          await refreshCodexSessions().catch(() => {});
        }
      }

      async function handleRenameSession(id, title) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.renameSession(id, title);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }

      async function handleToggleSessionPinned(id, pinned) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.toggleSessionPinned(id, pinned);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }

      function handleArchiveSession(id) {
        const chat = allSidebarTasks.find(c => c.id === id);
        setArchiveConfirm(chat || { id, title: t.newChat });
      }

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

      function handleSetTheme(th) {
        setActiveTheme(th);
        if (isWeb) {
          try { window.localStorage.setItem('pinvou.web.theme', th); } catch { /* silently degrade when WebView disables storage */ }
          return;
        }
        if (bridge.available) {
          bridge.settings.saveSettings({ theme: th === 'dark' ? 'genesis' : 'liquid-light' });
        }
      }

      function handleSetSearchProvider(p) {
        if (p === searchProvider) return;
        setEnabledSearchProviders(prev => [...new Set(['bing', ...prev, p])]);
        setSearchProvider(p);
        setSearchApiKey(searchKeyDrafts[p] || '');
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

      function handleTestSearchProvider(p) {
        if (!bridge.available || !bridge.settings.testSearchProvider) return Promise.resolve(t.uiMainApp.searchTestUnavailable);
        const action = searchProviderKeyAction(p);
        const draft = searchKeyDrafts[p] || '';
        return bridge.settings.testSearchProvider(p, action === 'replace' ? draft : '');
      }

      function handleSetSearchApiKey(k, providerOverride) {
        const targetProvider = providerOverride || searchProvider;
        if (targetProvider === searchProvider) setSearchApiKey(k);
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
      const scheduledUnread = !!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns));
      const mobileTitle = currentView === 'chat'
        ? ((((chatHistory || []).find(c => c.id === activeChat)) || {}).title || 'PINVOU')
        : currentView === 'codex'
          ? ((((codexHistory || []).find(c => c.id === activeCodexId)) || {}).title || t.sidebarTaskFilterCode)
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
      // 日期分组/平铺两种布局共用的任务项渲染
      const renderSidebarTaskItem = (chat) => {
        const detachKind = chat.taskKind === 'codex' ? 'codex-session' : 'session';
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
                ? () => handleOpenScheduledRunShortcut(chat.scheduledRun)
                : handleSwitchSession}
            onRename={handleRenameSession}
            onDelete={handleDeleteSession}
            onTogglePinned={handleToggleSessionPinned}
            onOpenFolder={can('externalSystemOpen') ? ((id) => bridge.artifacts.revealSessionFolder && bridge.artifacts.revealSessionFolder(id)) : undefined}
            onArchive={handleArchiveSession}
            dragKind={detachKind}
            dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === `${detachKind}:${chat.id}`}
            onPickUp={canDetachWindows ? ((geom) => beginTearOff(detachKind, chat.id, chat.title, geom)) : undefined}
          />
        );
      };

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
            <div className="fixed left-1/2 bottom-8 z-[120] -translate-x-1/2 rounded-full bg-black/80 px-4 py-2 text-[13px] font-medium text-white shadow-2xl">
              {settingsToast}
            </div>,
            document.body
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

          {can('desktopChrome') && <TitleBar theme={activeTheme} t={t} sidebarOpen={isSidebarOpen} />}

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
                list is, it never squeezes the nav. In code style it collapses to a single
                collapse bar by default, but New chat stays pinned; the remaining nav items
                can be collapsed again at the bottom after expanding. */}
            <div data-testid="sidebar-primary-nav" className={`shrink-0 flex flex-col gap-0.5 mt-1.5 max-sm:gap-0 max-sm:mt-1 ${isSidebarOpen ? 'px-3' : 'px-2 items-center'}`}>
              <NavItem
                icon={<Edit2 size={18} />} label={t.newChat}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => handleNewChat()}
              />
              {/* On the compact shell search is only reachable from the nav, so it must
                  stay pinned even when collapsed */}
              {(!isSidebarOpen || isCompactShell) && (
                <NavItem
                  icon={<Search size={18} />} label={t.searchChats}
                  active={searchOverlayOpen}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => setSearchOverlayOpen(true)}
                />
              )}
              {codeStyleActive && isSidebarOpen && !codeNavExpanded ? (
                <button
                  type="button"
                  data-testid="sidebar-primary-nav-expand"
                  onClick={() => setCodeNavExpanded(true)}
                  title={t.sidebarNavExpand}
                  className={`w-full h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                >
                  <span className="truncate">{t.sidebarNavExpand}</span>
                  <ChevronDown size={14} className="shrink-0" />
                </button>
              ) : (
              <>
              {SCHEDULED_TASKS_ENTRY_ENABLED && (
                <NavItem
                  icon={<Clock size={18} />} label={t.scheduledPlans}
                  active={currentView === 'scheduled'}
                  unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))}
                  theme={activeTheme}
                  t={t}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => navigateFromScheduledRun('scheduled')}
                  onPointerEnter={() => prefetchView('scheduled')} onFocus={() => prefetchView('scheduled')}
                />
              )}
              <NavItem
                icon={<Package size={18} />} label={t.outputs}
                active={currentView === 'outputs'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('outputs')}
                onPointerEnter={() => prefetchView('knowledge')} onFocus={() => prefetchView('knowledge')}
                dragKind={canDetachWindows ? 'outputs' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'outputs:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('outputs', undefined, t.outputs, geom) : undefined}
              />
              <NavItem
                icon={<BarChart2 size={18} />} label={t.monitor}
                active={currentView === 'monitor'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onPointerEnter={() => prefetchView('monitor')} onFocus={() => prefetchView('monitor')}
                onClick={() => {
                  navigateFromScheduledRun('monitor', () => {
                    const liveBridge = window.TauriBridge || bridge;
                    if (liveBridge?.monitor && typeof liveBridge.monitor.startMonitorPolling === 'function') liveBridge.monitor.startMonitorPolling();
                  });
                }}
                dragKind={canDetachWindows ? 'monitor' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'monitor:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('monitor', undefined, t.monitor, geom) : undefined}
              />
              <NavItem
                icon={<Puzzle size={18} />} label={t.toolStore}
                active={currentView === 'toolStore'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('toolStore')}
                onPointerEnter={() => prefetchView('toolStore')} onFocus={() => prefetchView('toolStore')}
                dragKind={canDetachWindows ? 'toolstore' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'toolstore:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('toolstore', undefined, t.toolStore, geom) : undefined}
              />
              <NavItem
                icon={<Layers size={18} />} label={t.cardPool}
                active={currentView === 'cardpool'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))}
                onPointerEnter={() => prefetchView('cardpool')} onFocus={() => prefetchView('cardpool')}
                dragKind={canDetachWindows ? 'cardpool' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'cardpool:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('cardpool', undefined, t.cardPool, geom) : undefined}
              />
              <NavItem
                icon={<BookOpen size={18} />} label={t.knowledge}
                active={currentView === 'knowledge'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('knowledge')}
                onPointerEnter={() => prefetchView('knowledge')} onFocus={() => prefetchView('knowledge')}
                dragKind={canDetachWindows ? 'knowledge' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'knowledge:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('knowledge', undefined, t.knowledge, geom) : undefined}
              />
              {/* 收起态专属:展开态近期列表的高亮项就是回会话入口,不重复渲染 */}
              {!isSidebarOpen && (
                <NavItem
                  icon={<MessageSquare size={18} />} label={t.currentChat}
                  active={currentView === 'chat'}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => navigateFromScheduledRun('chat')}
                />
              )}
              {codeStyleActive && isSidebarOpen && codeNavExpanded && (
                <button
                  type="button"
                  onClick={() => setCodeNavExpanded(false)}
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
                    <div className={`group h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold ${
                      activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'
                    }`}>
                      <span className="truncate">
                        {t.sidebarTaskList} ({codeStyleActive ? sidebarCodeTasks.length : sidebarTaskHistory.length})
                      </span>
                      <span className="flex items-center">
                        {/* 对话管理页入口:悬停任务列表行显现(触屏常显),替代原搜索入口 */}
                        <button
                          type="button"
                          onClick={() => navigateFromScheduledRun('search')}
                          className={`mr-1 h-6 px-2 shrink-0 rounded-full text-[12px] font-normal transition-opacity opacity-0 group-hover:opacity-100 max-sm:opacity-100 ${activeTheme === 'dark' ? 'text-[#A8C7FA] hover:bg-[#282A2C]' : 'text-[#0B57D0] hover:bg-[#E1E5EA]'}`}
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
                    {taskFilterOpen && (
                      <div
                        data-testid="sidebar-task-filter-menu"
                        className={`absolute right-0 top-9 z-50 w-44 overflow-hidden rounded-2xl border p-1.5 shadow-xl ${
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
                    {codeStyleActive ? (
                      (sidebarFolderPinned.length > 0 || sidebarFolderGroups.length > 0) ? (
                        <>
                          {sidebarFolderPinned.length > 0 && (
                            <div className="space-y-0.5">
                              {sidebarFolderPinned.map(renderSidebarTaskItem)}
                            </div>
                          )}
                          {sidebarFolderGroups.map((group) => {
                            const isOpen = folderGroupOpen[group.key] ?? true;
                            const label = group.key === TEMPORARY_GROUP_KEY
                              ? t.uiCodex.temporarySession
                              : workspaceDisplayName(group.key);
                            return (
                              <div key={group.key}>
                                <button
                                  type="button"
                                  data-testid="sidebar-folder-group"
                                  title={group.key === TEMPORARY_GROUP_KEY ? undefined : group.key}
                                  onClick={() => setFolderGroupOpen(prev => ({ ...prev, [group.key]: !isOpen }))}
                                  className={`w-full h-7 px-4 flex items-center justify-between rounded-full text-[12px] transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                                >
                                  <span className="truncate">{label} ({group.rows.length})</span>
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
                        <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
                          {t.sidebarTaskEmpty}
                        </div>
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
                      <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
                        {t.sidebarTaskEmpty}
                      </div>
                    ) : (
                      <div className="space-y-0.5">
                        {sidebarTaskHistory.length > 0 ? sidebarTaskHistory.map(renderSidebarTaskItem) : (
                          <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
                            {t.sidebarTaskEmpty}
                          </div>
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
                {showMegacubeSite && (
                  <button type="button"
                    onClick={() => invokeTauri('open_external_url', { url: 'https://www.h3c.com/cn/pub/minisite/202606/MegaCube/megacube/index.html' })}
                    title={t.megacubeSite}
                    className={`flex items-center rounded-xl transition-colors ${isSidebarOpen ? 'flex-1 min-w-0 px-2 py-1.5 gap-3' : 'justify-center w-10 h-10'} ${activeTheme === 'dark' ? 'hover:bg-[#333537] active:bg-[#3A3C3E]' : 'hover:bg-[#E1E5EA] active:bg-[#D8DCE1]'}`}
                  >
                    <img src="assets/megacube-icon.png" alt="MegaCube" className="w-8 h-8 shrink-0 rounded-lg object-contain" />
                    {isSidebarOpen && (
                      <span className="text-[14px] font-medium leading-none whitespace-nowrap text-left">MegaCube</span>
                    )}
                  </button>
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
                {/* Sidebar style toggle in code mode: separate from the left icon group,
                    pinned to the sidebar's bottom-right corner */}
                {isSidebarOpen && codeModeOn && (
                  <button
                    type="button"
                    data-testid="sidebar-code-style-toggle"
                    onClick={toggleSidebarCodeStyle}
                    title={sidebarCodeStyle === 'code' ? t.sidebarCodeStyleOff : t.sidebarCodeStyleOn}
                    aria-label={sidebarCodeStyle === 'code' ? t.sidebarCodeStyleOff : t.sidebarCodeStyleOn}
                    className={`ml-auto relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${sidebarCodeStyle === 'code' ? (activeTheme === 'dark' ? 'bg-[#004A77] text-[#C2E7FF] hover:bg-[#0B5C8F]' : 'bg-[#D3E3FD] text-[#0B57D0] hover:bg-[#C2DAFC]') : (activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]')}`}
                  >
                    {sidebarCodeStyle === 'code' ? <FolderOpen size={18} /> : <IconList size={18} />}
                  </button>
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
              <SettingsErrorBoundary theme={activeTheme} t={t}>
                <LazySettingsView
                  activeTheme={activeTheme} setActiveTheme={handleSetTheme}
                  language={language} setLanguage={handleSetLanguage}
                  superPerm={superPerm} setSuperPerm={handleToggleSuperPerm}
                  taskCompletedNotif={taskCompletedNotif} setTaskCompletedNotif={handleSetTaskCompletedNotif}
                  searchProvider={searchProvider} setSearchProvider={handleSetSearchProvider}
                  enabledSearchProviders={enabledSearchProviders}
                  onAddSearchProvider={handleAddSearchProvider}
                  onDeleteSearchProvider={handleDeleteSearchProvider}
                  onTestSearchProvider={handleTestSearchProvider}
                  searchApiKey={searchApiKey} setSearchApiKey={handleSetSearchApiKey}
                  searchHasSavedKey={searchHasSavedKey(searchProvider)}
                  savedModels={(bs && bs.savedModels) || []}
                  activeModelId={bs && bs.activeModelId}
                  onSaveModel={(m) => bridge.available && bridge.models.saveModel(m)}
                  onDeleteModel={(m) => { if (bridge.available) bridge.models.deleteModel(m.id); }}
                  onSetActiveModel={(id) => bridge.available && bridge.models.setActiveModel(id)}
                  onSaveSearchConfig={handleSaveSearchConfig}
                  onConfirmSearchConfig={handleConfirmSearchConfig}
                  onMemoryEnabledChange={handleSetMemoryEnabled}
                  onPetEnabledChange={handleSetPetEnabled}
                  searchNeedsRestart={searchNeedsRestart}
                  languageNeedsRestart={languageNeedsRestart}
                  bs={bs}
                  t={t}
                  sidebarDateGrouping={sidebarDateGrouping}
                  onSidebarDateGroupingChange={handleSetSidebarDateGrouping}
                  updateFocusTick={settingsUpdateFocusTick}
                  initialSection={settingsInitialSection}
                  onCloseSettings={() => navigateFromScheduledRun(settingsReturnViewRef.current || 'chat')}
                />
              </SettingsErrorBoundary>
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
            {currentView === 'toolStore' && <LazyToolStoreView theme={activeTheme} t={t} onNewChat={handleNewChat} />}
            {currentView === 'cardpool' && <LazyCardPoolView theme={activeTheme} t={t} bs={bs} onEquipped={() => { setCodeModeOn(false); setCurrentView('chat'); }} onAICreate={startAICard} initialMyOnly={poolMyOnly} />}
            {currentView === 'chat' && <ChatView theme={activeTheme} t={t} bs={bs} prefill={chatPrefill} prefillAppend={chatPrefillAppend} focusComposerTick={petFocusComposerTick} onPrefillConsumed={() => { setChatPrefill(''); setChatPrefillAppend(false); }} onOpenEditor={handleOpenPersonaEditor} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => openSettingsSection('general')} onGotoModelSettings={() => openSettingsSection('model')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} codeModeAvailable={codexAcpSupported} onSwitchHomeMode={handleSwitchHomeMode} browserDockAvailable={browserDockAvailable} browserDockOpen={browserPaneOpen} rightDockActivePanelId={browserDockSelectedPanelId} onRightDockPanelSelectionChange={selectRightDockPanel} onOpenBrowserDock={openBrowserDock} />}
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
                bs={bs}
                onGotoModelSettings={() => openSettingsSection('model')}
                onGotoSettings={() => openSettingsSection('general')}
                onGotoTools={() => navigateFromScheduledRun('toolStore')}
              />
            )}
            {SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled' && (
              bs && bs.scheduledRunContext ? (
                <ChatView theme={activeTheme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={handleOpenPersonaEditor} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => openSettingsSection('general')} onGotoModelSettings={() => openSettingsSection('model')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} browserDockAvailable={browserDockAvailable} browserDockOpen={browserPaneOpen} rightDockActivePanelId={browserDockSelectedPanelId} onRightDockPanelSelectionChange={selectRightDockPanel} onOpenBrowserDock={openBrowserDock} />
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
                onOpenFolder={can('externalSystemOpen') ? ((id) => bridge.artifacts.revealSessionFolder && bridge.artifacts.revealSessionFolder(id)) : undefined}
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
              <LazyWebAccessModal theme={activeTheme} bs={bs} t={t} onClose={() => setWebAccessOpen(false)} />
            </Suspense>
            </ViewErrorBoundary>
            )}

            {/* App 级自创卡编辑器: 聊天里「存入卡牌池」草稿走这条。错误边界与
                WebAccessModal 同款:lazy chunk 拉取失败不能卸载整个应用窗口。 */}
            {personaEditor && browserOverlayPublicationReady && (
              <ViewErrorBoundary t={t}>
              <Suspense fallback={null}>
              <LazyPersonaEditorModal initial={personaEditor.initial} isDark={activeTheme === 'dark'} t={t}
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

            {/* MegaCube(GB10) 本地大模型一键引导 —— 全局首屏弹窗;引导中禁止背景关窗 */}
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
                  if (liveBridge && typeof liveBridge.startMonitorPolling === 'function') liveBridge.startMonitorPolling();
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
              ...(SCHEDULED_TASKS_ENTRY_ENABLED ? [{ key: 'scheduled', label: t.scheduledPlans, icon: <Clock size={18} />,
                active: currentView === 'scheduled', dot: scheduledUnread,
                onClick: () => mobileNavigate('scheduled') }] : []),
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
