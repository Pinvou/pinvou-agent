// Shared module for the "thinking depth tiers" of local/private-network openai_compatible endpoints:
// probe hook + tier pill selector. Originated in SettingsView.jsx (form entry, 400ms debounce) and
// composer-shared.jsx (session model popover entry, no debounce) with identical mechanics differing only
// in params and layout, extracted here to dedupe. Probe/selection behavior matches both originals.
import { useEffect, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';
import {
  alwaysThinkingSpecForModel,
  baseUrlUsesLocalOrPrivate,
  localReasoningTiers,
  reasoningEffortDisplayForTiers,
  reasoningEffortTiersForModel,
} from './model-catalog.js';

// Probes the local server kind (vllm/ollama/lmstudio/generic) and returns { probedKind, probePending }.
// - enabled=false (endpoint not local/private-network) synchronously clears the probe window; tiers fall back to the model catalog;
// - probePending is true from the moment a probe is scheduled (debounce window included): no tiers are offered inside the window,
//   so the user cannot pick/save a misleading tier while the probe is undecided; an unreachable probe (bridge
//   unsupported/call failed) is reset by then/catch/finally to null + pending=false, landing on the default four tiers;
// - debounceMs>0 schedules a debounced probe (the form's base_url/api_key are per-keystroke input state; without debouncing
//   every keystroke would fire a probe, and the Rust-side cache key includes port/path, so every intermediate state is a
//   new key probed serially, ~12s worst case); debounceMs=0 matches the original session popover, firing inline in the effect;
// - trimInputs: the form entry trims before probing (as the original did; the dependency array still holds the raw inputs,
//   so whitespace-only changes still reschedule the probe); the session popover entry passes saved values and needs no trim.
// In-file only: consumed by the tier hook users below; not part of the module surface.
function useLocalServerKindProbe({ enabled, baseUrl, apiKey = '', modelId = null, debounceMs = 0, trimInputs = false }) {
  const [probedKind, setProbedKind] = useState(null);
  const [probePending, setProbePending] = useState(false);
  const probeSupported = bridge.available && !!bridge.models && typeof bridge.models.probeLocalServerKind === 'function';
  useEffect(() => {
    if (!enabled) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- leaving the local-compatible state must synchronously clear the probe window so stale tiers never render one commit
      setProbedKind(null);
      setProbePending(false);
      return;
    }
    let cancelled = false;
    // Enter pending during the debounce window (round-6 P2): no tiers are
    // offered from the moment the probe is scheduled (the default four
    // tiers from localProbeTiersForKind(null) are only the fallback for an
    // unreachable probe; they must not be exposed during the "probe
    // incoming" window, or the user could pick/save a misleading tier
    // before the result is known). An unreachable probe (bridge
    // unsupported/failed) is reset by then/catch to null + pending=false,
    // landing on the default four tiers.
    setProbePending(true);
    setProbedKind(null);
    const probeBaseUrl = trimInputs ? baseUrl.trim() : baseUrl;
    const probeApiKey = trimInputs ? apiKey.trim() : apiKey;
    const runProbe = () => {
      if (probeSupported) {
        // Credentials: a freshly typed form key wins, otherwise the saved
        // model id lets Rust read the stored credential — probing an
        // authenticated vLLM (--api-key) without a key 401s into generic and
        // falsely reports "thinking tiers are not supported".
        bridge.models.probeLocalServerKind(probeBaseUrl, probeApiKey, modelId)
          .then((kind) => { if (!cancelled) setProbedKind(kind); })
          // The probe call itself failing (command rejected/unsupported version) != probing generic:
          // reset to null to take localProbeTiersForKind's default four tiers, not a false "unsupported".
          .catch(() => { if (!cancelled) setProbedKind(null); })
          .finally(() => { if (!cancelled) setProbePending(false); });
      } else {
        // Web preview has no probing capability: keep the default four tiers (matches the old behavior), no false "unsupported".
        if (!cancelled) setProbedKind(null);
        if (!cancelled) setProbePending(false);
      }
    };
    if (debounceMs > 0) {
      const timer = setTimeout(runProbe, debounceMs);
      return () => { cancelled = true; clearTimeout(timer); };
    }
    runProbe();
    return () => { cancelled = true; };
  }, [enabled, baseUrl, apiKey, modelId, debounceMs, probeSupported, trimInputs]);
  return { probedKind, probePending };
}

// Shared reasoning-tier glue for the two reasoning-effort surfaces (the composer model
// popover in composer-shared.jsx and the model form in SettingsView.jsx): resolves the
// local/private-network probe together with the static tier table into one tier state.
// Both call sites previously carried identical copies of this block; the probe mechanics
// themselves stay in useLocalServerKindProbe.
//   model        - the model descriptor driving tier routing (the composer's saved model
//                  entry, or the form's { preset, model, vendor, base_url, provider_kind }
//                  snapshot); null (composer without an active model) means "no tiers".
//   storedEffort - the effort value whose highlight is computed. The composer passes the
//                  saved model's raw reasoning_effort normalized against the model's static
//                  table; the form passes its already-normalized live state. Normalization
//                  stays with the caller — this hook only maps the value onto the resolved
//                  tiers for display (reasoningEffortDisplayForTiers semantics).
//   baseUrl/apiKey/modelId/debounceMs/trimInputs - passed through to
//                  useLocalServerKindProbe (baseUrl also feeds the local/private check; the
//                  URL parser ignores edge whitespace so callers need not trim).
// Returns { isLocalCompatible, probedKind, probePending, reasoningEffortTiers,
//           noControlThinking, reasoningEffortDisplay }.
export function useModelReasoningTierState({ model, storedEffort = null, baseUrl, apiKey = '', modelId = null, debounceMs = 0, trimInputs = false }) {
  // local_vllm 预设的本地端点同样探测：本地"检测"流程历史存量把探测到的
  // Ollama/LM Studio/Tachyon 候选都存成该预设；探测出 tachyon（驭光）时档表
  // 必须切到 ['off','low','medium','max']（其 wire 无 high、最深档是 xhigh），
  // 否则表单暴露的 high 实际只能发 medium、真实最深档 max 不可达。
  const isLocalCompatible = !!model
    && (model.preset === 'openai_compatible' || model.preset === 'local_vllm')
    && baseUrlUsesLocalOrPrivate(baseUrl);
  const { probedKind, probePending } = useLocalServerKindProbe({
    enabled: isLocalCompatible,
    baseUrl,
    apiKey,
    modelId,
    debounceMs,
    trimInputs,
  });
  // local_vllm 预设只有探测出 tachyon 才跟随探测档表：Rust 侧 provider() 的
  // LocalVllm 臂只对 Tachyon 翻转方言，其余探测结果（含 Ollama/LM Studio 的
  // 历史存量）仍走 vllm wire，档表保持与静态四档一致，避免 UI 与实际路线劈叉。
  // openai_compatible 预设的所有探测结果都由探测路由消费，维持原语义。
  const probeKindForRoute = model && model.preset === 'local_vllm'
    ? (probedKind === 'tachyon' ? 'tachyon' : null)
    : probedKind;
  // local_vllm 预设没有 pending 隐藏窗：未探测路线已知是 vllm wire，pending 期
  // 直接给静态四档（默认 low），探测返回 tachyon 才切 ['off','low','medium',
  // 'max']。手动添加本地模型的表单契约（settings_ui_smoke ⑥.5b）要求档位
  // 立即可见且默认选中「低」，pending 期隐藏档位会让保存写入显式 high、绕过
  // bridge 的本地默认档约束；隐藏窗是 ⑥.5d 为 openai_compatible「引擎类型
  // 未知」端点设计的，不适用于已知 wire 的本地预设。这也消除了 pending 窗
  // 内保存把 reasoning_effort 写成 null 的问题。
  const pendingHidesTiers = probePending && model?.preset !== 'local_vllm';
  const reasoningEffortTiers = isLocalCompatible
    ? (pendingHidesTiers ? [] : (localReasoningTiers(model.model, probeKindForRoute) || []))
    : (model ? (reasoningEffortTiersForModel(model) || []) : []);
  // Local routes (vllm preset / local-compatible endpoints) hit the "always-thinking,
  // no-control" knowledge table: the effort-tier area shows an "always on" hint instead of
  // probe-unsupported.
  const noControlThinking = !!model
    && (model.preset === 'local_vllm' || isLocalCompatible)
    && !!(alwaysThinkingSpecForModel(model.model) || {}).noControl;
  // Highlight fallback: the display maps a stored tier missing from the (possibly
  // probed-narrowed) table to the nearest tier (think:true is equivalent to high), while
  // click/save comparison keeps the caller's original value.
  const reasoningEffortDisplay = reasoningEffortDisplayForTiers(storedEffort, reasoningEffortTiers);
  return { isLocalCompatible, probedKind, probePending, reasoningEffortTiers, noControlThinking, reasoningEffortDisplay };
}

// Tier pill selector: non-empty tiers render the pill group, empty tiers render a hint line ("probing"/"always
// on"/"unsupported"). variant only switches layout and tone (composer = compact session-popover pills, form =
// right-aligned form row); the DOM structure and class strings are verbatim from the two original implementations.
// The error line (e.g. effortSaveError) and title/leading label stay with the caller — popover and form place those differently.
//   t          - i18n copy (tier names from form: t.uiSettingsDetail.reasoningEffortTiers /
//                composer: t.thinkingDepthTiers; hint lines from t.uiSettingsDetail)
//   tiers      - tier list (empty = render the hint line)
//   selected   - currently highlighted tier (display value already normalized against the tier table)
//   onSelect   - (tier) => void
//   pending    - probe in progress (gray "probing service type…" hint line)
//   noControlThinking - "thinking is always on and cannot be disabled" hint (takes precedence over "unsupported")
//   variant    - 'composer' | 'form'
export function ReasoningTierPicker({ t, tiers, selected, onSelect, pending, noControlThinking, variant = 'composer' }) {
  if (!Array.isArray(tiers) || tiers.length === 0) {
    const detail = (t && t.uiSettingsDetail) || {};
    const text = pending
      ? detail.reasoningProbePending
      : noControlThinking
        ? detail.reasoningThinkingAlwaysOn
        : detail.reasoningProbeUnsupported;
    const tone = pending
      ? (variant === 'form' ? 'text-[#8A8A8E] dark:text-[#98989D]' : 'text-gray-400 dark:text-gray-500')
      : 'text-[#FF9500] dark:text-[#FFB340]';
    return variant === 'form'
      ? <span className={`ml-auto text-right text-[12px] leading-4 ${tone}`}>{text}</span>
      : <div className={`text-[11px] leading-4 ${tone}`}>{text}</div>;
  }
  const tierLabels = variant === 'form'
    ? ((t && t.uiSettingsDetail && t.uiSettingsDetail.reasoningEffortTiers) || {})
    : ((t && t.thinkingDepthTiers) || {});
  return (
    <div className={variant === 'form' ? 'ml-auto flex flex-wrap justify-end gap-1' : 'flex flex-wrap gap-1'}>
      {tiers.map(tier => (
        <button
          key={tier}
          type="button"
          onClick={() => onSelect(tier)}
          className={selected === tier
            ? (variant === 'form'
              ? 'h-7 min-w-[52px] px-3 rounded-full text-[13px] font-medium transition-colors bg-[#007AFF] text-white dark:bg-[#0A84FF]'
              : 'h-7 min-w-[48px] px-2.5 rounded-full text-[12px] font-medium transition-colors bg-[#007AFF] text-white')
            : (variant === 'form'
              ? 'h-7 min-w-[52px] px-3 rounded-full text-[13px] font-medium transition-colors bg-[#E5E5EA] text-[#636366] hover:bg-[#D9D9DE] dark:bg-white/[0.07] dark:text-[#C7C7CC] dark:hover:bg-white/[0.12]'
              : 'h-7 min-w-[48px] px-2.5 rounded-full text-[12px] font-medium transition-colors bg-black/[0.05] dark:bg-white/[0.08] text-gray-600 dark:text-gray-300 hover:bg-black/[0.09] dark:hover:bg-white/[0.13]')
          }
        >{tierLabels[tier] || tier}</button>
      ))}
    </div>
  );
}
