// Pure helpers for the native Code session lifecycle: display resolution for the
// draft→session activation handoff, issue/commit gating for native control
// refreshes across session switches, and the creation pipeline that persists
// draft controls before a new session is exposed and loaded.

export function resolveNativeModelId({
  activeId,
  controlsSessionId,
  controlsModelId,
  draftModelId,
  handoffModelId,
}) {
  if (activeId && controlsSessionId === activeId) return controlsModelId || null;
  if (activeId) return handoffModelId || null;
  return draftModelId || null;
}

// Issues the sequence number for a control refresh. A refresh aimed at a session
// other than the active one is certain to be dropped by canApplyNativeControlsRefresh,
// so it must not consume a sequence number: otherwise a late request for a stale
// session (e.g. fired after an awaited control mutation) would supersede the active
// session's in-flight authoritative refresh, leaving its controls stuck on fallback
// values with no replacement request.
export function claimNativeControlsRefreshId({ sessionId, activeId, latestRequestId }) {
  if (sessionId !== activeId) return { requestId: 0, latestRequestId };
  const requestId = latestRequestId + 1;
  return { requestId, latestRequestId: requestId };
}

export function canApplyNativeControlsRefresh({
  requestId,
  latestRequestId,
  sessionId,
  activeId,
}) {
  return requestId === latestRequestId && sessionId === activeId;
}

export async function finalizePreparedSessionCreation({
  sessionId,
  prepareSession,
  shouldActivate,
  activateSession,
  loadSession,
  loadInactiveSessionInfo,
}) {
  let preparationError = null;
  try {
    if (prepareSession) await prepareSession(sessionId);
  } catch (err) {
    preparationError = err;
  }

  if (!shouldActivate()) {
    if (preparationError) throw preparationError;
    const info = loadInactiveSessionInfo ? await loadInactiveSessionInfo(sessionId) : null;
    return { id: sessionId, info, activated: false };
  }

  activateSession(sessionId);
  const info = await loadSession(sessionId);
  // Preparation can partially persist controls before failing. Loading the created
  // session first preserves that state and gives a retry a stable session target. Return
  // the error so the caller can rebind session-scoped operation state before reporting it.
  return { id: sessionId, info, activated: true, preparationError };
}
