function uniquePanelIds(values) {
  const result = [];
  for (const value of values || []) {
    if (typeof value !== 'string' || !value || result.includes(value)) continue;
    result.push(value);
  }
  return result;
}

export function createRightDockState() {
  return {
    mountedPanelIds: [],
    visiblePanelStack: [],
    occlusionIds: [],
  };
}

export function mountRightDockPanel(state, panelId) {
  if (!panelId || state.mountedPanelIds.includes(panelId)) return state;
  return {
    ...state,
    mountedPanelIds: [...state.mountedPanelIds, panelId],
  };
}

export function unmountRightDockPanel(state, panelId) {
  if (!panelId) return state;
  const mountedPanelIds = state.mountedPanelIds.filter((id) => id !== panelId);
  const visiblePanelStack = state.visiblePanelStack.filter((id) => id !== panelId);
  if (
    mountedPanelIds.length === state.mountedPanelIds.length
    && visiblePanelStack.length === state.visiblePanelStack.length
  ) return state;
  return { ...state, mountedPanelIds, visiblePanelStack };
}

export function activateRightDockPanel(state, panelId) {
  if (!panelId) return state;
  const mountedPanelIds = state.mountedPanelIds.includes(panelId)
    ? state.mountedPanelIds
    : [...state.mountedPanelIds, panelId];
  const withoutPanel = state.visiblePanelStack.filter((id) => id !== panelId);
  const visiblePanelStack = [...withoutPanel, panelId];
  if (
    mountedPanelIds === state.mountedPanelIds
    && visiblePanelStack.length === state.visiblePanelStack.length
    && visiblePanelStack.every((id, index) => id === state.visiblePanelStack[index])
  ) return state;
  return { ...state, mountedPanelIds, visiblePanelStack };
}

export function hideRightDockPanel(state, panelId) {
  if (!panelId || !state.visiblePanelStack.includes(panelId)) return state;
  return {
    ...state,
    visiblePanelStack: state.visiblePanelStack.filter((id) => id !== panelId),
  };
}

export function setRightDockOcclusion(state, occlusionId, active) {
  if (!occlusionId) return state;
  const current = uniquePanelIds(state.occlusionIds);
  const next = active
    ? uniquePanelIds([...current, occlusionId])
    : current.filter((id) => id !== occlusionId);
  if (next.length === current.length && next.every((id, index) => id === current[index])) {
    return state;
  }
  return { ...state, occlusionIds: next };
}

export function rightDockSnapshot(state) {
  const occluded = state.occlusionIds.length > 0;
  const activePanelId = occluded
    ? null
    : (state.visiblePanelStack[state.visiblePanelStack.length - 1] || null);
  return {
    activePanelId,
    mountedPanelCount: state.mountedPanelIds.length,
    visiblePanelCount: state.visiblePanelStack.length,
    // There is exactly one physical host regardless of the number of logical panels.
    openSidePanelCount: activePanelId ? 1 : 0,
    occluded,
  };
}

export function reduceRightDockState(state, action) {
  switch (action.type) {
    case 'mount':
      return mountRightDockPanel(state, action.panelId);
    case 'unmount':
      return unmountRightDockPanel(state, action.panelId);
    case 'activate':
      return activateRightDockPanel(state, action.panelId);
    case 'hide':
      return hideRightDockPanel(state, action.panelId);
    case 'occlude':
      return setRightDockOcclusion(state, action.occlusionId, action.active);
    default:
      return state;
  }
}
