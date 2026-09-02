// 草稿态（尚未创建会话）配置快照的 localStorage 缓存。独立成模块是为了让
// 设置页（ProvidersSection）与对话页（CodexAcpView）共用而不产生循环引用
// （CodexAcpView ↔ SettingsView）。
//
// ACP 的模型/权限模式/推理强度等配置项是会话级的：这里缓存每个 agent 最近
// 一次会话上报的配置快照，供新会话草稿预展示和预选。

const DRAFT_CONTROLS_CACHE_KEY = 'pinvou_codex_draft_controls';

export function loadDraftControlsCache() {
  try {
    const value = JSON.parse(localStorage.getItem(DRAFT_CONTROLS_CACHE_KEY) || '{}');
    return value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  } catch {
    return {};
  }
}

export function snapshotSessionControls(info) {
  if (!info) return null;
  const snapshot = {
    models: Array.isArray(info.models) ? info.models : [],
    current_model_id: info.current_model_id || '',
    modes: info.modes || null,
    config_options: Array.isArray(info.config_options) ? info.config_options : [],
    provider: info.provider || null,
  };
  if (!snapshot.models.length && !snapshot.modes && !snapshot.config_options.length) return null;
  return snapshot;
}

export function rememberDraftControls(agentId, info) {
  const snapshot = snapshotSessionControls(info);
  if (!agentId || !snapshot) return null;
  const cache = { ...loadDraftControlsCache(), [agentId]: snapshot };
  try {
    localStorage.setItem(DRAFT_CONTROLS_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // 缓存写不进去时仅影响下次草稿预展示，本次会话不受影响。
  }
  return snapshot;
}

/// 切换/删除 Provider 或恢复官方登录后调用：用新 Provider 的模型重写草稿
/// 配置快照——对话页模型选择器立即可见且显示正确模型名（旧快照残留会让
/// 用户看到旧 Provider 的模型；直接删除则选择器整排消失）。官方登录时
/// 无法预知默认模型，快照失效，首次会话上报后重建。
///
/// 注意 config_options 里的 `model` 选项必须剔除：它带着旧 Provider 上报的
/// options 与 currentValue，保留会走 config 通道继续显示旧模型；剔除后
/// resolveAcpSessionControls 回落到 models 兜底列表（即新模型）。其余
/// config（推理强度等）与 modes 与 Provider 无关，保留。
export function reseedDraftControlsAfterProviderSwitch(agentId, model, modelEntries) {
  try {
    const cache = loadDraftControlsCache();
    if (agentId && model) {
      const prev = cache[agentId] || {};
      const prevConfigOptions = Array.isArray(prev.config_options) ? prev.config_options : [];
      // Claude 的真实会话以上下文无关的别名（default/sonnet/opus/haiku/fable）
      // 上报模型选项、显示名为槽位映射值；调用方可传 modelEntries 复刻这一
      // 形态，草稿下拉即与真实会话一致（5 个选项、名字同为 Provider 模型）。
      const models = Array.isArray(modelEntries) && modelEntries.length
        ? modelEntries
        : [{ id: model, name: model }];
      cache[agentId] = {
        models,
        current_model_id: model,
        modes: prev.modes || null,
        config_options: prevConfigOptions.filter(option => option && option.id !== 'model'),
        provider: null,
      };
    } else if (agentId) {
      delete cache[agentId];
    } else {
      localStorage.removeItem(DRAFT_CONTROLS_CACHE_KEY);
      return;
    }
    localStorage.setItem(DRAFT_CONTROLS_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // 缓存重写失败仅影响草稿预展示，可忽略。
  }
}

// 一次性模型探针标记：切换/删除 Provider 或恢复官方后由设置页写入，对话页
// 草稿态（未建会话）消费一次——主动连接 ACP 拉取真实模型列表覆盖 reseed 的
// 占位快照，之后恢复懒加载。key 前缀由本模块独占，避免两侧各自硬编码漂移。
const MODELS_PROBE_PENDING_PREFIX = 'pinvou_acp_models_probe:';

export function markAcpModelsProbePending(agentId) {
  if (!agentId) return;
  try {
    localStorage.setItem(MODELS_PROBE_PENDING_PREFIX + agentId, '1');
  } catch {
    // 标记写不进去时仅退回 reseed 占位快照，不影响使用。
  }
}

// 读取并立即清除探针标记：先清再探，失败也只是一次性机会（防重入/重复连接）。
export function consumeAcpModelsProbePending(agentId) {
  if (!agentId) return false;
  try {
    const pending = localStorage.getItem(MODELS_PROBE_PENDING_PREFIX + agentId) === '1';
    if (pending) localStorage.removeItem(MODELS_PROBE_PENDING_PREFIX + agentId);
    return pending;
  } catch {
    return false;
  }
}
