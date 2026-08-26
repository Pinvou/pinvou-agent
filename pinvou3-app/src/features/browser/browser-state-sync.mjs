export function isBrowserSnapshotDomainCurrent(requestEventEpoch, currentEventEpoch) {
  return requestEventEpoch === currentEventEpoch;
}

// A session command can synchronously publish the bridge target before its
// promise settles. That snapshot is an acknowledgement of the command already
// holding the serialized native-surface ticket, not a second navigation
// request. Keep command tokens by identity so a same-id ABA completion cannot
// clear a newer request. Observing a genuinely different bridge target clears
// the echo set, which preserves B -> C -> B latest-wins ordering.
export function createBrowserSessionCommandEchoGuard() {
  let sequence = 0;
  const commands = [];
  const normalizeTarget = (target) => target == null ? null : String(target);

  return {
    begin(target, baselineTarget) {
      const token = {
        id: sequence + 1,
        target: normalizeTarget(target),
        baselineTarget: normalizeTarget(baselineTarget),
        targetObserved: false,
      };
      sequence = token.id;
      commands.push(token);
      return token;
    },

    observe(target) {
      const normalizedTarget = normalizeTarget(target);
      for (let index = commands.length - 1; index >= 0; index -= 1) {
        const command = commands[index];
        if (command.target !== normalizedTarget) continue;
        command.targetObserved = true;
        return { type: 'command-echo', token: command };
      }

      // Other bridge domains can notify while a command is waiting for its
      // first session mutation. An unchanged baseline is not a divergence.
      if (commands.some((command) => (
        !command.targetObserved && command.baselineTarget === normalizedTarget
      ))) {
        return { type: 'baseline' };
      }

      commands.length = 0;
      return { type: 'external' };
    },

    settle(token) {
      const index = commands.indexOf(token);
      if (index < 0) return false;
      commands.splice(index, 1);
      return true;
    },

    isPending(token) {
      return commands.includes(token);
    },
  };
}

export function shouldHydrateBrowserAddressInput({ focused, dirty }) {
  return !(focused && dirty);
}

const DEFAULT_BROWSER_STATUS_RETRY_DELAYS_MS = Object.freeze([250, 750, 1500]);

export function browserStatusRetryDelay(
  failedAttempt,
  delays = DEFAULT_BROWSER_STATUS_RETRY_DELAYS_MS,
) {
  if (!Number.isSafeInteger(failedAttempt) || failedAttempt < 0) {
    throw new TypeError('failedAttempt must be a non-negative safe integer');
  }
  if (!Array.isArray(delays)) throw new TypeError('delays must be an array');
  const delay = delays[failedAttempt];
  return Number.isFinite(delay) && delay >= 0 ? delay : null;
}

// Lifecycle discovery in app/main tracks every workspace, not just the
// currently rendered BrowserView. A scalar epoch is too broad: an event for a
// background workspace would invalidate the current workspace's first status
// snapshot. This tracker keeps per-session epochs while retaining one global
// generation for the host's legacy "all sessions stopped" event.
export function createBrowserSessionEpochTracker() {
  let globalEpoch = 0;
  const sessionEpochs = new Map();
  const isSessionId = (sessionId) => (
    typeof sessionId === 'string' && sessionId.length > 0
  );
  const snapshot = (sessionId) => ({
    global: globalEpoch,
    session: isSessionId(sessionId) ? (sessionEpochs.get(sessionId) || 0) : 0,
  });

  return {
    advance(sessionId = null) {
      if (!isSessionId(sessionId)) {
        globalEpoch += 1;
        sessionEpochs.clear();
      } else {
        sessionEpochs.set(sessionId, (sessionEpochs.get(sessionId) || 0) + 1);
      }
      return snapshot(sessionId);
    },

    snapshot,

    isCurrent(sessionId, captured) {
      if (!captured || typeof captured !== 'object') return false;
      const current = snapshot(sessionId);
      return captured.global === current.global && captured.session === current.session;
    },
  };
}

// Listener registration is IPC-backed and normally settles immediately, but a
// wedged renderer bridge must not leave the browser UI waiting forever before
// its first status hydration. A timeout means "degraded": callers hydrate and
// start reconciliation polling while any late listener still owns its normal
// cleanup path.
export async function awaitBrowserListenerReadiness(
  registrations,
  {
    timeoutMs = 2000,
    schedule = globalThis.setTimeout,
    cancel = globalThis.clearTimeout,
  } = {},
) {
  if (!Array.isArray(registrations)) throw new TypeError('registrations must be an array');
  if (!Number.isFinite(timeoutMs) || timeoutMs < 0) {
    throw new TypeError('timeoutMs must be a non-negative number');
  }
  if (typeof schedule !== 'function') throw new TypeError('schedule must be a function');
  if (typeof cancel !== 'function') throw new TypeError('cancel must be a function');

  let timer;
  const registrationsSettled = Promise.allSettled(registrations).then(() => true);
  const deadline = new Promise((resolve) => {
    timer = schedule(() => resolve(false), timeoutMs);
  });
  const ready = await Promise.race([registrationsSettled, deadline]);
  if (ready) cancel(timer);
  return ready;
}

export function eventTargetsActiveBrowserTab(eventTab, activeTab) {
  return typeof eventTab === 'string'
    && eventTab.length > 0
    && typeof activeTab === 'string'
    && activeTab.length > 0
    && eventTab === activeTab;
}

export function isMonotonicControlRevision(currentRevision, incomingRevision) {
  return Number.isFinite(incomingRevision)
    && (!Number.isFinite(currentRevision) || incomingRevision >= currentRevision);
}

export function reconcilePendingNavigationWithActiveTab(pendingNavigation, activeTab) {
  if (!pendingNavigation) return null;
  if (typeof activeTab !== 'string' || activeTab.length === 0) return pendingNavigation;
  if (pendingNavigation.tab == null) return { ...pendingNavigation, tab: activeTab };
  return pendingNavigation.tab === activeTab ? pendingNavigation : null;
}

export function navigationEventSettlesPending(
  pendingNavigation,
  eventTab,
  eventRequestId,
  activeTab,
) {
  if (!pendingNavigation || typeof eventTab !== 'string' || eventTab.length === 0) return false;
  const targetsPendingTab = pendingNavigation.tab != null
    ? pendingNavigation.tab === eventTab
    : eventTargetsActiveBrowserTab(eventTab, activeTab);
  return targetsPendingTab
    && typeof eventRequestId === 'string'
    && eventRequestId === pendingNavigation.requestId;
}

export function isFragmentOnlyBrowserNavigation(currentUrl, nextUrl) {
  if (typeof currentUrl !== 'string' || typeof nextUrl !== 'string' || currentUrl === nextUrl) {
    return false;
  }
  try {
    const current = new URL(currentUrl);
    const next = new URL(nextUrl);
    current.hash = '';
    next.hash = '';
    return current.href === next.href;
  } catch {
    return false;
  }
}
