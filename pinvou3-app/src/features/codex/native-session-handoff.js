// Pure state and lifecycle helpers for materializing a native Code draft into a session.

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
  // session first preserves that state and gives a retry a stable session target.
  if (preparationError) throw preparationError;
  return { id: sessionId, info, activated: true };
}
