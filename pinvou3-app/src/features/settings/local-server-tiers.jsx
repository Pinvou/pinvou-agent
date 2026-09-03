// 本地/私网 openai_compatible 端点的「思考深度档位」共享件：探测 hook +
// 档位胶囊选择器。原生于 SettingsView.jsx（表单入口，400ms 防抖）与
// composer-shared.jsx（会话模型弹层入口，不防抖），两处机制一致仅参数与
// 排版不同，抽到本模块消重。探测/选择行为与两处原实现一致。
import { useEffect, useState } from 'react';
import { bridge } from '../../hooks/useBridge.js';

// 探测本地服务类型（vllm/ollama/lmstudio/generic）并返回 { probedKind, probePending }。
// - enabled=false（端点非本地/私网）时同步清空探测窗口，档位回落到按模型目录；
// - probePending 自探测排程起即为 true（含防抖窗口）：窗口内不下发档位，
//   避免用户在「探测未定」时选中/保存误导档位；探测不可达（bridge 不支持/
//   调用失败）由 then/catch/finally 复位为 null + pending=false，落到默认四档；
// - debounceMs>0 时防抖排程（表单 base_url/api_key 是逐键输入 state，不防抖
//   会逐键触发探测，Rust 侧缓存 key 含端口/路径，每个中间态都是新 key、各自
//   串行探测最坏 ~12s）；debounceMs=0 时与原会话弹层实现一致，effect 内同步发起；
// - trimInputs：表单入口探测前 trim（与原实现一致，依赖数组仍是原始输入，
//   纯空白变化仍会重排程探测）；会话弹层入口传保存值，无需 trim。
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
          // 探测调用本身失败（命令被拒/版本不支持）≠ 探测出 generic：
          // 置回 null 走 localProbeTiersForKind 的默认四档，不误报「不支持」。
          .catch(() => { if (!cancelled) setProbedKind(null); })
          .finally(() => { if (!cancelled) setProbePending(false); });
      } else {
        // web 预览无探测能力：保持默认四档（与旧行为一致），不误报不支持。
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

// 档位提示的 zh 兜底串：与 i18n 文案同语义，仅 t.uiSettingsDetail 缺 key 时
// 兜底（与抽离前两处调用的内联兜底一致）。
const TIER_FALLBACK_COPY = {
  pending: '正在探测服务类型…',
  alwaysOn: '该模型思考始终开启，无法关闭',
  unsupported: '该端点不支持思考档位调节',
};

// 档位胶囊选择器：tiers 非空渲染胶囊组，为空渲染「探测中/常开/不支持」提示行。
// variant 只切换排版与色调（composer=会话弹层紧凑胶囊，form=表单右对齐行），
// DOM 结构与类串与两处原实现逐字一致；错误行（如 effortSaveError）与标题/
// 前置标签仍由调用方渲染——弹层与表单的错误行、行布局位置不同。
//   t          - i18n 文案（档位名取 form: t.uiSettingsDetail.reasoningEffortTiers /
//                composer: t.thinkingDepthTiers；提示行取 t.uiSettingsDetail）
//   tiers      - 档位列表（空 = 渲染提示行）
//   selected   - 当前高亮档位（已按档位表归一的显示值）
//   onSelect   - (tier) => void
//   pending    - 探测进行中（提示行灰色「正在探测服务类型…」）
//   noControlThinking - 「思考始终开启，无法关闭」提示（优先级高于「不支持」）
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
