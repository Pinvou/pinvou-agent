// Process-wide ownership and transition ordering for the single native browser surface.
// This state must load with the app because navigation can hide the WebView while the
// visual BrowserView is absent; the React view itself remains an on-demand chunk.
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  createDegradedNativeSurfaceHideLease,
  hideNativeSurfaceWithRetry,
  isFailedNativeSurfaceHideGenerationCurrent,
} from './native-surface-transition.mjs';

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

export function claimNativeSurfaceShow(owner, sessionId, bounds, boundsKey) {
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

export function claimNativeSurfaceHide(owner, sessionId) {
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

export function observeNativeSurfaceHide(promise, reason) {
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

export function ownsNativeSurfaceShow(owner, sessionId, boundsKey) {
  return nativeSurfaceCoordinator.owner === owner
    && nativeSurfaceCoordinator.desired === 'show'
    && sameNativeSurfaceTarget(sessionId, boundsKey);
}

export function releaseNativeSurface(owner, sessionId) {
  if (nativeSurfaceCoordinator.owner !== owner) return Promise.resolve();
  if (nativeSurfaceCoordinator.desired === 'hide') {
    nativeSurfaceCoordinator.owner = null;
    nativeSurfaceCoordinator.intentOwner = null;
    return nativeSurfaceCoordinator.pending || Promise.resolve();
  }
  return claimNativeSurfaceHide(owner, sessionId);
}

export function isNativeSurfaceOwner(owner) {
  return nativeSurfaceCoordinator.owner === owner;
}

export function subscribeNativeSurfaceResume(listener) {
  nativeSurfaceResumeListeners.add(listener);
  return () => nativeSurfaceResumeListeners.delete(listener);
}
