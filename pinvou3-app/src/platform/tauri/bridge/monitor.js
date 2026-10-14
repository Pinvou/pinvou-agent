/**
 * monitor feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  // biome-ignore lint/suspicious/noRedundantUseStrict: verbatim copy of a classic-script artifact; strict mode is part of the payload
  "use strict";
  // biome-ignore lint/suspicious/noAssignInExpressions: registry bootstrap of the verbatim payload; splitting the statements would diverge from the artifact
  const registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["monitor"] = function (context) {let pinvouSharedtauriMonitorCache = null;
function pinvouSharedtauriMonitor() {
  if (!pinvouSharedtauriMonitorCache) pinvouSharedtauriMonitorCache = window.PinvouBridgeShared.create("tauriMonitor", { monitorBaseline: { get value() { return monitorBaseline; }, set value(v) { monitorBaseline = v; } }, clearMonitorBaseline, monitorIntervalId: { get value() { return monitorIntervalId; }, set value(v) { monitorIntervalId = v; } }, gpuUtilHistory: { get value() { return gpuUtilHistory; }, set value(v) { gpuUtilHistory = v; } }, monitorPollInFlight: { get value() { return monitorPollInFlight; }, set value(v) { monitorPollInFlight = v; } }, pollMonitor });
  return pinvouSharedtauriMonitorCache;
}


    const state = context.state;
    const notify = context.notify;
    const invoke = context.invoke;
    let monitorIntervalId = null;
    let monitorPollInFlight = false;
    let gpuUtilHistory = [];
    // state.tokens.max is written back only for real max_model_len values from
    // get_backend_status or the monitor snapshot (both assignment sites are
    // truthiness-guarded).
    const MONITOR_BASELINE_KEY = "pinvou3.monitorStatsBaseline.self";
    let monitorBaseline = null;
    try {
      const storedBaseline = localStorage.getItem(MONITOR_BASELINE_KEY);
      if (storedBaseline) monitorBaseline = JSON.parse(storedBaseline);
    } catch { monitorBaseline = null; }
  // ── Monitor ──────────────────────────────────────────────────────
  // pollMonitor/appQueueSnapshot/monitorFmtEqual 与格式化 fallback 统一在
  // shared/bridge-shared-helpers.js 的 tauriMonitor cluster（betaVersionSuffix
  // 键两侧同值同名）；此处仅保留 lane 内转发的公开入口。

function numOr0(x) { return pinvouSharedtauriMonitor().numOr0(x); }

  // 用基准点把累计 counter 换算成「自清除以来」的区间值。sp=app 自测(snap.self_perf,
  // TTFT/TPS/tokens 全从这);v=vllm(仅 KV 的本地 prefix_cache 分支要它)。无基准 → 直接
  // 用进程生命周期累计值。任一 counter 倒退（< 基准：app 或 vLLM 重启、counter 归零）
  // → 丢弃失效基准，回落到累计值，避免负数。
  // KV 命中率(混合):本地 vLLM 用 /metrics prefix_cache(vllmKvPct);拿不到再用 usage 的
  // cache token 口径(selfKvPct,给云端/D3)。二者都按区间(扣基准)重算。

  function clearMonitorBaseline() {
    monitorBaseline = null;
    try { localStorage.removeItem(MONITOR_BASELINE_KEY); } catch { /* ignore when localStorage is unavailable */ }
  }

  // 把当前 counter 快照存为基准点 → 监控页「后 4 项」从此刻起重新计。
  // 自测计数(TTFT/TPS/tokens/usage-cache)+ vLLM prefix_cache(供本地 KV 分支)一起存。
  function clearMonitorStats() {
    const sp = state.monitor && state.monitor.self_perf;
    if (!sp) return false;
    const v = (state.monitor && state.monitor.vllm) || {};
    monitorBaseline = {
      ttft_sum_s: numOr0(sp.ttft_sum_s),
      ttft_count: numOr0(sp.ttft_count),
      tps_tokens: numOr0(sp.tps_tokens),
      tps_time_s: numOr0(sp.tps_time_s),
      gen_tokens: numOr0(sp.gen_tokens_total),
      prompt_tokens: numOr0(sp.prompt_tokens_total),
      cache_hit: numOr0(sp.cache_hit_tokens),
      cache_miss: numOr0(sp.cache_miss_tokens),
      pc_hits: numOr0(v.prefix_cache_hits),
      pc_queries: numOr0(v.prefix_cache_queries),
      at: Date.now(),  // 记录清除时刻，供「统计自 HH:MM 起」状态文字
    };
    try { localStorage.setItem(MONITOR_BASELINE_KEY, JSON.stringify(monitorBaseline)); } catch { /* ignore when localStorage is unavailable */ }
    pollMonitor();  // 立即刷新显示，无需等下一个轮询周期
    return true;
  }

async function pollMonitor() { return pinvouSharedtauriMonitor().pollMonitor(); }

function startMonitorPolling() { return pinvouSharedtauriMonitor().startMonitorPolling(); }
function stopMonitorPolling() { return pinvouSharedtauriMonitor().stopMonitorPolling(); }

  // ── Backend status (live dot) ────────────────────────────────────
  async function pollBackendStatus() {
    try {
      const s = await invoke("get_backend_status");
      state.backendOnline = !!s.vllm_online;
      // 修 token 分母时机 bug：不再依赖用户打开监控页才拿到真实 max_model_len
      if (s.max_model_len) {
        state.tokens.max = s.max_model_len;
      }
    } catch {
      state.backendOnline = false;
    }
    notify();
  }

    return {
      startMonitorPolling,
      stopMonitorPolling,
      clearMonitorStats,
      pollBackendStatus
    };
  };
})(window);
