const PINVOU_MODES = ['work', 'design'];

const PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v3';
const PREVIOUS_PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v2';
const LEGACY_PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v1';
const DEFAULT_PINVOU_MODE_SCOPE = 'draft';
const MAX_SESSION_MODE_STATES = 200;

const UNROUTED_WORK_SUBTAB = 'general';
const DEFAULT_WORK_SUBTAB = UNROUTED_WORK_SUBTAB;
const DEFAULT_DESIGN_SUBTAB = UNROUTED_WORK_SUBTAB;
const WORK_SUBTABS = [UNROUTED_WORK_SUBTAB, 'document-writing'];
const DESIGN_SUBTABS = [UNROUTED_WORK_SUBTAB, 'poster', 'data-visualization'];

function normalizePinvouMode(value) {
  return PINVOU_MODES.includes(value) ? value : 'work';
}

function normalizeWorkSubtab(value) {
  return WORK_SUBTABS.includes(value) ? value : DEFAULT_WORK_SUBTAB;
}

function normalizeDesignSubtab(value) {
  return DESIGN_SUBTABS.includes(value) ? value : DEFAULT_DESIGN_SUBTAB;
}

function createPinvouModeScopeKey(sessionId) {
  const normalized = String(sessionId || '').trim();
  return normalized ? `session:${normalized}` : DEFAULT_PINVOU_MODE_SCOPE;
}

function createPinvouModeState(value) {
  const input = value && typeof value === 'object' ? value : {};
  return {
    mode: normalizePinvouMode(input.mode),
    workSubtab: normalizeWorkSubtab(input.workSubtab),
    designSubtab: normalizeDesignSubtab(input.designSubtab),
    selectedDesignElementId: input.selectedDesignElementId || undefined,
    designRuntimeStatus: input.designRuntimeStatus || 'idle',
  };
}

function persistedPinvouModeState(value) {
  const normalized = createPinvouModeState(value);
  return {
    mode: normalized.mode,
    workSubtab: normalized.workSubtab,
    designSubtab: normalized.designSubtab,
  };
}

function createEmptyModeStore() {
  return {
    draft: persistedPinvouModeState(),
    sessions: {},
    sessionOrder: [],
  };
}

function parsedModeStoreFromRaw(raw) {
  const empty = createEmptyModeStore();
  try {
    const parsed = JSON.parse(raw || '{}');
    if (!parsed || typeof parsed !== 'object') return empty;
    const sessions = parsed.sessions && typeof parsed.sessions === 'object' ? parsed.sessions : {};
    const normalizedSessions = {};
    Object.keys(sessions).forEach((key) => {
      normalizedSessions[key] = persistedPinvouModeState(sessions[key]);
    });
    return {
      draft: persistedPinvouModeState(parsed.draft),
      sessions: normalizedSessions,
      sessionOrder: Array.isArray(parsed.sessionOrder)
        ? parsed.sessionOrder.filter((key) => Object.prototype.hasOwnProperty.call(normalizedSessions, key))
        : Object.keys(normalizedSessions),
    };
  } catch (_) {
    return empty;
  }
}

function migratePreviousModeStore(previous) {
  const migrated = parsedModeStoreFromRaw(previous);
  return {
    ...migrated,
    draft: persistedPinvouModeState({
      ...migrated.draft,
      workSubtab: migrated.draft.workSubtab === 'document-writing'
        ? UNROUTED_WORK_SUBTAB
        : migrated.draft.workSubtab,
      designSubtab: migrated.draft.designSubtab === 'poster'
        ? UNROUTED_WORK_SUBTAB
        : migrated.draft.designSubtab,
    }),
  };
}

function readModeStore(target) {
  const empty = createEmptyModeStore();
  if (!target || typeof target.getItem !== 'function') return empty;
  try {
    const current = target.getItem(PINVOU_MODE_STORAGE_KEY);
    if (current) return parsedModeStoreFromRaw(current);
    const previous = target.getItem(PREVIOUS_PINVOU_MODE_STORAGE_KEY);
    if (previous) return migratePreviousModeStore(previous);
    return empty;
  } catch (_) {
    return empty;
  }
}

function readLegacyDraftState(target) {
  if (!target || typeof target.getItem !== 'function') return null;
  try {
    const raw = target.getItem(LEGACY_PINVOU_MODE_STORAGE_KEY);
    if (!raw) return null;
    return persistedPinvouModeState(JSON.parse(raw));
  } catch (_) {
    return null;
  }
}

function hasStoredModeState(target) {
  if (!target || typeof target.getItem !== 'function') return false;
  try {
    return !!target.getItem(PINVOU_MODE_STORAGE_KEY) || !!target.getItem(PREVIOUS_PINVOU_MODE_STORAGE_KEY);
  } catch (_) {
    return false;
  }
}

function loadPinvouModeState(storage, scopeKey = DEFAULT_PINVOU_MODE_SCOPE) {
  const target = storage || (typeof window !== 'undefined' ? window.localStorage : null);
  const store = readModeStore(target);
  if (scopeKey === DEFAULT_PINVOU_MODE_SCOPE) {
    return createPinvouModeState(
      hasStoredModeState(target) ? store.draft : (readLegacyDraftState(target) || store.draft),
    );
  }
  if (Object.prototype.hasOwnProperty.call(store.sessions, scopeKey)) {
    return createPinvouModeState(store.sessions[scopeKey]);
  }
  // 历史会话没有场景记录时保持普通聊天，避免被新入口静默归入公文写作。
  return createPinvouModeState({ mode: 'work', workSubtab: UNROUTED_WORK_SUBTAB });
}

function hasPinvouModeState(storage, scopeKey) {
  if (!scopeKey || scopeKey === DEFAULT_PINVOU_MODE_SCOPE) return false;
  const target = storage || (typeof window !== 'undefined' ? window.localStorage : null);
  const store = readModeStore(target);
  return Object.prototype.hasOwnProperty.call(store.sessions, scopeKey);
}

function savePinvouModeState(state, storage, scopeKey = DEFAULT_PINVOU_MODE_SCOPE) {
  const target = storage || (typeof window !== 'undefined' ? window.localStorage : null);
  const normalized = createPinvouModeState(state);
  if (!target || typeof target.setItem !== 'function') return normalized;

  const store = readModeStore(target);
  if (scopeKey === DEFAULT_PINVOU_MODE_SCOPE) {
    store.draft = persistedPinvouModeState(normalized);
  } else {
    store.sessions[scopeKey] = persistedPinvouModeState(normalized);
    store.sessionOrder = store.sessionOrder.filter((key) => key !== scopeKey);
    store.sessionOrder.push(scopeKey);
    while (store.sessionOrder.length > MAX_SESSION_MODE_STATES) {
      const expired = store.sessionOrder.shift();
      if (expired) delete store.sessions[expired];
    }
  }
  try {
    target.setItem(PINVOU_MODE_STORAGE_KEY, JSON.stringify(store));
  } catch (_) {
    // 持久化失败不影响当前会话内模式切换。
  }
  return normalized;
}

function reducePinvouModeState(state, action) {
  const current = createPinvouModeState(state);
  if (!action || typeof action !== 'object') return current;
  switch (action.type) {
    case 'set-mode': {
      const mode = normalizePinvouMode(action.mode);
      return {
        ...current,
        mode,
        selectedDesignElementId: mode === 'design' ? current.selectedDesignElementId : undefined,
        designRuntimeStatus: mode === 'design' ? current.designRuntimeStatus : 'idle',
      };
    }
    case 'set-work-subtab':
      return {
        ...current,
        workSubtab: normalizeWorkSubtab(action.subtab),
      };
    case 'set-design-subtab':
      return {
        ...current,
        designSubtab: normalizeDesignSubtab(action.subtab),
      };
    case 'set-design-runtime-status':
      return {
        ...current,
        designRuntimeStatus: action.status || current.designRuntimeStatus,
      };
    case 'set-selected-design-element':
      return {
        ...current,
        selectedDesignElementId: action.elementId || undefined,
      };
    default:
      return current;
  }
}

export {
  DEFAULT_PINVOU_MODE_SCOPE,
  DESIGN_SUBTABS,
  PINVOU_MODE_STORAGE_KEY,
  PINVOU_MODES,
  WORK_SUBTABS,
  createPinvouModeScopeKey,
  createPinvouModeState,
  hasPinvouModeState,
  loadPinvouModeState,
  normalizeDesignSubtab,
  normalizePinvouMode,
  normalizeWorkSubtab,
  reducePinvouModeState,
  savePinvouModeState,
};
