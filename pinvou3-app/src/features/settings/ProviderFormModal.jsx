// ACP Provider（第三方中转）新增/编辑弹窗。照 ModelFormModal 范式：
// 预设填充 + 掩码 key（keep/replace/delete）+ 草稿-保存。

import React, { useEffect, useState } from 'react';
import { ChevronDown, X } from '../../components/icons.jsx';
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  ACP_MODEL_1M_VARIANTS, ACP_MODEL_PRESETS, ACP_PROVIDER_PRESETS, CLAUDE_MODEL_SLOT_IDS,
} from './acp-provider-catalog.js';

// 模型建议下拉（替代原生 datalist）：**候选全量展示、不做字符过滤**（原生
// datalist 会把不匹配的候选隐藏，用户容易误以为「只有匹配的几个模型可
// 选」），仅按输入把匹配项排在前面。点击选中；Esc/失焦关闭。
function ModelSuggestInput({ value, onChange, suggestions, inputClass, placeholder, testId }) {
  const [open, setOpen] = useState(false);
  const query = String(value || '').trim().toLowerCase();
  const score = name => {
    const lower = name.toLowerCase();
    if (!query) return 0;
    if (lower === query) return 0;
    if (lower.startsWith(query)) return 1;
    if (lower.includes(query)) return 2;
    return 3;
  };
  const sorted = query
    ? [...suggestions].sort((a, b) => score(a) - score(b) || a.localeCompare(b))
    : suggestions;
  return (
    <div className="relative mt-1.5">
      <input
        data-testid={testId}
        className={inputClass}
        value={value}
        onChange={event => { onChange(event.target.value); setOpen(true); }}
        onFocus={() => setOpen(true)}
        onBlur={() => window.setTimeout(() => setOpen(false), 150)}
        onKeyDown={event => { if (event.key === 'Escape') setOpen(false); }}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
      />
      {open && sorted.length > 0 && (
        <div
          data-testid={`${testId}-suggest`}
          className="absolute z-10 mt-1 max-h-44 w-full overflow-y-auto custom-scrollbar rounded-xl border border-black/[0.08] dark:border-white/[0.12] bg-white dark:bg-[#2A2B2D] shadow-lg"
        >
          {sorted.map(modelName => (
            <button
              key={modelName}
              type="button"
              data-testid={`${testId}-option-${modelName}`}
              // onMouseDown + preventDefault：在 input blur 前完成选中
              onMouseDown={event => { event.preventDefault(); onChange(modelName); setOpen(false); }}
              className="block w-full px-3 py-2 text-left font-mono text-[12px] hover:bg-black/[0.05] dark:hover:bg-white/[0.08]"
            >
              {modelName}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

// 预设选择器（替代原生 select，与 ModelSuggestInput 同款面板样式）：
// 按钮展示当前选择，点开全量列表，失焦/Esc 关闭。
function PresetSelect({ value, onChange, presets, otherLabel, copy, inputClass }) {
  const [open, setOpen] = useState(false);
  const current = presets.find(preset => preset.key === value) || null;
  // 展示名走 i18n（copy[preset.nameKey]），缺失 key 时回退 preset.name
  const label = preset => (preset && copy && copy[preset.nameKey]) || (preset && preset.name) || '';
  return (
    <div className="relative mt-1.5">
      <button
        type="button"
        data-testid="acp-provider-preset"
        onClick={() => setOpen(current => !current)}
        onBlur={() => window.setTimeout(() => setOpen(false), 150)}
        onKeyDown={event => { if (event.key === 'Escape') setOpen(false); }}
        className={`${inputClass} flex items-center justify-between gap-2 text-left`}
      >
        <span className="truncate">{current ? label(current) : otherLabel}</span>
        <ChevronDown size={14} className="shrink-0 opacity-60" />
      </button>
      {open && (
        <div
          data-testid="acp-provider-preset-suggest"
          className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto custom-scrollbar rounded-xl border border-black/[0.08] dark:border-white/[0.12] bg-white dark:bg-[#2A2B2D] shadow-lg"
        >
          <button
            type="button"
            data-testid="acp-provider-preset-option-other"
            onMouseDown={event => { event.preventDefault(); onChange(''); setOpen(false); }}
            className="block w-full px-3 py-2 text-left text-[12px] opacity-70 hover:bg-black/[0.05] dark:hover:bg-white/[0.08]"
          >
            {otherLabel}
          </button>
          {presets.map(preset => (
            <button
              key={preset.key}
              type="button"
              data-testid={`acp-provider-preset-option-${preset.key}`}
              onMouseDown={event => { event.preventDefault(); onChange(preset.key); setOpen(false); }}
              className={`block w-full px-3 py-2 text-left text-[12px] hover:bg-black/[0.05] dark:hover:bg-white/[0.08] ${preset.key === value ? 'font-semibold text-[#007AFF]' : ''}`}
            >
              {label(preset)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export function ProviderFormModal({ agent, copy, isDark, initial, onClose, onSaved }) {
  const [name, setName] = useState(initial?.name || '');
  const [baseUrl, setBaseUrl] = useState(initial?.baseUrl || '');
  // Kimi Agent 默认走 Kimi 原生协议（Kimi Code 官方文档的专用类型）；
  // Claude Code 只支持 Anthropic 协议（固定，不提供选择）；
  // Codex 固定 Responses（OpenAI 协议家族），记录默认 openai；
  // 其余 Agent 默认 Anthropic 兼容。
  const [wireApi, setWireApi] = useState(
    initial?.wireApi || (agent === 'kimi' ? 'kimi' : agent === 'codex' ? 'openai' : 'anthropic')
  );
  const [model, setModel] = useState(initial?.model || '');
  // 上下文窗口（可选，仅 codex/kimi）：codex 写模型 catalog、
  // kimi 写 max_context_size；留空用 CLI 默认。
  const [contextWindow, setContextWindow] = useState(
    initial?.contextWindow ? String(initial.contextWindow) : ''
  );
  // Claude Code 细化模型槽位：默认跟随主模型，可单独修改；必填（留空槽位
  // 会让 CC 子 agent 回落官方模型走官方流量）。
  const [modelSlots, setModelSlots] = useState(() => ({ ...(initial?.modelSlots || {}) }));
  const [apiKey, setApiKey] = useState('');
  const [keyLoaded, setKeyLoaded] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  // 预设下拉：'' = 其它（自定义，不应用任何预设）。
  const [presetKey, setPresetKey] = useState('');

  // 模型建议列表：选择预设后按该厂商官方在列名单筛选；「其它」显示全部官方在列模型。
  // Claude 额外提供该厂商的 1M 上下文变体（仅 CC 需要显式声明 [1m]；选中预设
  // 只显示本厂商变体，「其它」时汇总全部）。
  const activePreset = ACP_PROVIDER_PRESETS.find(preset => preset.key === presetKey) || null;
  const baseSuggestions = activePreset && activePreset.models && activePreset.models.length
    ? activePreset.models
    : ACP_MODEL_PRESETS;
  const oneMVariants = activePreset
    ? (activePreset.models1m || [])
    : ACP_MODEL_1M_VARIANTS;
  const suggestedModels = agent === 'claude'
    ? [...baseSuggestions, ...oneMVariants.filter(variant => !baseSuggestions.includes(variant))]
    : baseSuggestions;
  // wire 协议由 Agent 的 CLI 硬约束决定，不提供选择：
  // - Claude Code 只支持 Anthropic 协议（避免选了 OpenAI 兼容后必然 404/400）
  // - Codex 官方当前版本统一使用 Responses 协议（wire_api 仅 "responses" 合法）
  const wireLocked = agent === 'claude' || agent === 'codex';
  const wireLockedLabel = agent === 'claude' ? copy.wireAnthropic : copy.wireResponses;
  // N10：Claude 只支持 Anthropic 协议，选中无 Anthropic 端点的预设时给出
  // 警告（列表保持全量，不隐藏供应商）。
  const anthropicEndpointMissing =
    wireLocked && agent === 'claude' && activePreset && !activePreset.baseUrlAnthropic;

  useEffect(() => {
    const onKey = event => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // 编辑有 key 的 Provider 时回填真实密钥：输入框 type=password 自然掩码，
  // 点「显示」切 text 变明文（密码管理器范式）。仅编辑时拉取，列表永不回传。
  useEffect(() => {
    if (!initial || !initial.hasCredential) return undefined;
    let alive = true;
    invokeTauri('get_acp_provider_key', { agent, providerId: initial.id })
      .then(key => {
        if (!alive) return;
        if (key) setApiKey(key);
        setKeyLoaded(true);
      })
      // 回填失败**不**置 keyLoaded：保持 false → save 时按「保留」处理，
      // 防止读不到已保存密钥时用户看到空字段直接保存即静默删除（评审高危项）。
      .catch(() => {});
    return () => { alive = false; };
    // eslint 的 exhaustive-deps 本仓未启用；仅挂载/编辑目标变化时拉一次
  }, [agent, initial]);

  // 选择预设只填 base URL 与 wire 协议，**不自动填模型**：模型由用户自行
  // 输入（输入框提供按厂商筛选的官方在列模型建议），避免预设值造成
  // 「不可自行填写」的误解，也兼容中转商自有的模型名。
  // wire 按 Agent 适配：Kimi 原生预设（Moonshot/Kimi Code）用于非 Kimi Agent
  // 时落到 OpenAI 兼容（这些端点同样提供 OpenAI 协议）；Claude 固定 Anthropic。
  const applyPreset = preset => {
    if (!preset) return;
    const useAnthropicEndpoint = agent === 'claude' || preset.wireApi === 'anthropic';
    setBaseUrl((useAnthropicEndpoint && preset.baseUrlAnthropic) || preset.baseUrl || '');
    if (wireLocked) return;
    if (preset.wireApi === 'kimi' && agent !== 'kimi') {
      setWireApi('openai');
    } else {
      setWireApi(preset.wireApi || 'anthropic');
    }
  };

  // 主模型变化时，把「仍等于旧主模型或为空」的槽位一起跟随；用户手动改过的
  // 槽位保持不变。比较基准用渲染期的 model 值：输入是离散事件、同步刷新，
  // 逐键准确（不能用 ref，updater 延迟执行会读到新值导致永远不跟随）。
  const changeModel = value => {
    const previous = model;
    setModelSlots(slots => {
      const next = { ...slots };
      for (const slot of CLAUDE_MODEL_SLOT_IDS) {
        if (!next[slot] || next[slot] === previous) next[slot] = value;
      }
      return next;
    });
    setModel(value);
  };

  const save = async () => {
    if (!String(name || '').trim()) {
      setError(copy.providerName);
      return;
    }
    if (!String(baseUrl || '').trim()) {
      setError(copy.baseUrl);
      return;
    }
    let slotPayload = null;
    if (agent === 'claude') {
      const missing = CLAUDE_MODEL_SLOT_IDS.filter(slot => !String(modelSlots[slot] || '').trim());
      if (missing.length) {
        setError(copy.modelSlotsRequired);
        return;
      }
      slotPayload = Object.fromEntries(
        CLAUDE_MODEL_SLOT_IDS.map(slot => [slot, String(modelSlots[slot]).trim()])
      );
    }
    let contextWindowPayload = null;
    if (agent !== 'claude' && String(contextWindow || '').trim()) {
      const parsed = Number.parseInt(String(contextWindow).trim(), 10);
      if (!Number.isFinite(parsed) || parsed <= 0) {
        setError(copy.contextWindowInvalid);
        return;
      }
      contextWindowPayload = parsed;
    }
    setSaving(true);
    setError('');
    // key 语义（密码管理器范式，无选择器）：编辑时字段已回填真实 key——
    // 清空 = 删除，改动 = 替换，原样 = 重写同值（无效果）。例外：编辑有
    // key 但回填失败且字段仍为空 → 保留（防止读不到时误删）。
    const trimmedKey = String(apiKey || '').trim();
    const apiKeyAction = !initial
      ? 'replace'
      : trimmedKey
        ? 'replace'
        : (initial.hasCredential && !keyLoaded ? 'keep' : 'delete');
    // 「清空 = 删除」会删掉已保存密钥：二次确认防误触。回填失败时
    // keyLoaded=false 恒走 keep，不会到这里。
    if (apiKeyAction === 'delete' && !window.confirm(copy.deleteKeyConfirm)) {
      setSaving(false);
      return;
    }
    try {
      const saved = await invokeTauri('save_acp_provider', {
        agent,
        providerId: initial?.id || null,
        name: String(name).trim(),
        baseUrl: String(baseUrl).trim(),
        model: String(model || '').trim() || null,
        modelSlots: slotPayload,
        contextWindow: contextWindowPayload,
        wireApi,
        apiKey: trimmedKey || null,
        apiKeyAction,
      });
      onSaved(saved);
      onClose();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };

  const inputClass = `w-full h-10 rounded-xl px-3 text-[13px] outline-none transition-colors ${
    isDark
      ? 'bg-white/[0.06] text-[#E8EAED] border border-white/[0.09] focus:border-[#64B5F6]/50'
      : 'bg-[#F0F4F9] text-[#1F1F1F] border border-black/[0.06] focus:border-[#0B57D0]/50'
  }`;

  return (
    <div
      data-testid="acp-provider-form-dialog"
      role="dialog"
      aria-modal="true"
      className="fixed inset-0 z-[110] flex items-center justify-center bg-black/45 backdrop-blur-[14px] animate-in fade-in duration-200"
      onClick={onClose}
    >
      <div
        onClick={event => event.stopPropagation()}
        className={`relative w-[min(480px,calc(100vw-24px))] max-h-[calc(100vh-48px)] overflow-y-auto custom-scrollbar rounded-[24px] p-6 ${
          isDark ? 'bg-[#1E1F20] text-[#E8EAED]' : 'bg-white text-[#1F1F1F]'
        }`}
      >
        <div className="flex items-center justify-between mb-5">
          <h2 className="text-[17px] font-semibold">
            {initial ? `${copy.edit} · ${initial.name}` : copy.addProvider}
          </h2>
          <button data-testid="acp-provider-form-close" onClick={onClose} aria-label={copy.cancel} className="h-8 w-8 rounded-full flex items-center justify-center hover:bg-black/[0.06] dark:hover:bg-white/[0.08]">
            <X size={16} />
          </button>
        </div>

        {/* 预设（仅新增时展示）：默认「其它」= 自定义填写；选择预设后自动填
            base URL 与协议，模型建议列表按该厂商筛选。 */}
        {!initial && (
          <div className="block mb-4">
            <span className="text-[12px] font-medium opacity-70">{copy.addProvider}</span>
            <PresetSelect
              value={presetKey}
              onChange={next => {
                setPresetKey(next);
                applyPreset(ACP_PROVIDER_PRESETS.find(preset => preset.key === next));
              }}
              presets={ACP_PROVIDER_PRESETS}
              otherLabel={copy.providerOther}
              copy={copy}
              inputClass={inputClass}
            />
          </div>
        )}

        <label className="block mb-4">
          <span className="text-[12px] font-medium opacity-70">{copy.providerName}</span>
          <input
            data-testid="acp-provider-name"
            className={`${inputClass} mt-1.5`}
            value={name}
            onChange={event => setName(event.target.value)}
            placeholder={copy.providerNamePlaceholder}
          />
        </label>

        <label className="block mb-4">
          <span className="text-[12px] font-medium opacity-70">{copy.baseUrl}</span>
          <input
            data-testid="acp-provider-base-url"
            className={`${inputClass} mt-1.5 font-mono`}
            value={baseUrl}
            onChange={event => setBaseUrl(event.target.value)}
            placeholder={copy.baseUrlPlaceholder}
            spellCheck={false}
          />
        </label>

        <div className="mb-4">
          <span className="block text-[12px] font-medium opacity-70 mb-1.5">{copy.wireApi}</span>
          {wireLocked ? (
            // Claude Code 固定 Anthropic / Codex 固定 Responses
            <div className="mt-1.5 h-9 inline-flex items-center px-3.5 rounded-full bg-black/[0.04] dark:bg-white/[0.06] text-[12px] font-semibold opacity-70">
              {wireLockedLabel}
            </div>
          ) : (
            <div className="flex gap-1.5">
              {[
                { key: 'anthropic', label: copy.wireAnthropic },
                { key: 'openai', label: copy.wireOpenai },
                // Kimi 原生协议（type="kimi"）仅 Kimi Agent 提供：Kimi Code
                // 托管服务与 Kimi Platform API key 的官方接入方式。
                ...(agent === 'kimi' ? [{ key: 'kimi', label: copy.wireKimi }] : []),
              ].map(option => (
                <button
                  key={option.key}
                  data-testid={`acp-provider-wire-${option.key}`}
                  type="button"
                  onClick={() => setWireApi(option.key)}
                  className={`h-9 px-3.5 rounded-full text-[12px] font-semibold transition-colors ${
                    wireApi === option.key
                      ? 'bg-[#007AFF] text-white'
                      : isDark
                        ? 'bg-white/[0.08] text-[#C7C7CC]'
                        : 'bg-[#F0F4F9] text-[#5F6368]'
                  }`}
                >
                  {option.label}
                </button>
              ))}
            </div>
          )}
        </div>

        {anthropicEndpointMissing && (
          <div data-testid="acp-provider-no-anthropic-warning" className="mb-4 rounded-xl px-3 py-2.5 text-[12px] leading-relaxed bg-amber-500/[0.1] text-amber-700 dark:text-amber-300">
            {copy.noAnthropicEndpointWarning}
          </div>
        )}

        <div className="block mb-4">
          <span className="text-[12px] font-medium opacity-70">{copy.model} · {copy.modelOptional}</span>
          <ModelSuggestInput
            testId="acp-provider-model"
            inputClass={inputClass}
            value={model}
            onChange={changeModel}
            placeholder={copy.modelPlaceholder}
            suggestions={suggestedModels}
          />
        </div>

        {/* Claude Code 细化模型槽位：默认跟随主模型，可单独修改；必填（留空
            槽位会让 CC 子 agent 回落官方模型走官方流量）。 */}
        {agent === 'claude' && (
          <div className="mb-4">
            <span className="block text-[12px] font-medium opacity-70 mb-1.5">{copy.modelSlotsTitle}</span>
            <div className="space-y-2">
              {CLAUDE_MODEL_SLOT_IDS.map(slot => (
                <label key={slot} className="flex items-center gap-2">
                  <span className="w-20 shrink-0 text-[12px] opacity-60">{copy[`slot_${slot}`]}</span>
                  <span className="flex-1">
                    <ModelSuggestInput
                      testId={`acp-provider-slot-${slot}`}
                      inputClass={inputClass}
                      value={modelSlots[slot] || ''}
                      onChange={value =>
                        setModelSlots(slots => ({ ...slots, [slot]: value }))
                      }
                      suggestions={suggestedModels}
                    />
                  </span>
                </label>
              ))}
            </div>
            <span className="block mt-1.5 text-[12px] opacity-50">{copy.modelSlotsHint}</span>
          </div>
        )}

        {/* 上下文窗口（可选，仅 codex/kimi）：codex 写模型 catalog、
            kimi 写 max_context_size；claude 用 [1m] 变体表达，无需此字段 */}
        {agent !== 'claude' && (
          <label className="block mb-4">
            <span className="text-[12px] font-medium opacity-70">{copy.contextWindow} · {copy.modelOptional}</span>
            <input
              data-testid="acp-provider-context-window"
              className={`${inputClass} mt-1.5 font-mono`}
              value={contextWindow}
              onChange={event => setContextWindow(event.target.value.replace(/[^0-9]/g, ''))}
              placeholder={copy.contextWindowPlaceholder}
              inputMode="numeric"
              spellCheck={false}
            />
            <span className="block mt-1.5 text-[12px] opacity-50">{copy.contextWindowHint}</span>
          </label>
        )}

        <label className="block mb-4">
          <span className="text-[12px] font-medium opacity-70">{copy.apiKey}</span>
          <div className="mt-1.5 flex items-center gap-2">
            <input
              data-testid="acp-provider-api-key"
              className={`${inputClass} font-mono`}
              // 恒为 text + WebkitTextSecurity 掩码：type=password 会触发
              // WebView2 自带的眼睛按钮（与「显示」重复）
              type="text"
              style={showKey ? undefined : { WebkitTextSecurity: 'disc' }}
              value={apiKey}
              onChange={event => setApiKey(event.target.value)}
              placeholder={copy.apiKeyPlaceholder}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
            <button
              type="button"
              data-testid="acp-provider-key-toggle"
              onClick={() => setShowKey(current => !current)}
              className="shrink-0 h-10 px-3 rounded-xl text-[12px] font-medium border border-black/[0.08] dark:border-white/[0.12]"
            >
              {showKey ? copy.hideKey : copy.showKey}
            </button>
          </div>
          <span className="block mt-1.5 text-[12px] opacity-50">{copy.apiKeyHint}</span>
        </label>

        {error && (
          <div data-testid="acp-provider-form-error" className="mb-4 rounded-xl px-3 py-2.5 text-[12px] text-red-500 bg-red-500/[0.08]">
            {error}
          </div>
        )}

        <div className={`rounded-xl px-3 py-2.5 text-[12px] leading-relaxed ${isDark ? 'bg-amber-500/[0.08] text-amber-200/80' : 'bg-amber-500/[0.1] text-amber-800'}`}>
          {copy.thirdPartyWarning}
        </div>

        <div className="mt-6 flex justify-end gap-2">
          <button
            data-testid="acp-provider-form-cancel"
            onClick={onClose}
            disabled={saving}
            className="h-10 px-4 rounded-full text-[13px] font-semibold border border-black/[0.08] dark:border-white/[0.12] disabled:opacity-50"
          >
            {copy.cancel}
          </button>
          <button
            data-testid="acp-provider-form-save"
            onClick={save}
            disabled={saving}
            className="h-10 px-5 rounded-full bg-[#007AFF] text-white text-[13px] font-semibold disabled:opacity-60"
          >
            {saving ? copy.saving : copy.save}
          </button>
        </div>
      </div>
    </div>
  );
}
