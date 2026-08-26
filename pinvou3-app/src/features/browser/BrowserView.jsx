// 普通对话模式的内嵌浏览器侧栏：
// - 显示与 Agent 共用的系统原生 WebView，多标签按当前对话隔离
// - 原生表面不可用时显式报错并允许重试，不使用连续截图作为浏览回退
// - 地址栏导航 + 后退/前进/刷新 + 新建/切换/关闭标签 + 在系统浏览器打开
// 仅当当前对话实际启用浏览器能力（Rust 端 emit browser:activated）后挂载，
// 未调用时不渲染、不加载。

import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { invokeTauri, listenTauri } from '../../platform/tauri/client.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import {
  browserPerformanceNow,
  recordBrowserPerformance,
} from './browser-performance.mjs';
import {
  browserAddressValue,
  browserTabLabel,
  isInternalBlankPageUrl,
  shouldShowNativeBrowserSurface,
} from './browser-display.mjs';
import {
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  Globe,
  Maximize2,
  Plus,
  RefreshCw,
  XIcon,
} from '../../components/icons.jsx';

// 底层用安全的空文档初始化原生 WebView；产品界面统一呈现为“新标签页”，
// 不向用户暴露 about:blank 这一实现细节。
const HOME_URL = 'about:blank';
let nativeSurfaceGenerationPromise = null;
let nativeSurfaceVisibilitySequence = 0;

// The browser page is one physical native surface, while React effects can overlap
// briefly during portal remounts, responsive layout changes, or HMR. Keep the
// physical intent outside an individual effect so identical bounds share one show
// command and an obsolete cleanup cannot hide a newer task's surface.
const nativeSurfaceCoordinator = {
  owner: null,
  transitionOwner: null,
  desired: 'unknown',
  sessionId: null,
  boundsKey: '',
  phase: 'unknown',
  pending: null,
};
const nativeSurfaceResumeListeners = new Set();

function beginNativeSurfaceGeneration() {
  if (!nativeSurfaceGenerationPromise) {
    nativeSurfaceGenerationPromise = invokeTauri('browser_begin_surface_generation')
      .then((generation) => {
        if (!Number.isSafeInteger(generation) || generation <= 0) {
          throw new Error('invalid native browser surface generation');
        }
        nativeSurfaceVisibilitySequence = 0;
        return generation;
      })
      .catch((error) => {
        nativeSurfaceGenerationPromise = null;
        nativeSurfaceVisibilitySequence = 0;
        throw error;
      });
  }
  return nativeSurfaceGenerationPromise;
}

async function invokeNativeSurface(command, args) {
  const visibilityGeneration = await beginNativeSurfaceGeneration();
  nativeSurfaceVisibilitySequence += 1;
  return invokeTauri(command, {
    ...args,
    visibilityGeneration,
    visibilitySequence: nativeSurfaceVisibilitySequence,
  });
}

function sameNativeSurfaceTarget(sessionId, boundsKey) {
  return nativeSurfaceCoordinator.sessionId === sessionId
    && nativeSurfaceCoordinator.boundsKey === boundsKey;
}

function claimNativeSurfaceShow(owner, sessionId, bounds, boundsKey) {
  // A React transition already owns a hide ACK barrier. ResizeObserver/HMR
  // callbacks from the still-published old tree must not supersede that hide
  // with a newer show sequence.
  if (nativeSurfaceCoordinator.transitionOwner) return Promise.resolve(false);
  if (
    nativeSurfaceCoordinator.desired === 'show'
    && sameNativeSurfaceTarget(sessionId, boundsKey)
  ) {
    nativeSurfaceCoordinator.owner = owner;
    if (nativeSurfaceCoordinator.phase === 'visible') return Promise.resolve(true);
    if (nativeSurfaceCoordinator.phase === 'showing' && nativeSurfaceCoordinator.pending) {
      return nativeSurfaceCoordinator.pending;
    }
  }

  nativeSurfaceCoordinator.owner = owner;
  nativeSurfaceCoordinator.desired = 'show';
  nativeSurfaceCoordinator.sessionId = sessionId;
  nativeSurfaceCoordinator.boundsKey = boundsKey;
  nativeSurfaceCoordinator.phase = 'showing';
  const pending = invokeNativeSurface('browser_show_native_surface', { sessionId, bounds })
    .then((shown) => {
      const available = !!shown;
      if (
        nativeSurfaceCoordinator.pending === pending
        && nativeSurfaceCoordinator.desired === 'show'
        && sameNativeSurfaceTarget(sessionId, boundsKey)
      ) {
        nativeSurfaceCoordinator.phase = available ? 'visible' : 'hidden';
        nativeSurfaceCoordinator.pending = null;
      }
      return available;
    })
    .catch((error) => {
      if (nativeSurfaceCoordinator.pending === pending) {
        nativeSurfaceCoordinator.phase = 'unknown';
        nativeSurfaceCoordinator.pending = null;
      }
      throw error;
    });
  nativeSurfaceCoordinator.pending = pending;
  return pending;
}

function claimNativeSurfaceHide(owner, sessionId) {
  if (
    nativeSurfaceCoordinator.desired === 'hide'
    && nativeSurfaceCoordinator.sessionId === sessionId
    && (nativeSurfaceCoordinator.phase === 'hiding' || nativeSurfaceCoordinator.phase === 'hidden')
  ) {
    nativeSurfaceCoordinator.owner = owner;
    return nativeSurfaceCoordinator.pending || Promise.resolve();
  }

  nativeSurfaceCoordinator.owner = owner;
  nativeSurfaceCoordinator.desired = 'hide';
  nativeSurfaceCoordinator.sessionId = sessionId;
  nativeSurfaceCoordinator.boundsKey = '';
  nativeSurfaceCoordinator.phase = 'hiding';
  const pending = invokeNativeSurface('browser_hide_native_surface', { sessionId })
    .then(() => {
      if (
        nativeSurfaceCoordinator.pending === pending
        && nativeSurfaceCoordinator.desired === 'hide'
        && nativeSurfaceCoordinator.sessionId === sessionId
      ) {
        nativeSurfaceCoordinator.phase = 'hidden';
        nativeSurfaceCoordinator.pending = null;
      }
    })
    .catch((error) => {
      if (nativeSurfaceCoordinator.pending === pending) {
        nativeSurfaceCoordinator.phase = 'unknown';
        nativeSurfaceCoordinator.pending = null;
      }
      throw error;
    });
  nativeSurfaceCoordinator.pending = pending;
  return pending;
}

function resumeNativeSurfaceOwner(owner, sessionId) {
  if (nativeSurfaceCoordinator.transitionOwner !== owner) return;
  nativeSurfaceCoordinator.transitionOwner = null;
  if (nativeSurfaceCoordinator.owner === owner) {
    nativeSurfaceCoordinator.owner = null;
    nativeSurfaceCoordinator.desired = 'unknown';
    nativeSurfaceCoordinator.boundsKey = '';
    if (nativeSurfaceCoordinator.phase !== 'hidden') {
      nativeSurfaceCoordinator.phase = 'unknown';
    }
  }
  nativeSurfaceResumeListeners.forEach((listener) => listener(sessionId));
}

// React layers cannot cover a native child WebView. Callers acquire this lease
// before publishing an overlay/view/session switch, then release it after the
// React state mutation has been queued. A failed hide never yields a lease.
export async function acquireNativeSurfaceTransitionHide(fallbackSessionId) {
  const owner = Symbol('browser-native-surface-transition');
  const sessionId = nativeSurfaceCoordinator.sessionId || fallbackSessionId;
  if (!sessionId) return { release() {} };

  nativeSurfaceCoordinator.transitionOwner = owner;
  try {
    await claimNativeSurfaceHide(owner, sessionId);
  } catch (error) {
    resumeNativeSurfaceOwner(owner, sessionId);
    throw error;
  }

  let released = false;
  return {
    sessionId,
    release() {
      if (released) return;
      released = true;
      resumeNativeSurfaceOwner(owner, sessionId);
    },
  };
}

function ownsNativeSurfaceShow(owner, sessionId, boundsKey) {
  return nativeSurfaceCoordinator.owner === owner
    && nativeSurfaceCoordinator.desired === 'show'
    && sameNativeSurfaceTarget(sessionId, boundsKey);
}

function releaseNativeSurface(owner, sessionId) {
  if (nativeSurfaceCoordinator.owner !== owner) return Promise.resolve();
  if (nativeSurfaceCoordinator.desired === 'hide') {
    nativeSurfaceCoordinator.owner = null;
    return nativeSurfaceCoordinator.pending || Promise.resolve();
  }
  return claimNativeSurfaceHide(owner, sessionId);
}

export function BrowserView({
  theme,
  t,
  sessionId,
  nativeSurfaceSuspended = false,
  ownershipSlot = null,
}) {
  const isDark = theme === 'dark';
  const [nativeAvailable, setNativeAvailable] = useState(null);
  const [surfaceEpoch, setSurfaceEpoch] = useState(0);
  const [initialStatusResolved, setInitialStatusResolved] = useState(false);
  const [url, setUrl] = useState('');
  const [urlInput, setUrlInput] = useState('');
  const [tabs, setTabs] = useState([]);
  const [activeSession, setActiveSession] = useState(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const [persistenceWarning, setPersistenceWarning] = useState('');
  const [controlOwner, setControlOwner] = useState(null);
  const [controlRevision, setControlRevision] = useState(null);
  const wheelRef = useRef(null);
  const urlInputRef = useRef(null);
  const activeSessionRef = useRef(null);
  const sessionIdRef = useRef(sessionId);
  const statusRequestEpochRef = useRef(0);
  const tabsRequestEpochRef = useRef(0);
  sessionIdRef.current = sessionId;
  const showingNewTab = running && isInternalBlankPageUrl(url);
  const nativeSurfaceReady = shouldShowNativeBrowserSurface({
    statusResolved: initialStatusResolved,
    running,
    url,
    suspended: nativeSurfaceSuspended,
  });
  const shouldSuspendNativeSurface = !nativeSurfaceReady;

  useEffect(() => {
    const resume = () => setSurfaceEpoch((epoch) => epoch + 1);
    nativeSurfaceResumeListeners.add(resume);
    return () => nativeSurfaceResumeListeners.delete(resume);
  }, [sessionId]);

  // 工具栏由 React 提供，页面由应用内系统 WebView 原生承载。这里故意没有
  // 截图回退：创建失败必须对用户可见，不能静默切换身份与交互语义。
  useEffect(() => {
    const host = wheelRef.current;
    if (!host || !sessionId) return undefined;
    const surfaceOwner = Symbol(`browser-surface:${sessionId}`);
    if (shouldSuspendNativeSurface) {
      claimNativeSurfaceHide(surfaceOwner, sessionId).catch(() => {});
      return () => {
        void releaseNativeSurface(surfaceOwner, sessionId).catch(() => {});
      };
    }
    let disposed = false;
    let raf = 0;
    let syncing = false;
    let queued = false;
    let lastShownBoundsKey = '';

    const syncBounds = async () => {
      if (disposed || syncing) {
        queued = true;
        return;
      }
      const rect = host.getBoundingClientRect();
      if (rect.width < 2 || rect.height < 2) return;
      const scale = window.devicePixelRatio || 1;
      const bounds = {
        x: Math.round(rect.left * scale),
        y: Math.round(rect.top * scale),
        width: Math.max(1, Math.round(rect.width * scale)),
        height: Math.max(1, Math.round(rect.height * scale)),
      };
      const boundsKey = `${sessionId}:${bounds.x}:${bounds.y}:${bounds.width}:${bounds.height}`;
      if (boundsKey === lastShownBoundsKey) return;
      syncing = true;
      const showStartedAt = browserPerformanceNow();
      try {
        const shown = await claimNativeSurfaceShow(surfaceOwner, sessionId, bounds, boundsKey);
        // cleanup/suspension claims a newer sequence before an old show can settle.
        // The host rejects that stale show, and the owner check also prevents an old
        // task/effect from publishing availability into the current React instance.
        if (disposed || !ownsNativeSurfaceShow(surfaceOwner, sessionId, boundsKey)) return;
        if (shown) lastShownBoundsKey = boundsKey;
        setNativeAvailable(!!shown);
        if (!shown) {
          claimNativeSurfaceHide(surfaceOwner, sessionId).catch(() => {});
        }
      } catch {
        if (!disposed && nativeSurfaceCoordinator.owner === surfaceOwner) {
          setNativeAvailable(false);
          claimNativeSurfaceHide(surfaceOwner, sessionId).catch(() => {});
        }
      } finally {
        recordBrowserPerformance('dock_surface_show_ms', browserPerformanceNow() - showStartedAt);
        syncing = false;
        if (queued && !disposed) {
          queued = false;
          scheduleSync();
        }
      }
    };
    const scheduleSync = () => {
      if (raf || disposed) return;
      raf = requestAnimationFrame(() => {
        raf = 0;
        void syncBounds();
      });
    };
    const observer = new ResizeObserver(scheduleSync);
    observer.observe(host);
    window.addEventListener('resize', scheduleSync);
    scheduleSync();
    return () => {
      disposed = true;
      observer.disconnect();
      window.removeEventListener('resize', scheduleSync);
      if (raf) cancelAnimationFrame(raf);
      releaseNativeSurface(surfaceOwner, sessionId).catch(() => {});
    };
  }, [sessionId, surfaceEpoch, shouldSuspendNativeSurface]);

  // ---- 状态同步 ----
  const refreshStatus = useCallback(async () => {
    const requestedSessionId = sessionId;
    const requestEpoch = statusRequestEpochRef.current + 1;
    statusRequestEpochRef.current = requestEpoch;
    const isCurrent = () => (
      sessionIdRef.current === requestedSessionId
      && statusRequestEpochRef.current === requestEpoch
    );
    try {
      const statusStartedAt = browserPerformanceNow();
      const st = await invokeTauri('browser_status', { sessionId: requestedSessionId });
      recordBrowserPerformance(
        'workspace_restore_status_ms',
        browserPerformanceNow() - statusStartedAt,
      );
      if (!isCurrent() || st?.sessionId !== requestedSessionId) return;
      setRunning(!!st.running);
      setUrl(st.url || '');
      // 同步回填地址栏输入框：挂载/切标签/开关标签后 Rust 不 emit
      // browser:navigation，不回填则输入框停留上一标签页的 URL。初始化
      // about:blank 仅是宿主实现细节，地址栏应保持空白。
      setUrlInput(browserAddressValue(st.url));
      if (st.activeTab) {
        activeSessionRef.current = st.activeTab;
        setActiveSession(st.activeTab);
      }
      setControlOwner(st.controlOwner || null);
      setControlRevision(Number.isFinite(st.controlRevision) ? st.controlRevision : null);
      setPersistenceWarning(st.persistenceWarning || '');
      // 真正的恢复失败必须留在浏览器侧栏中可见；正常不存在的工作区由
      // main.jsx 保持关闭，不应伪装成错误或自动展开。
      setError(st.restoreError || '');
      setInitialStatusResolved(true);
    } catch (e) {
      if (!isCurrent()) return;
      setRunning(false);
      setControlOwner(null);
      setControlRevision(null);
      setPersistenceWarning('');
      setError(typeof e === 'string' ? e : String(e));
      setInitialStatusResolved(true);
    }
  }, [sessionId]);
  const refreshTabs = useCallback(async () => {
    const requestedSessionId = sessionId;
    const requestEpoch = tabsRequestEpochRef.current + 1;
    tabsRequestEpochRef.current = requestEpoch;
    try {
      const list = await invokeTauri('browser_list_tabs', { sessionId: requestedSessionId });
      if (
        sessionIdRef.current !== requestedSessionId
        || tabsRequestEpochRef.current !== requestEpoch
      ) return;
      setTabs(list || []);
    } catch {
      /* 浏览器未就绪时静默 */
    }
  }, [sessionId]);

  useEffect(() => {
    let disposed = false;
    refreshStatus();
    refreshTabs();
    const unsubs = [];
    // 退订竞态守卫：listenTauri 的 promise 可能在组件卸载后才 resolve，
    // 此时 push 进已失效的 unsubs 会让监听器永不退订，并在已卸载组件上继续
    // setState。与 main.jsx 的 browser 监听采用同款 disposed 模式。
    const guard = (p) => p.then((u) => {
      if (disposed) u && u();
      else unsubs.push(u);
    }).catch(() => {});
    guard(listenTauri('browser:navigation', (e) => {
      // 只对当前激活标签页的导航更新地址栏：后台标签页的 frameNavigated
      // 不应覆盖地址栏，否则 openExternal 会打开非当前标签页的 URL。
      const p = e.payload || {};
      if (p.sessionId !== sessionId) return;
      if (p.url && (p.tab == null || activeSessionRef.current == null || p.tab === activeSessionRef.current)) {
        setUrl(p.url);
        setUrlInput(browserAddressValue(p.url));
        if (p.sessionId) refreshTabs();
      }
    }));
    guard(listenTauri('browser:tabs-changed', (event) => {
      if (event.payload?.sessionId !== sessionId) return;
      refreshTabs();
      // 激活标签页可能被 MCP 关闭后由 Rust 自愈切换到其他页：同步地址栏/activeTab。
      refreshStatus();
    }));
    guard(listenTauri('browser:tab-title', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      if (!payload.tab || !payload.title) return;
      setTabs((current) => current.map((tab) => (
        tab.target_id === payload.tab ? { ...tab, title: payload.title } : tab
      )));
    }));
    guard(listenTauri('browser:activated', (event) => {
      if (event.payload?.sessionId !== sessionId) return;
      refreshStatus();
      refreshTabs();
      setSurfaceEpoch((epoch) => epoch + 1);
    }));
    guard(listenTauri('browser:stopped', (event) => {
      if (event.payload?.sessionId && event.payload.sessionId !== sessionId) return;
      setRunning(false);
      setError('');
      setControlOwner(null);
      setControlRevision(null);
      setPersistenceWarning('');
    }));
    guard(listenTauri('browser:control-changed', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      setControlOwner(payload.owner || null);
      setControlRevision(Number.isFinite(payload.revision) ? payload.revision : null);
    }));
    guard(listenTauri('browser:navigation-blocked', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      const scheme = payload.scheme ? ` (${payload.scheme})` : '';
      setError(`${t.browserBlockedNavigation}${scheme}`);
    }));
    guard(listenTauri('browser:automation-unavailable', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      setError(t.browserAutomationUnavailable);
    }));
    guard(listenTauri('browser:download-blocked', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      setError(t.browserDownloadBlocked(payload.source || ''));
    }));
    guard(listenTauri('browser:persistence-warning', (event) => {
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      setPersistenceWarning(payload.error || t.browserPersistenceWarning);
    }));
    guard(listenTauri('browser:persistence-restored', (event) => {
      if (event.payload?.sessionId !== sessionId) return;
      setPersistenceWarning('');
    }));
    return () => {
      disposed = true;
      unsubs.forEach((u) => u && u());
    };
  }, [
    refreshStatus,
    refreshTabs,
    sessionId,
    t.browserAutomationUnavailable,
    t.browserBlockedNavigation,
    t.browserDownloadBlocked,
    t.browserPersistenceWarning,
  ]);

  // ---- 导航 ----
  const navigate = useCallback(async (raw) => {
    let target = (raw || '').trim();
    if (!target) return;
    if (!/^https?:\/\//i.test(target) && target !== 'about:blank') {
      target = 'https://' + target;
    }
    try {
      setError('');
      await invokeTauri('browser_navigate', { sessionId, url: target });
      setUrlInput(browserAddressValue(target));
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [sessionId]);

  const runNav = useCallback(async (cmd) => {
    try {
      setError('');
      await invokeTauri(cmd, { sessionId });
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [sessionId]);

  const openExternal = useCallback(async () => {
    if (!url || isInternalBlankPageUrl(url)) return;
    try {
      await invokeTauri('open_user_external_url', { url });
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [url]);

  // ---- 多标签 ----
  const createTab = useCallback(async () => {
    try {
      await invokeTauri('browser_create_tab', { sessionId, url: HOME_URL, background: false });
      refreshTabs();
      // 新标签页激活后刷新 URL/activeTab 状态：Rust 侧 create_tab 不 emit
      // browser:navigation（about:blank 无 frameNavigated），不刷新则地址栏
      // 停留在旧页、"在系统浏览器打开"会拿到上一个标签页的 URL。
      refreshStatus();
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [sessionId, refreshTabs, refreshStatus]);

  const closeTab = useCallback(
    async (targetId) => {
      try {
        await invokeTauri('browser_close_tab', { sessionId, targetId });
        refreshTabs();
        refreshStatus();
      } catch (e) {
        setError(typeof e === 'string' ? e : String(e));
      }
    },
    [sessionId, refreshTabs, refreshStatus]
  );

  const activateTab = useCallback(
    async (targetId) => {
      try {
        const switchStartedAt = browserPerformanceNow();
        await invokeTauri('browser_activate_tab', { sessionId, targetId });
        recordBrowserPerformance('tab_switch_ms', browserPerformanceNow() - switchStartedAt);
        activeSessionRef.current = targetId;
        setActiveSession(targetId);
        // 刷新 URL/导航状态：Rust 侧 activate_tab 切换原生子视图但不 emit
        // browser:navigation，不刷新则地址栏显示旧页 URL、openExternal
        // 可能把上一标签页的 URL 发给系统浏览器。
        refreshStatus();
      } catch (e) {
        setError(typeof e === 'string' ? e : String(e));
      }
    },
    [sessionId, refreshStatus]
  );

  const stopBrowser = useCallback(async () => {
    try {
      await invokeTauri('browser_stop', { sessionId });
      setRunning(false);
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [sessionId]);

  const handBackToAgent = useCallback(async () => {
    try {
      setError('');
      const control = await invokeTauri('browser_hand_back_to_agent', { sessionId });
      setControlOwner(control?.controlOwner || 'agent');
      setControlRevision(Number.isFinite(control?.controlRevision) ? control.controlRevision : null);
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [sessionId]);

  // ---- 渲染 ----
  const shell = 'flex h-full flex-col overflow-hidden';
  const toolbarCls = `flex shrink-0 items-center gap-1 border-b px-2 py-1.5 ${
    isDark ? 'border-[#2A2B2E] bg-[#17181A]' : 'border-[#E5E7EB] bg-[#F8F9FA]'
  }`;
  const btnCls = `rounded-md p-1.5 transition-colors ${
    isDark ? 'text-[#B8B8B8] hover:bg-[#2A2B2E] hover:text-[#F2F2F2]' : 'text-[#555] hover:bg-[#ECECEC] hover:text-[#111]'
  }`;
  const iconBtn = (title, icon, onClick, disabled) => (
    <button
      type="button"
      title={title}
      aria-label={title}
      className={btnCls}
      onClick={onClick}
      disabled={disabled}
      style={disabled ? { opacity: 0.35 } : undefined}
    >
      {icon}
    </button>
  );
  const ownerIsUser = controlOwner === 'user';
  const ownerIsUnclaimed = controlOwner === 'unclaimed';
  const ownershipControl = running && controlOwner ? (
    <div
      className="flex items-center gap-1.5"
      data-testid="browser-control-owner"
      data-owner={controlOwner}
      data-revision={controlRevision == null ? undefined : controlRevision}
      title={ownerIsUser ? t.browserHandBackHint : ownerIsUnclaimed ? t.browserControlUnclaimedHint : t.browserAgentControl}
    >
      <span
        className={`inline-flex h-6 items-center rounded-full px-2 text-[11px] font-medium ${
          ownerIsUser
            ? isDark ? 'bg-[#3B2E19] text-[#F7C873]' : 'bg-[#FFF2CC] text-[#7A4E00]'
            : ownerIsUnclaimed
              ? isDark ? 'bg-white/10 text-[#D4D4D4]' : 'bg-black/5 text-[#555]'
              : isDark ? 'bg-[#173B2C] text-[#7EE2AE]' : 'bg-[#DDF7E9] text-[#17643A]'
        }`}
      >
        {ownerIsUser ? t.browserUserControl : ownerIsUnclaimed ? t.browserControlUnclaimed : t.browserAgentControl}
      </span>
      {ownerIsUser && (
        <button
          type="button"
          data-testid="browser-hand-back"
          className={`h-6 rounded-md px-2 text-[11px] font-medium transition-colors ${
            isDark ? 'bg-white/10 text-[#E8E8E8] hover:bg-white/15' : 'bg-black/5 text-[#333] hover:bg-black/10'
          }`}
          title={t.browserHandBackHint}
          onClick={handBackToAgent}
        >
          {t.browserHandBackAgent}
        </button>
      )}
    </div>
  ) : null;

  return (
    <div className={shell} data-testid="browser-view">
      {ownershipSlot && ownershipControl ? createPortal(ownershipControl, ownershipSlot) : null}
      {/* 标签条在地址栏上方，与桌面浏览器/Codex 的信息层级一致。 */}
      {tabs.length > 0 && (
        <div
          className={`flex shrink-0 items-center gap-1 overflow-x-auto px-2 py-1 ${
            isDark ? 'border-b border-[#2A2B2E] bg-[#1A1B1D]' : 'border-b border-[#E5E7EB] bg-white'
          }`}
        >
          {tabs.map((tab) => {
            const active = tab.target_id === (activeSession || activeSessionRef.current);
            return (
              <div
                key={tab.target_id}
                role="button"
                tabIndex={0}
                aria-pressed={active}
                title={browserTabLabel(tab, t.browserEmptyTab)}
                onClick={() => activateTab(tab.target_id)}
                onKeyDown={(event) => {
                  if (event.key !== 'Enter' && event.key !== ' ') return;
                  event.preventDefault();
                  activateTab(tab.target_id);
                }}
                className={`group flex max-w-[180px] cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-[12px] ${
                  active
                    ? isDark
                      ? 'bg-[#2E2F33] text-[#F2F2F2]'
                      : 'bg-[#E9EBEE] text-[#111]'
                    : isDark
                      ? 'text-[#9A9A9A] hover:bg-[#232428]'
                      : 'text-[#666] hover:bg-[#F0F0F0]'
                }`}
              >
                <Globe size={12} className="shrink-0" style={{ opacity: 0.7 }} />
                <span className="truncate">{browserTabLabel(tab, t.browserEmptyTab)}</span>
                {tabs.length > 1 && <button
                  type="button"
                  aria-label={t.browserTabClose}
                  className={`shrink-0 rounded p-0.5 opacity-0 group-hover:opacity-100 ${
                    isDark ? 'hover:bg-[#3A3B3F]' : 'hover:bg-[#DCDCDC]'
                  }`}
                  title={t.browserTabClose}
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(tab.target_id);
                  }}
                >
                  <XIcon size={11} />
                </button>}
              </div>
            );
          })}
          {iconBtn(t.browserNewTab, <Plus size={15} />, createTab)}
        </div>
      )}

      {/* 工具条 */}
      <div className={toolbarCls}>
        {iconBtn(t.browserBack, <ChevronLeft size={17} />, () => runNav('browser_back'))}
        {iconBtn(t.browserForward, <ChevronRight size={17} />, () => runNav('browser_forward'))}
        {iconBtn(t.browserRefresh, <RefreshCw size={16} />, () => runNav('browser_reload'))}
        <form
          className="mx-1 flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1"
          style={{
            background: isDark ? '#232428' : '#FFFFFF',
            border: `1px solid ${isDark ? '#3A3B3F' : '#D8DADC'}`,
          }}
          onSubmit={(e) => {
            e.preventDefault();
            navigate(urlInput);
          }}
        >
          <Globe size={14} style={{ opacity: 0.5 }} />
          <input
            ref={urlInputRef}
            className="w-full bg-transparent text-[13px] outline-none"
            style={{ color: isDark ? '#E8E8E8' : '#222' }}
            placeholder={t.browserUrlPlaceholder}
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            // IME 组合确认候选词的 Enter 不得触发提交导航（macOS WKWebView
            // bug 165004 下 isComposing 已复位但 keyCode 229 保留，必须走
            // 统一守卫；契约见 tests/ime_compose_guard.test.mjs）。
            onKeyDown={(e) => { if (e.key === 'Enter' && isImeComposing(e)) e.preventDefault(); }}
            spellCheck={false}
            data-testid="browser-url-input"
          />
        </form>
        {!ownershipSlot && ownershipControl}
        {iconBtn(t.browserOpenExternal, <ExternalLink size={15} />, openExternal, !url || isInternalBlankPageUrl(url))}
        {iconBtn(t.browserStop, <XIcon size={16} />, stopBrowser)}
      </div>

      {/* 原生 child WebView 永远绘制在 React DOM 之上，不能把错误浮层放进其
          占位节点，否则用户实际看不到。横幅占据独立布局行，出现时宿主 bounds
          会随 ResizeObserver 自动下移。 */}
      {running && error && (
        <button
          type="button"
          data-testid="browser-error-banner"
          role="alert"
          onClick={() => setError('')}
          className="mx-2 my-1 shrink-0 rounded-md px-3 py-2 text-left text-[12px] shadow-sm"
          style={{
            background: isDark ? '#2A1B1B' : '#FDECEC',
            border: `1px solid ${isDark ? '#5C2B2B' : '#F2B8B5'}`,
            color: isDark ? '#F2B2B2' : '#8C2B2B',
          }}
        >
          <div>{t.browserError}</div>
          <div className="mt-1" style={{ opacity: 0.75, wordBreak: 'break-all' }}>{error}</div>
        </button>
      )}
      {running && persistenceWarning && (
        <div
          data-testid="browser-persistence-warning"
          role="status"
          className="mx-2 my-1 shrink-0 rounded-md px-3 py-2 text-left text-[12px] shadow-sm"
          style={{
            background: isDark ? '#2E2818' : '#FFF7D6',
            border: `1px solid ${isDark ? '#66572A' : '#E8CF72'}`,
            color: isDark ? '#F1D98A' : '#705500',
          }}
        >
          <div>{t.browserPersistenceWarning}</div>
          <div className="mt-1" style={{ opacity: 0.75, wordBreak: 'break-all' }}>{persistenceWarning}</div>
        </div>
      )}

      {/* 原生页面覆盖这个占位区域；React 只负责状态与错误提示。 */}
      <div
        ref={wheelRef}
        className="relative min-h-0 flex-1 overflow-hidden"
        data-testid="browser-native-host"
        style={{ background: isDark ? '#101113' : '#F4F5F6' }}
      >
        {!running && (
          <div className="flex h-full items-center justify-center p-6 text-center text-[13px]" style={{ color: isDark ? '#9A9A9A' : '#777' }}>
            {error ? (
              <div>
                <div>{t.browserError}</div>
                <div className="mt-2" style={{ opacity: 0.6 }}>{error}</div>
              </div>
            ) : (
              <div>
                <div className="mb-2"><Maximize2 size={28} style={{ opacity: 0.4, margin: '0 auto' }} /></div>
                <div>{t.browserLoading}</div>
                <div className="mt-2" style={{ opacity: 0.6, maxWidth: 360 }}>{t.browserNotRunning}</div>
              </div>
            )}
          </div>
        )}
        {showingNewTab && (
          <button
            type="button"
            data-testid="browser-new-tab-page"
            className="flex h-full w-full flex-col items-center justify-center text-center outline-none"
            style={{
              color: isDark ? '#D7D7D7' : '#313131',
              background: isDark ? '#101113' : '#FFFFFF',
            }}
            onClick={() => urlInputRef.current?.focus()}
          >
            <Globe size={30} strokeWidth={1.7} style={{ opacity: 0.72 }} />
            <div className="mt-4 text-[16px] font-semibold">{t.browserStartBrowsing}</div>
            <div
              className="mt-2 text-[13px]"
              style={{ color: isDark ? '#8E8E8E' : '#8A8A8A' }}
            >
              {t.browserStartBrowsingHint}
            </div>
          </button>
        )}
        {nativeAvailable == null && running && !showingNewTab && (
          <div className="flex h-full items-center justify-center text-[13px]" style={{ color: isDark ? '#9A9A9A' : '#777' }}>
            {t.browserLoading}
          </div>
        )}
        {nativeAvailable === false && running && !showingNewTab && (
          <div
            className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center text-[13px]"
            data-testid="browser-native-unavailable"
            style={{ color: isDark ? '#B8B8B8' : '#555' }}
          >
            <div>{t.browserNativeUnavailable}</div>
            <button
              type="button"
              className={`rounded-md border px-3 py-1.5 ${isDark ? 'border-white/15 hover:bg-white/10' : 'border-black/15 hover:bg-black/5'}`}
              onClick={() => {
                setNativeAvailable(null);
                setSurfaceEpoch((epoch) => epoch + 1);
              }}
            >
              {t.browserRetry}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
