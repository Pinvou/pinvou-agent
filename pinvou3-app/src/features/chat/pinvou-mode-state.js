const PINVOU_MODES = ['work', 'design', 'code'];
const CODE_AGENT_PROVIDERS = ['codex', 'claude-code', 'kimi-code'];

const PINVOU_MODE_STORAGE_KEY = 'pinvou_mode_state_v1';

const PINVOU_MODE_LABELS = {
  work: '工作',
  design: '设计',
  code: '代码',
};

const CODE_AGENT_PROVIDER_LABELS = {
  codex: 'Codex',
  'claude-code': 'Claude Code',
  'kimi-code': 'Kimi Code',
};

function normalizePinvouMode(value) {
  return PINVOU_MODES.includes(value) ? value : 'work';
}

function normalizeCodeAgentProvider(value) {
  return CODE_AGENT_PROVIDERS.includes(value) ? value : undefined;
}

function createPinvouModeState(value) {
  const input = value && typeof value === 'object' ? value : {};
  return {
    mode: normalizePinvouMode(input.mode),
    codeProvider: normalizeCodeAgentProvider(input.codeProvider),
    selectedDesignElementId: input.selectedDesignElementId || undefined,
    designRuntimeStatus: input.designRuntimeStatus || 'idle',
  };
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
    case 'set-code-provider':
      return {
        ...current,
        codeProvider: normalizeCodeAgentProvider(action.provider),
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

function loadPinvouModeState(storage) {
  const target = storage || (typeof window !== 'undefined' ? window.localStorage : null);
  if (!target || typeof target.getItem !== 'function') return createPinvouModeState();
  try {
    return createPinvouModeState(JSON.parse(target.getItem(PINVOU_MODE_STORAGE_KEY) || '{}'));
  } catch (_) {
    return createPinvouModeState();
  }
}

function savePinvouModeState(state, storage) {
  const target = storage || (typeof window !== 'undefined' ? window.localStorage : null);
  const normalized = createPinvouModeState(state);
  if (target && typeof target.setItem === 'function') {
    try {
      target.setItem(PINVOU_MODE_STORAGE_KEY, JSON.stringify({
        mode: normalized.mode,
        codeProvider: normalized.codeProvider,
      }));
    } catch (_) {
      // 持久化失败不影响当前会话内模式切换。
    }
  }
  return normalized;
}

export {
  CODE_AGENT_PROVIDERS,
  CODE_AGENT_PROVIDER_LABELS,
  PINVOU_MODE_LABELS,
  PINVOU_MODES,
  createPinvouModeState,
  loadPinvouModeState,
  normalizeCodeAgentProvider,
  normalizePinvouMode,
  reducePinvouModeState,
  savePinvouModeState,
};
