const EMPTY_BROWSER_PANE_STATE = Object.freeze({
  open: false,
  browserSelected: false,
  activation: 0,
});

const EMPTY_BROWSER_OPEN_STATE = Object.freeze({
  attempt: 0,
  status: 'idle',
  error: '',
});

export function browserPaneStateFor(statesBySession, sessionId) {
  if (!sessionId) return EMPTY_BROWSER_PANE_STATE;
  return statesBySession?.[sessionId] || EMPTY_BROWSER_PANE_STATE;
}

export function activateBrowserPane(statesBySession, sessionId) {
  if (!sessionId) return statesBySession;
  const current = browserPaneStateFor(statesBySession, sessionId);
  return {
    ...statesBySession,
    [sessionId]: {
      open: true,
      browserSelected: true,
      activation: current.activation + 1,
    },
  };
}

export function restoreBrowserPane(statesBySession, sessionId) {
  if (
    !sessionId
    || Object.prototype.hasOwnProperty.call(statesBySession || {}, sessionId)
  ) return statesBySession;
  return activateBrowserPane(statesBySession, sessionId);
}

export function selectArtifactsPane(statesBySession, sessionId) {
  if (!sessionId) return statesBySession;
  const current = browserPaneStateFor(statesBySession, sessionId);
  return {
    ...statesBySession,
    [sessionId]: {
      ...current,
      browserSelected: false,
    },
  };
}

export function closeBrowserPane(statesBySession, sessionId) {
  if (!sessionId) return statesBySession;
  const current = browserPaneStateFor(statesBySession, sessionId);
  return {
    ...statesBySession,
    [sessionId]: {
      ...current,
      open: false,
      browserSelected: false,
    },
  };
}

export function removeBrowserPaneState(statesBySession, sessionId) {
  if (!sessionId || !Object.prototype.hasOwnProperty.call(statesBySession || {}, sessionId)) {
    return statesBySession;
  }
  const next = { ...statesBySession };
  delete next[sessionId];
  return next;
}

export function browserOpenStateFor(statesBySession, sessionId) {
  if (!sessionId) return EMPTY_BROWSER_OPEN_STATE;
  return statesBySession?.[sessionId] || EMPTY_BROWSER_OPEN_STATE;
}

export function beginBrowserOpen(statesBySession, sessionId, attempt) {
  if (!sessionId) return statesBySession;
  return {
    ...statesBySession,
    [sessionId]: { attempt, status: 'starting', error: '' },
  };
}

export function settleBrowserOpen(
  statesBySession,
  sessionId,
  attempt,
  status,
  error = '',
) {
  if (!sessionId) return statesBySession;
  const current = browserOpenStateFor(statesBySession, sessionId);
  if (current.attempt !== attempt) return statesBySession;
  return {
    ...statesBySession,
    [sessionId]: { attempt, status, error },
  };
}
