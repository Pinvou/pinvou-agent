// Embedded browser dock for normal chats:
// - Shows the system-native WebView shared with the Agent, with tabs scoped per chat.
// - Reports native-surface failures explicitly and allows retry; no screenshot fallback.
// - Supports address navigation, history, reload, tab management, and external opening.
// It mounts only after Rust emits browser:activated for the current chat.

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useReducer,
  useRef,
  useState,
} from 'react';
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
import {
  createDegradedNativeSurfaceHideLease,
  hideNativeSurfaceWithRetry,
  isFailedNativeSurfaceHideGenerationCurrent,
} from './native-surface-transition.mjs';
import {
  EMPTY_PERSISTENCE_WARNING,
  isPersistenceStatusCurrent,
  persistenceWarningReducer,
  visiblePersistenceWarning,
} from './persistence-warning.mjs';
import { dispatchBrowserNavigation } from './browser-navigation.mjs';
import {
  awaitBrowserListenerReadiness,
  browserStatusRetryDelay,
  eventTargetsActiveBrowserTab,
  isBrowserSnapshotDomainCurrent,
  isFragmentOnlyBrowserNavigation,
  isMonotonicControlRevision,
  navigationEventSettlesPending,
  reconcilePendingNavigationWithActiveTab,
  shouldHydrateBrowserAddressInput,
} from './browser-state-sync.mjs';

// The host initializes a native WebView with a safe blank document. Present it as a
// product new-tab page without exposing the about:blank implementation detail.
const HOME_URL = 'about:blank';
const NAVIGATION_PENDING_TIMEOUT_MS = 30_000;
let nativeSurfaceGenerationPromise = null;
let nativeSurfaceVisibilitySequence = 0;
let nativeSurfaceIntentSequence = 0;

// The browser page is one physical native surface, while React effects can overlap
// briefly during portal remounts, responsive layout changes, or HMR. Keep the
// physical intent outside an individual effect so identical bounds share one show
// command and an obsolete cleanup cannot hide a newer task's surface.
const nativeSurfaceCoordinator = {
  owner: null,
  intentOwner: null,
  transitionOwners: new Set(),
  desired: 'unknown',
  sessionId: null,
  boundsKey: '',
  phase: 'unknown',
  pending: null,
  intentSequence: 0,
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
  if (nativeSurfaceCoordinator.transitionOwners.size > 0) return Promise.resolve(false);
  if (
    nativeSurfaceCoordinator.desired === 'show'
    && sameNativeSurfaceTarget(sessionId, boundsKey)
  ) {
    nativeSurfaceCoordinator.owner = owner;
    nativeSurfaceCoordinator.intentOwner = owner;
    if (nativeSurfaceCoordinator.phase === 'visible') return Promise.resolve(true);
    if (nativeSurfaceCoordinator.phase === 'showing' && nativeSurfaceCoordinator.pending) {
      return nativeSurfaceCoordinator.pending;
    }
  }

  nativeSurfaceCoordinator.owner = owner;
  nativeSurfaceCoordinator.intentOwner = owner;
  nativeSurfaceCoordinator.desired = 'show';
  nativeSurfaceCoordinator.sessionId = sessionId;
  nativeSurfaceCoordinator.boundsKey = boundsKey;
  nativeSurfaceCoordinator.phase = 'showing';
  nativeSurfaceCoordinator.intentSequence = ++nativeSurfaceIntentSequence;
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
    // Multiple UI channels may share the same physical hide. Do not replace
    // the creator's retry identity: doing so would cancel its only retry and
    // make the later lease observe a failed barrier it accidentally caused.
    return nativeSurfaceCoordinator.pending || Promise.resolve();
  }

  nativeSurfaceCoordinator.owner = owner;
  nativeSurfaceCoordinator.intentOwner = owner;
  nativeSurfaceCoordinator.desired = 'hide';
  nativeSurfaceCoordinator.sessionId = sessionId;
  nativeSurfaceCoordinator.boundsKey = '';
  nativeSurfaceCoordinator.phase = 'hiding';
  const intentSequence = ++nativeSurfaceIntentSequence;
  nativeSurfaceCoordinator.intentSequence = intentSequence;
  const intentIsCurrent = () => (
    nativeSurfaceCoordinator.pending === pending
    && nativeSurfaceCoordinator.desired === 'hide'
    && nativeSurfaceCoordinator.sessionId === sessionId
    && nativeSurfaceCoordinator.owner === nativeSurfaceCoordinator.intentOwner
    && nativeSurfaceCoordinator.intentSequence === intentSequence
  );
  const pending = hideNativeSurfaceWithRetry({
    hide: () => invokeNativeSurface('browser_hide_native_surface', { sessionId }),
    isCurrent: intentIsCurrent,
    waitBeforeRetry: () => new Promise((resolve) => {
      window.setTimeout(resolve, 50);
    }),
    onError: (error, { attempt, willRetry }) => {
      console.error(
        `[browser] native surface hide failed (session=${sessionId}, attempt=${attempt}/2, retry=${willRetry})`,
        error,
      );
    },
  })
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

function observeNativeSurfaceHide(promise, reason) {
  void Promise.resolve(promise).catch((error) => {
    console.error(`[browser] native surface hide did not complete (${reason})`, error);
  });
}

function resumeNativeSurfaceOwner(owner, sessionId) {
  if (!nativeSurfaceCoordinator.transitionOwners.delete(owner)) return;
  if (nativeSurfaceCoordinator.transitionOwners.size > 0) return;
  nativeSurfaceCoordinator.owner = null;
  nativeSurfaceCoordinator.intentOwner = null;
  nativeSurfaceCoordinator.desired = 'unknown';
  nativeSurfaceCoordinator.boundsKey = '';
  if (nativeSurfaceCoordinator.phase !== 'hidden') {
    nativeSurfaceCoordinator.phase = 'unknown';
  }
  nativeSurfaceResumeListeners.forEach((listener) => {
    listener(sessionId);
  });
}

// React layers cannot cover a native child WebView. Callers acquire this lease
// before publishing an overlay/view/session switch, then release it after the
// guarded React tree has committed. Only primary navigation may retain a
// degraded cleanup lease after both initial hide attempts fail.
export async function acquireNativeSurfaceTransitionHide(
  fallbackSessionId,
  { retainOnFailure = false } = {},
) {
  const owner = Symbol('browser-native-surface-transition');
  const sessionId = nativeSurfaceCoordinator.sessionId || fallbackSessionId;
  if (!sessionId) return { release() {} };

  nativeSurfaceCoordinator.transitionOwners.add(owner);
  try {
    const hide = claimNativeSurfaceHide(owner, sessionId);
    await hide;
  } catch (error) {
    if (retainOnFailure) {
      return createDegradedNativeSurfaceHideLease({
        error,
        isRetryCurrent: () => isFailedNativeSurfaceHideGenerationCurrent({
          transitionOwnerPresent: nativeSurfaceCoordinator.transitionOwners.has(owner),
          capturedSessionId: sessionId,
          currentSessionId: nativeSurfaceCoordinator.sessionId,
          desired: nativeSurfaceCoordinator.desired,
          phase: nativeSurfaceCoordinator.phase,
          pending: nativeSurfaceCoordinator.pending,
        }),
        retryHide: () => claimNativeSurfaceHide(owner, sessionId),
        releaseOwner: () => resumeNativeSurfaceOwner(owner, sessionId),
        onRetryError: (retryError) => {
          console.error(
            `[browser] degraded navigation cleanup hide failed (session=${sessionId})`,
            retryError,
          );
        },
      });
    }
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
    nativeSurfaceCoordinator.intentOwner = null;
    return nativeSurfaceCoordinator.pending || Promise.resolve();
  }
  return claimNativeSurfaceHide(owner, sessionId);
}

function BrowserIconButton({ title, icon, onClick, disabled = false, className }) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      className={className}
      onClick={onClick}
      disabled={disabled}
      style={disabled ? { opacity: 0.35 } : undefined}
    >
      {icon}
    </button>
  );
}

// This component coordinates native-surface ownership, lifecycle events, and toolbar state.
// Splitting those state machines is a dedicated refactor; their behavior is pinned by browser tests.
// eslint-disable-next-line sonarjs/cognitive-complexity -- keep the cross-process lifecycle in one audited component for this change
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
  const [persistenceWarningState, dispatchPersistenceWarning] = useReducer(
    persistenceWarningReducer,
    EMPTY_PERSISTENCE_WARNING,
  );
  const [controlOwner, setControlOwner] = useState(null);
  const [controlRevision, setControlRevision] = useState(null);
  const wheelRef = useRef(null);
  const urlInputRef = useRef(null);
  const committedUrlRef = useRef('');
  const urlInputFocusedRef = useRef(false);
  const urlInputDirtyRef = useRef(false);
  const addressOwnerSessionIdRef = useRef(sessionId);
  const activeSessionRef = useRef(null);
  const sessionIdRef = useRef(sessionId);
  const statusRequestEpochRef = useRef(0);
  const tabsRequestEpochRef = useRef(0);
  const lifecycleEventEpochRef = useRef(0);
  const tabsEventEpochRef = useRef(0);
  const controlEventEpochRef = useRef(0);
  const errorEventEpochRef = useRef(0);
  const persistenceEventEpochRef = useRef(0);
  const controlRevisionRef = useRef(null);
  const navigationRequestEpochRef = useRef(0);
  const pendingNavigationRef = useRef(null);
  const navigationWatchdogRef = useRef(0);
  const browserViewMountedRef = useRef(true);
  const navigationClientId = useId();
  const beginErrorOperation = useCallback(() => {
    const operationEpoch = errorEventEpochRef.current + 1;
    errorEventEpochRef.current = operationEpoch;
    setError('');
    return operationEpoch;
  }, []);
  const reportErrorForOperation = useCallback((operationEpoch, operationError) => {
    // A newer command or host event owns the banner. A late failure from an
    // older invoke must not overwrite that newer success/error state.
    if (
      !browserViewMountedRef.current
      || errorEventEpochRef.current !== operationEpoch
    ) return false;
    errorEventEpochRef.current += 1;
    setError(typeof operationError === 'string' ? operationError : String(operationError));
    return true;
  }, []);
  const clearNavigationWatchdog = useCallback(() => {
    if (!navigationWatchdogRef.current) return;
    window.clearTimeout(navigationWatchdogRef.current);
    navigationWatchdogRef.current = 0;
  }, []);
  const publishCommittedUrl = useCallback((nextUrl, ownerSessionId = sessionIdRef.current) => {
    if (ownerSessionId !== addressOwnerSessionIdRef.current) return false;
    const committedUrl = nextUrl || '';
    committedUrlRef.current = committedUrl;
    setUrl(committedUrl);
    if (shouldHydrateBrowserAddressInput({
      focused: urlInputFocusedRef.current,
      dirty: urlInputDirtyRef.current,
    })) {
      setUrlInput(browserAddressValue(committedUrl));
    }
    return true;
  }, []);
  const persistenceWarning = visiblePersistenceWarning(persistenceWarningState);
  const showingNewTab = running && isInternalBlankPageUrl(url);
  const nativeSurfaceReady = shouldShowNativeBrowserSurface({
    statusResolved: initialStatusResolved,
    running,
    url,
    suspended: nativeSurfaceSuspended,
  });
  const shouldSuspendNativeSurface = !nativeSurfaceReady;

  useEffect(() => {
    browserViewMountedRef.current = true;
    return () => {
      browserViewMountedRef.current = false;
      statusRequestEpochRef.current += 1;
      tabsRequestEpochRef.current += 1;
      errorEventEpochRef.current += 1;
    };
  }, []);

  useLayoutEffect(() => {
    sessionIdRef.current = sessionId;
    if (addressOwnerSessionIdRef.current === sessionId) return;
    addressOwnerSessionIdRef.current = sessionId;
    urlInputFocusedRef.current = false;
    urlInputDirtyRef.current = false;
    committedUrlRef.current = '';
    activeSessionRef.current = null;
    urlInputRef.current?.blur();
    setUrl('');
    setUrlInput('');
    setActiveSession(null);
  }, [sessionId]);

  useEffect(() => {
    const resume = () => setSurfaceEpoch((epoch) => epoch + 1);
    nativeSurfaceResumeListeners.add(resume);
    return () => {
      nativeSurfaceResumeListeners.delete(resume);
      navigationRequestEpochRef.current += 1;
      pendingNavigationRef.current = null;
      clearNavigationWatchdog();
    };
  }, [clearNavigationWatchdog, sessionId]);

  // React owns the toolbar while the system WebView owns the page. There is no
  // screenshot fallback: creation failures must remain visible to the user.
  useEffect(() => {
    const host = wheelRef.current;
    if (!host || !sessionId) return;
    const surfaceOwner = Symbol(`browser-surface:${sessionId}`);
    if (shouldSuspendNativeSurface) {
      observeNativeSurfaceHide(
        claimNativeSurfaceHide(surfaceOwner, sessionId),
        'suspend',
      );
      return () => {
        observeNativeSurfaceHide(
          releaseNativeSurface(surfaceOwner, sessionId),
          'suspended effect cleanup',
        );
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
          observeNativeSurfaceHide(
            claimNativeSurfaceHide(surfaceOwner, sessionId),
            'show unavailable',
          );
        }
      } catch {
        if (!disposed && nativeSurfaceCoordinator.owner === surfaceOwner) {
          setNativeAvailable(false);
          observeNativeSurfaceHide(
            claimNativeSurfaceHide(surfaceOwner, sessionId),
            'show failure cleanup',
          );
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
      observeNativeSurfaceHide(
        releaseNativeSurface(surfaceOwner, sessionId),
        'visible effect cleanup',
      );
    };
  }, [sessionId, surfaceEpoch, shouldSuspendNativeSurface]);

  // ---- State synchronization ----
  const refreshStatus = useCallback(async ({ preserveError = false } = {}) => {
    const requestedSessionId = sessionId;
    if (!browserViewMountedRef.current) return 'stale';
    const requestEpoch = statusRequestEpochRef.current + 1;
    statusRequestEpochRef.current = requestEpoch;
    const lifecycleEventEpoch = lifecycleEventEpochRef.current;
    const controlEventEpoch = controlEventEpochRef.current;
    const errorEventEpoch = errorEventEpochRef.current;
    const persistenceEventEpoch = persistenceEventEpochRef.current;
    const isCurrent = () => (
      browserViewMountedRef.current
      && sessionIdRef.current === requestedSessionId
      && statusRequestEpochRef.current === requestEpoch
    );
    try {
      const statusStartedAt = browserPerformanceNow();
      const st = await invokeTauri('browser_status', { sessionId: requestedSessionId });
      recordBrowserPerformance(
        'workspace_restore_status_ms',
        browserPerformanceNow() - statusStartedAt,
      );
      if (!isCurrent() || st?.sessionId !== requestedSessionId) return 'stale';
      if (isBrowserSnapshotDomainCurrent(
        lifecycleEventEpoch,
        lifecycleEventEpochRef.current,
      )) {
        setRunning(!!st.running);
        const nextActiveTab = st.activeTab || null;
        const reconciledPendingNavigation = reconcilePendingNavigationWithActiveTab(
          pendingNavigationRef.current,
          nextActiveTab,
        );
        pendingNavigationRef.current = reconciledPendingNavigation;
        if (reconciledPendingNavigation == null) clearNavigationWatchdog();
        // A navigation intent publishes its address before dispatch. Until a
        // Finished event or dispatch failure settles it, even a newer status
        // call still contains the previous committed URL and must not flash it
        // back into the address bar. Switching tabs, however, detaches that
        // intent from the newly active tab and immediately resumes hydration.
        if (reconciledPendingNavigation == null) {
          publishCommittedUrl(st.url, requestedSessionId);
        }
        activeSessionRef.current = nextActiveTab;
        setActiveSession(nextActiveTab);
      }
      if (isBrowserSnapshotDomainCurrent(
        controlEventEpoch,
        controlEventEpochRef.current,
      )) {
        const nextRevision = Number.isFinite(st.controlRevision) ? st.controlRevision : null;
        if (
          nextRevision == null
          || isMonotonicControlRevision(controlRevisionRef.current, nextRevision)
        ) {
          controlRevisionRef.current = nextRevision;
          setControlOwner(st.controlOwner || null);
          setControlRevision(nextRevision);
        }
      }
      if (isPersistenceStatusCurrent(
        persistenceEventEpoch,
        persistenceEventEpochRef.current,
      )) {
        dispatchPersistenceWarning({ type: 'hydrate', message: st.persistenceWarning || '' });
      }
      // Keep genuine restore failures visible in the dock. main.jsx keeps a normally
      // absent workspace closed, so it must not look like an error or auto-expand.
      if (
        !preserveError
        && isBrowserSnapshotDomainCurrent(errorEventEpoch, errorEventEpochRef.current)
      ) {
        setError(st.restoreError || '');
      }
      setInitialStatusResolved(true);
      return 'success';
    } catch (e) {
      if (!isCurrent()) return 'stale';
      if (isBrowserSnapshotDomainCurrent(
        lifecycleEventEpoch,
        lifecycleEventEpochRef.current,
      )) {
        setRunning(false);
      }
      if (isBrowserSnapshotDomainCurrent(
        controlEventEpoch,
        controlEventEpochRef.current,
      )) {
        controlRevisionRef.current = null;
        setControlOwner(null);
        setControlRevision(null);
      }
      // A failed status RPC is not evidence that a previously reported
      // persistence failure was repaired. Keep the warning until a successful
      // empty snapshot or browser:persistence-restored explicitly clears it.
      if (
        !preserveError
        && isBrowserSnapshotDomainCurrent(errorEventEpoch, errorEventEpochRef.current)
      ) {
        setError(typeof e === 'string' ? e : String(e));
      }
      setInitialStatusResolved(true);
      return 'failed';
    }
  }, [clearNavigationWatchdog, publishCommittedUrl, sessionId]);
  const refreshTabs = useCallback(async () => {
    const requestedSessionId = sessionId;
    if (!browserViewMountedRef.current) return;
    const requestEpoch = tabsRequestEpochRef.current + 1;
    tabsRequestEpochRef.current = requestEpoch;
    const tabsEventEpoch = tabsEventEpochRef.current;
    try {
      const list = await invokeTauri('browser_list_tabs', { sessionId: requestedSessionId });
      if (
        !browserViewMountedRef.current
        || sessionIdRef.current !== requestedSessionId
        || tabsRequestEpochRef.current !== requestEpoch
        || !isBrowserSnapshotDomainCurrent(tabsEventEpoch, tabsEventEpochRef.current)
      ) return;
      setTabs(list || []);
    } catch {
      /* The browser may not be ready yet. */
    }
  }, [sessionId]);
  const retryStatus = useCallback(() => {
    errorEventEpochRef.current += 1;
    setError('');
    setInitialStatusResolved(false);
    void refreshStatus();
    void refreshTabs();
  }, [refreshStatus, refreshTabs]);

  useEffect(() => {
    let disposed = false;
    let reconciliationTimer = 0;
    let statusRetryTimer = 0;
    let listenerRegistrationFailed = false;
    const unsubs = [];
    const registrations = [];
    // listenTauri may resolve after unmount. Unsubscribe immediately instead of
    // adding the callback to an inactive list and leaking updates into this component.
    const guard = (eventName, promise) => {
      const registration = Promise.resolve(promise).then((u) => {
      if (disposed) u && u();
      else unsubs.push(u);
      }).catch((registrationError) => {
        if (disposed) return;
        listenerRegistrationFailed = true;
        console.error(`[browser] failed to register ${eventName} listener`, registrationError);
      });
      registrations.push(registration);
    };
    guard('browser:navigation', listenTauri('browser:navigation', (e) => {
      if (disposed || !browserViewMountedRef.current) return;
      // Only the active tab may update the address bar. A background frameNavigated
      // event must not make openExternal target the wrong tab.
      const p = e.payload || {};
      if (p.sessionId !== sessionId) return;
      const pendingNavigation = pendingNavigationRef.current;
      const settlesPendingNavigation = navigationEventSettlesPending(
        pendingNavigation,
        p.tab,
        p.requestId,
        activeSessionRef.current,
      );
      if (settlesPendingNavigation) {
        pendingNavigationRef.current = null;
        clearNavigationWatchdog();
      }
      if (!p.url || !eventTargetsActiveBrowserTab(p.tab, activeSessionRef.current)) {
        // During initial restore there is not yet an authoritative active tab.
        // Do not guess from the first event (which may belong to a background
        // tab); ask the host for the active mapping instead.
        if (activeSessionRef.current == null) {
          lifecycleEventEpochRef.current += 1;
          refreshStatus();
        }
        return;
      }
      // A Finished event from an older request on this same tab may arrive
      // after a newer optimistic intent. Keep the newer address until its
      // requestId is committed by the host.
      if (pendingNavigation && !settlesPendingNavigation) return;
      lifecycleEventEpochRef.current += 1;
      publishCommittedUrl(p.url, sessionId);
      refreshTabs();
    }));
    guard('browser:tabs-changed', listenTauri('browser:tabs-changed', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      if (event.payload?.sessionId !== sessionId) return;
      tabsEventEpochRef.current += 1;
      lifecycleEventEpochRef.current += 1;
      refreshTabs();
      // Rust may recover to another tab after MCP closes the active one.
      refreshStatus();
    }));
    guard('browser:tab-title', listenTauri('browser:tab-title', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      if (!payload.tab || !payload.title) return;
      tabsEventEpochRef.current += 1;
      setTabs((current) => current.map((tab) => (
        tab.target_id === payload.tab ? { ...tab, title: payload.title } : tab
      )));
      // The event may arrive while the first list_tabs call is in flight. Its
      // epoch was just invalidated, and the local merge is a no-op if that tab
      // has not hydrated yet, so fetch the host's URL-paired title snapshot.
      refreshTabs();
    }));
    guard('browser:activated', listenTauri('browser:activated', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      if (event.payload?.sessionId !== sessionId) return;
      lifecycleEventEpochRef.current += 1;
      tabsEventEpochRef.current += 1;
      refreshStatus();
      refreshTabs();
      setSurfaceEpoch((epoch) => epoch + 1);
    }));
    guard('browser:stopped', listenTauri('browser:stopped', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      if (event.payload?.sessionId && event.payload.sessionId !== sessionId) return;
      statusRequestEpochRef.current += 1;
      tabsRequestEpochRef.current += 1;
      lifecycleEventEpochRef.current += 1;
      tabsEventEpochRef.current += 1;
      controlEventEpochRef.current += 1;
      errorEventEpochRef.current += 1;
      persistenceEventEpochRef.current += 1;
      pendingNavigationRef.current = null;
      clearNavigationWatchdog();
      activeSessionRef.current = null;
      controlRevisionRef.current = null;
      setRunning(false);
      setActiveSession(null);
      setError('');
      setControlOwner(null);
      setControlRevision(null);
      dispatchPersistenceWarning({ type: 'clear' });
      setInitialStatusResolved(true);
    }));
    guard('browser:control-changed', listenTauri('browser:control-changed', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      // WorkspaceControl is shared by every tab. tabToken identifies the
      // causal tab for diagnostics/lease checks; it does not scope ownership.
      if (!isMonotonicControlRevision(controlRevisionRef.current, payload.revision)) return;
      controlEventEpochRef.current += 1;
      controlRevisionRef.current = payload.revision;
      setControlOwner(payload.owner || null);
      setControlRevision(payload.revision);
    }));
    guard('browser:navigation-blocked', listenTauri('browser:navigation-blocked', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      const settlesPendingNavigation = navigationEventSettlesPending(
        pendingNavigationRef.current,
        payload.tab,
        payload.requestId,
        activeSessionRef.current,
      );
      if (settlesPendingNavigation) {
        pendingNavigationRef.current = null;
        clearNavigationWatchdog();
        lifecycleEventEpochRef.current += 1;
      }
      errorEventEpochRef.current += 1;
      const scheme = payload.scheme ? ` (${payload.scheme})` : '';
      setError(`${t.browserBlockedNavigation}${scheme}`);
      if (settlesPendingNavigation) {
        // Restore the last committed address without immediately clearing the
        // security error that explains why the optimistic target was rejected.
        refreshStatus({ preserveError: true });
      }
    }));
    guard('browser:automation-unavailable', listenTauri('browser:automation-unavailable', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      errorEventEpochRef.current += 1;
      setError(t.browserAutomationUnavailable);
    }));
    guard('browser:download-blocked', listenTauri('browser:download-blocked', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      errorEventEpochRef.current += 1;
      setError(t.browserDownloadBlocked(payload.source || ''));
    }));
    guard('browser:persistence-warning', listenTauri('browser:persistence-warning', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      const payload = event.payload || {};
      if (payload.sessionId !== sessionId) return;
      persistenceEventEpochRef.current += 1;
      dispatchPersistenceWarning({
        type: 'report',
        message: payload.error || t.browserPersistenceWarning,
      });
    }));
    guard('browser:persistence-restored', listenTauri('browser:persistence-restored', (event) => {
      if (disposed || !browserViewMountedRef.current) return;
      if (event.payload?.sessionId !== sessionId) return;
      persistenceEventEpochRef.current += 1;
      dispatchPersistenceWarning({ type: 'clear' });
    }));

    // Register every event stream before the first snapshot. Otherwise a
    // transition between an early status response and a late listener ACK can
    // be lost permanently. Failed registration degrades to bounded polling.
    void awaitBrowserListenerReadiness(registrations, {
      schedule: window.setTimeout.bind(window),
      cancel: window.clearTimeout.bind(window),
    }).then((listenersReady) => {
      if (disposed) return;
      if (!listenersReady) {
        listenerRegistrationFailed = true;
        console.error('[browser] listener registration timed out; enabling reconciliation');
      }
      const hydrateInitialStatus = async (failedAttempt = 0) => {
        const outcome = await refreshStatus();
        if (disposed || outcome !== 'failed' || listenerRegistrationFailed) return;
        const retryDelay = browserStatusRetryDelay(failedAttempt);
        if (retryDelay == null) return;
        statusRetryTimer = window.setTimeout(() => {
          statusRetryTimer = 0;
          void hydrateInitialStatus(failedAttempt + 1);
        }, retryDelay);
      };
      void hydrateInitialStatus();
      void refreshTabs();
      if (listenerRegistrationFailed) {
        reconciliationTimer = window.setInterval(() => {
          void refreshStatus();
          void refreshTabs();
        }, 2000);
      }
    });
    return () => {
      disposed = true;
      if (reconciliationTimer) window.clearInterval(reconciliationTimer);
      if (statusRetryTimer) window.clearTimeout(statusRetryTimer);
      unsubs.forEach((unsubscribe) => {
        if (unsubscribe) unsubscribe();
      });
    };
  }, [
    refreshStatus,
    refreshTabs,
    clearNavigationWatchdog,
    publishCommittedUrl,
    sessionId,
    t,
    t.browserAutomationUnavailable,
    t.browserBlockedNavigation,
    t.browserDownloadBlocked,
    t.browserPersistenceWarning,
  ]);

  // ---- Navigation ----
  const navigate = useCallback(async (raw) => {
    let target = (raw || '').trim();
    if (!target) return;
    // The host dispatches against its active tab. Until the first status has
    // supplied that identity, an event cannot be correlated safely and may
    // leave an optimistic address pending forever.
    if (activeSessionRef.current == null) {
      refreshStatus();
      return;
    }
    if (!/^https?:\/\//i.test(target) && target !== 'about:blank') {
      target = 'https://' + target;
    }
    const navigationEpoch = navigationRequestEpochRef.current + 1;
    navigationRequestEpochRef.current = navigationEpoch;
    const requestId = `${navigationClientId}-${navigationEpoch}`;
    const fragmentOnly = isFragmentOnlyBrowserNavigation(url, target);
    clearNavigationWatchdog();
    pendingNavigationRef.current = fragmentOnly
      ? null
      : {
        epoch: navigationEpoch,
        tab: activeSessionRef.current,
        requestId,
      };
    if (!fragmentOnly) {
      navigationWatchdogRef.current = window.setTimeout(() => {
        if (
          !browserViewMountedRef.current
          || pendingNavigationRef.current?.requestId !== requestId
        ) return;
        pendingNavigationRef.current = null;
        navigationWatchdogRef.current = 0;
        lifecycleEventEpochRef.current += 1;
        refreshStatus();
        refreshTabs();
      }, NAVIGATION_PENDING_TIMEOUT_MS);
    }
    const errorOperationEpoch = beginErrorOperation();
    try {
      await dispatchBrowserNavigation({
        target: browserAddressValue(target),
        publishInput: (address) => {
          lifecycleEventEpochRef.current += 1;
          setUrlInput(address);
          if (fragmentOnly) publishCommittedUrl(target, sessionId);
        },
        dispatch: () => invokeTauri('browser_navigate', {
          sessionId,
          url: target,
          requestId,
        }),
      });
    } catch (e) {
      if (navigationRequestEpochRef.current !== navigationEpoch) return;
      pendingNavigationRef.current = null;
      clearNavigationWatchdog();
      if (reportErrorForOperation(errorOperationEpoch, e)) {
        refreshStatus({ preserveError: true });
      }
    }
  }, [
    beginErrorOperation,
    clearNavigationWatchdog,
    navigationClientId,
    reportErrorForOperation,
    sessionId,
    refreshStatus,
    refreshTabs,
    publishCommittedUrl,
    url,
  ]);

  const runNav = useCallback(async (cmd) => {
    navigationRequestEpochRef.current += 1;
    pendingNavigationRef.current = null;
    clearNavigationWatchdog();
    lifecycleEventEpochRef.current += 1;
    const errorOperationEpoch = beginErrorOperation();
    try {
      await invokeTauri(cmd, { sessionId });
      refreshStatus();
      refreshTabs();
    } catch (e) {
      reportErrorForOperation(errorOperationEpoch, e);
    }
  }, [
    beginErrorOperation,
    clearNavigationWatchdog,
    reportErrorForOperation,
    sessionId,
    refreshStatus,
    refreshTabs,
  ]);

  const openExternal = useCallback(async () => {
    if (!url || isInternalBlankPageUrl(url)) return;
    const errorOperationEpoch = beginErrorOperation();
    try {
      await invokeTauri('open_user_external_url', { url });
    } catch (e) {
      reportErrorForOperation(errorOperationEpoch, e);
    }
  }, [beginErrorOperation, reportErrorForOperation, url]);

  // ---- Tabs ----
  const createTab = useCallback(async () => {
    const errorOperationEpoch = beginErrorOperation();
    try {
      await invokeTauri('browser_create_tab', { sessionId, url: HOME_URL, background: false });
      refreshTabs();
      // create_tab does not emit browser:navigation for about:blank, so refresh state
      // after activation or the address bar and external-open URL stay on the old tab.
      refreshStatus();
    } catch (e) {
      reportErrorForOperation(errorOperationEpoch, e);
    }
  }, [beginErrorOperation, reportErrorForOperation, sessionId, refreshTabs, refreshStatus]);

  const closeTab = useCallback(
    async (targetId) => {
      const errorOperationEpoch = beginErrorOperation();
      try {
        await invokeTauri('browser_close_tab', { sessionId, targetId });
        refreshTabs();
        refreshStatus();
      } catch (e) {
        reportErrorForOperation(errorOperationEpoch, e);
      }
    },
    [beginErrorOperation, reportErrorForOperation, sessionId, refreshTabs, refreshStatus]
  );

  const activateTab = useCallback(
    async (targetId) => {
      const errorOperationEpoch = beginErrorOperation();
      try {
        const switchStartedAt = browserPerformanceNow();
        await invokeTauri('browser_activate_tab', { sessionId, targetId });
        if (
          !browserViewMountedRef.current
          || sessionIdRef.current !== sessionId
        ) return;
        recordBrowserPerformance('tab_switch_ms', browserPerformanceNow() - switchStartedAt);
        pendingNavigationRef.current = reconcilePendingNavigationWithActiveTab(
          pendingNavigationRef.current,
          targetId,
        );
        activeSessionRef.current = targetId;
        setActiveSession(targetId);
        // activate_tab switches the native child view without browser:navigation.
        // Refresh state so the address bar and openExternal follow the selected tab.
        refreshStatus();
      } catch (e) {
        reportErrorForOperation(errorOperationEpoch, e);
      }
    },
    [beginErrorOperation, reportErrorForOperation, sessionId, refreshStatus]
  );

  const stopBrowser = useCallback(async () => {
    const errorOperationEpoch = beginErrorOperation();
    try {
      await invokeTauri('browser_stop', { sessionId });
      if (
        !browserViewMountedRef.current
        || sessionIdRef.current !== sessionId
      ) return;
      // The command response and browser:stopped event use independent IPC
      // deliveries. Invalidate every older status response before publishing
      // the local success so a delayed running=true snapshot cannot reopen it.
      statusRequestEpochRef.current += 1;
      lifecycleEventEpochRef.current += 1;
      setRunning(false);
    } catch (e) {
      reportErrorForOperation(errorOperationEpoch, e);
    }
  }, [beginErrorOperation, reportErrorForOperation, sessionId]);

  const handBackToAgent = useCallback(async () => {
    const errorOperationEpoch = beginErrorOperation();
    try {
      const control = await invokeTauri('browser_hand_back_to_agent', { sessionId });
      if (
        !browserViewMountedRef.current
        || sessionIdRef.current !== sessionId
      ) return;
      const nextRevision = Number.isFinite(control?.controlRevision)
        ? control.controlRevision
        : null;
      if (isMonotonicControlRevision(controlRevisionRef.current, nextRevision)) {
        controlEventEpochRef.current += 1;
        controlRevisionRef.current = nextRevision;
        setControlOwner(control?.controlOwner || 'agent');
        setControlRevision(nextRevision);
      } else {
        refreshStatus();
      }
    } catch (e) {
      reportErrorForOperation(errorOperationEpoch, e);
    }
  }, [beginErrorOperation, reportErrorForOperation, refreshStatus, sessionId]);

  const handleAddressFocus = useCallback(() => {
    urlInputFocusedRef.current = true;
  }, []);

  const handleAddressChange = useCallback((event) => {
    urlInputDirtyRef.current = true;
    setUrlInput(event.target.value);
  }, []);
  const handleAddressBlur = useCallback(() => {
    urlInputFocusedRef.current = false;
    if (!urlInputDirtyRef.current) return;
    urlInputDirtyRef.current = false;
    setUrlInput(browserAddressValue(committedUrlRef.current));
  }, []);
  const handleAddressSubmit = useCallback(() => {
    // A submitted value becomes the user's committed navigation intent. It is
    // now safe for Finished/status hydration to publish the canonical redirect
    // (for example an HTTP -> HTTPS upgrade) while the input remains focused.
    urlInputDirtyRef.current = false;
    void navigate(urlInput);
  }, [navigate, urlInput]);

  // ---- Rendering ----
  const shell = 'flex h-full flex-col overflow-hidden';
  const toolbarCls = `flex shrink-0 items-center gap-1 border-b px-2 py-1.5 ${
    isDark ? 'border-[#2A2B2E] bg-[#17181A]' : 'border-[#E5E7EB] bg-[#F8F9FA]'
  }`;
  const btnCls = `rounded-md p-1.5 transition-colors ${
    isDark ? 'text-[#B8B8B8] hover:bg-[#2A2B2E] hover:text-[#F2F2F2]' : 'text-[#555] hover:bg-[#ECECEC] hover:text-[#111]'
  }`;
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
      {/* Keep tabs above the address bar, matching desktop browsers and Codex. */}
      {tabs.length > 0 && (
        <div
          className={`flex shrink-0 items-center gap-1 overflow-x-auto px-2 py-1 ${
            isDark ? 'border-b border-[#2A2B2E] bg-[#1A1B1D]' : 'border-b border-[#E5E7EB] bg-white'
          }`}
        >
          {tabs.map((tab) => {
            const active = tab.target_id === activeSession;
            return (
              /* biome-ignore lint/a11y/useSemanticElements: the composite tab contains a nested close button, so the outer activator cannot itself be a button */
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
          <BrowserIconButton
            title={t.browserNewTab}
            icon={<Plus size={15} />}
            onClick={createTab}
            className={btnCls}
          />
        </div>
      )}

      {/* Toolbar */}
      <div className={toolbarCls}>
        <BrowserIconButton
          title={t.browserBack}
          icon={<ChevronLeft size={17} />}
          onClick={() => runNav('browser_back')}
          className={btnCls}
        />
        <BrowserIconButton
          title={t.browserForward}
          icon={<ChevronRight size={17} />}
          onClick={() => runNav('browser_forward')}
          className={btnCls}
        />
        <BrowserIconButton
          title={t.browserRefresh}
          icon={<RefreshCw size={16} />}
          onClick={() => runNav('browser_reload')}
          className={btnCls}
        />
        <form
          className="mx-1 flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1"
          style={{
            background: isDark ? '#232428' : '#FFFFFF',
            border: `1px solid ${isDark ? '#3A3B3F' : '#D8DADC'}`,
          }}
          onSubmit={(e) => {
            e.preventDefault();
            handleAddressSubmit();
          }}
        >
          <Globe size={14} style={{ opacity: 0.5 }} />
          <input
            ref={urlInputRef}
            className="w-full bg-transparent text-[13px] outline-none"
            style={{ color: isDark ? '#E8E8E8' : '#222' }}
            placeholder={t.browserUrlPlaceholder}
            value={urlInput}
            onFocus={handleAddressFocus}
            onChange={handleAddressChange}
            onBlur={handleAddressBlur}
            disabled={!activeSession}
            // Enter used to confirm an IME candidate must not submit navigation.
            // macOS WKWebView bug 165004 can clear isComposing while retaining
            // keyCode 229, so use the shared guard tested by ime_compose_guard.
            onKeyDown={(e) => { if (e.key === 'Enter' && isImeComposing(e)) e.preventDefault(); }}
            spellCheck={false}
            data-testid="browser-url-input"
          />
        </form>
        {!ownershipSlot && ownershipControl}
        <BrowserIconButton
          title={t.browserOpenExternal}
          icon={<ExternalLink size={15} />}
          onClick={openExternal}
          disabled={!url || isInternalBlankPageUrl(url)}
          className={btnCls}
        />
        <BrowserIconButton
          title={t.browserStop}
          icon={<XIcon size={16} />}
          onClick={stopBrowser}
          className={btnCls}
        />
      </div>

      {/* The native child WebView always paints above React. Keep error banners in a
          separate layout row so they remain visible and ResizeObserver shifts bounds. */}
      {running && error && (
        <button
          type="button"
          data-testid="browser-error-banner"
          onClick={() => {
            errorEventEpochRef.current += 1;
            setError('');
          }}
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
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">{t.browserPersistenceWarning}</div>
            <button
              type="button"
              data-testid="browser-persistence-warning-dismiss"
              title={t.winClose}
              aria-label={t.winClose}
              className="shrink-0 rounded p-0.5 transition-colors hover:bg-black/10 dark:hover:bg-white/10"
              onClick={() => dispatchPersistenceWarning({ type: 'dismiss' })}
            >
              <XIcon size={13} />
            </button>
          </div>
          <div className="mt-1" style={{ opacity: 0.75, wordBreak: 'break-all' }}>{persistenceWarning}</div>
        </div>
      )}

      {/* The native page covers this slot; React owns only state and error feedback. */}
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
                <button
                  type="button"
                  data-testid="browser-status-retry"
                  className={`mt-4 rounded-md border px-3 py-1.5 ${isDark ? 'border-white/15 hover:bg-white/10' : 'border-black/15 hover:bg-black/5'}`}
                  onClick={retryStatus}
                >
                  {t.browserRetry}
                </button>
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
