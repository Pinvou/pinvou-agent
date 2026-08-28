export function isSubagentPanelPublicationCurrent({
  transitionCurrent,
  requestId,
  currentRequestId,
  sessionId,
  currentSessionId,
}) {
  return transitionCurrent === true
    && requestId === currentRequestId
    && sessionId === currentSessionId;
}

export function invokeObservedPanelSelection(select, args = [], onError = () => {}) {
  if (typeof select !== 'function') return false;
  try {
    const result = select(...args);
    if (!result || typeof result.then !== 'function') return result;
    return Promise.resolve(result).catch((error) => {
      onError(error);
      return false;
    });
  } catch (error) {
    onError(error);
    return false;
  }
}
