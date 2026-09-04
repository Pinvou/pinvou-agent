// The design lane has been merged into work: only work remains here (the
// code lane does not persist through this module). Any historical value
// (including 'design') folds into work on read.
const PINVOU_MODES = ['work'];

const PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v4';
const V3_PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v3';
const V2_PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v2';
const LEGACY_PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v1';
const DEFAULT_PINVOU_MODE_SCOPE = 'draft';
const MAX_SESSION_MODE_STATES = 200;

const UNROUTED_SUBTAB = 'general';
const DEFAULT_SUBTAB = UNROUTED_SUBTAB;
// Merged scene list: work's originals (personal workbench / document
// writing) + the design additions folded in (poster / data visualization).
const SUBTABS = [UNROUTED_SUBTAB, 'personal-workbench', 'document-writing', 'poster', 'data-visualization'];

function normalizePinvouMode(value) {
  return PINVOU_MODES.includes(value) ? value : 'work';
}

function normalizeSubtab(value) {
  return SUBTABS.includes(value) ? value : DEFAULT_SUBTAB;
}

function createPinvouModeScopeKey(sessionId) {
  const normalized = String(sessionId || '').trim();
  return normalized ? `session:${normalized}` : DEFAULT_PINVOU_MODE_SCOPE;
}

function createPinvouModeState(value) {
  const input = value && typeof value === 'object' ? value : {};
  return {
    mode: normalizePinvouMode(input.mode),
    subtab: normalizeSubtab(input.subtab),
  };
}

function persistedPinvouModeState(value) {
  const normalized = createPinvouModeState(value);
  return {
    mode: normalized.mode,
    subtab: normalized.subtab,
  };
}

// The v3/v2 era entry shape was {mode, workSubtab, designSubtab}:
// mode==='design' took designSubtab, otherwise workSubtab — folded into the
// single subtab here.
function upgradeLegacyEntry(entry) {
  const input = entry && typeof entry === 'object' ? entry : {};
  const subtab = input.mode === 'design' ? input.designSubtab : input.workSubtab;
  return persistedPinvouModeState({ mode: 'work', subtab });
}

function createEmptyModeStore() {
  return {
    draft: persistedPinvouModeState(),
    sessions: {},
    sessionOrder: [],
  };
}

function parsedModeStoreFromRawWith(raw, upgradeEntry) {
  const empty = createEmptyModeStore();
  try {
    const parsed = JSON.parse(raw || '{}');
    if (!parsed || typeof parsed !== 'object') return empty;
    const sessions = parsed.sessions && typeof parsed.sessions === 'object' ? parsed.sessions : {};
    const normalizedSessions = {};
    Object.keys(sessions).forEach((key) => {
      normalizedSessions[key] = upgradeEntry(sessions[key]);
    });
    return {
      draft: upgradeEntry(parsed.draft),
      sessions: normalizedSessions,
      sessionOrder: Array.isArray(parsed.sessionOrder)
        // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
        ? parsed.sessionOrder.filter((key) => Object.prototype.hasOwnProperty.call(normalizedSessions, key))
        : Object.keys(normalizedSessions),
    };
  } catch {
    return empty;
  }
}

function parsedModeStoreFromRaw(raw) {
  return parsedModeStoreFromRawWith(raw, persistedPinvouModeState);
}

// v3 → v4: entry fold (design → work + designSubtab).
function migrateV3ModeStore(raw) {
  return parsedModeStoreFromRawWith(raw, upgradeLegacyEntry);
}

// v2 → v4: first apply the old v2→v3 semantics (draft-scoped scene choices
// reset to general, session scopes untouched), then the v3→v4 entry fold.
function migrateV2ModeStore(raw) {
  const migrated = parsedModeStoreFromRawWith(raw, upgradeLegacyEntry);
  const draftSubtab = migrated.draft.subtab;
  return {
    ...migrated,
    draft: persistedPinvouModeState({
      mode: 'work',
      subtab: draftSubtab === 'document-writing' || draftSubtab === 'poster'
        ? UNROUTED_SUBTAB
        : draftSubtab,
    }),
  };
}

function readModeStore(target) {
  const empty = createEmptyModeStore();
  if (!target || typeof target.getItem !== 'function') return empty;
  try {
    const current = target.getItem(PINVOU_MODE_STORAGE_KEY);
    if (current) return parsedModeStoreFromRaw(current);
    const v3 = target.getItem(V3_PINVOU_MODE_STORAGE_KEY);
    if (v3) return migrateV3ModeStore(v3);
    const v2 = target.getItem(V2_PINVOU_MODE_STORAGE_KEY);
    if (v2) return migrateV2ModeStore(v2);
    return empty;
  } catch {
    return empty;
  }
}

function readLegacyDraftState(target) {
  if (!target || typeof target.getItem !== 'function') return null;
  try {
    const raw = target.getItem(LEGACY_PINVOU_MODE_STORAGE_KEY);
    if (!raw) return null;
    return persistedPinvouModeState(JSON.parse(raw));
  } catch {
    return null;
  }
}

function hasStoredModeState(target) {
  if (!target || typeof target.getItem !== 'function') return false;
  try {
    return !!target.getItem(PINVOU_MODE_STORAGE_KEY)
      || !!target.getItem(V3_PINVOU_MODE_STORAGE_KEY)
      || !!target.getItem(V2_PINVOU_MODE_STORAGE_KEY);
  } catch {
    return false;
  }
}

function loadPinvouModeState(storage, scopeKey = DEFAULT_PINVOU_MODE_SCOPE) {
  const target = storage || (typeof window === 'undefined' ? null : window.localStorage);
  const store = readModeStore(target);
  if (scopeKey === DEFAULT_PINVOU_MODE_SCOPE) {
    return createPinvouModeState(
      hasStoredModeState(target) ? store.draft : (readLegacyDraftState(target) || store.draft),
    );
  }
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
  if (Object.prototype.hasOwnProperty.call(store.sessions, scopeKey)) {
    return createPinvouModeState(store.sessions[scopeKey]);
  }
  // 历史会话没有场景记录时保持普通聊天，避免被新入口静默归入公文写作。
  return createPinvouModeState({ mode: 'work', subtab: UNROUTED_SUBTAB });
}

/**
 * No static importer: tests/pinvou_mode_state.test.js reads this file as text,
 * strips the export, and evaluates it by name in a Node vm sandbox; knip
 * cannot build an edge for that channel, so the `@public` tag keeps it from
 * being removed as a dead export.
 * @public
 */
export function hasPinvouModeState(storage, scopeKey) {
  if (!scopeKey || scopeKey === DEFAULT_PINVOU_MODE_SCOPE) return false;
  const target = storage || (typeof window === 'undefined' ? null : window.localStorage);
  const store = readModeStore(target);
  // biome-ignore lint/suspicious/noPrototypeBuiltins: Safari 14 is the floor and Object.hasOwn is unavailable; this call is already the safe form
  return Object.prototype.hasOwnProperty.call(store.sessions, scopeKey);
}

function savePinvouModeState(state, storage, scopeKey = DEFAULT_PINVOU_MODE_SCOPE) {
  const target = storage || (typeof window === 'undefined' ? null : window.localStorage);
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
  } catch {
    // 持久化失败不影响当前会话内模式切换。
  }
  return normalized;
}

function reducePinvouModeState(state, action) {
  const current = createPinvouModeState(state);
  if (!action || typeof action !== 'object') return current;
  switch (action.type) {
    case 'set-mode':
      return {
        ...current,
        mode: normalizePinvouMode(action.mode),
      };
    case 'set-subtab':
      return {
        ...current,
        subtab: normalizeSubtab(action.subtab),
      };
    default:
      return current;
  }
}

export {
  DEFAULT_PINVOU_MODE_SCOPE,
  PINVOU_MODE_STORAGE_KEY,
  PINVOU_MODES,
  SUBTABS,
  UNROUTED_SUBTAB,
  createPinvouModeScopeKey,
  createPinvouModeState,
  loadPinvouModeState,
  normalizePinvouMode,
  normalizeSubtab,
  reducePinvouModeState,
  savePinvouModeState,
};
