// Shared module for the "thinking depth tiers" of local/private-network openai_compatible endpoints:
// probe hook + tier pill selector. Originated in SettingsView.jsx (form entry, 400ms debounce) and
// composer-shared.jsx (session model popover entry, no debounce) with identical mechanics differing only
// in params and layout, extracted here to dedupe. Probe/selection behavior matches both originals.
import { useEffect, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';

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
export function useLocalServerKindProbe({ enabled, baseUrl, apiKey = '', modelId = null, debounceMs = 0, trimInputs = false }) {
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

// zh fallback strings for the tier hints: same semantics as the i18n copy, used only when
// t.uiSettingsDetail lacks the key (matching the inline fallbacks at both call sites before extraction).
const TIER_FALLBACK_COPY = {
  pending: '正在探测服务类型…',
  alwaysOn: '该模型思考始终开启，无法关闭',
  unsupported: '该端点不支持思考档位调节',
};

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
      ? (detail.reasoningProbePending || TIER_FALLBACK_COPY.pending)
      : noControlThinking
        ? (detail.reasoningThinkingAlwaysOn || TIER_FALLBACK_COPY.alwaysOn)
        : (detail.reasoningProbeUnsupported || TIER_FALLBACK_COPY.unsupported);
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
