/**
 * monitor feature for the Tauri bridge.
 * Registered before bridge.js builds the backwards-compatible facade.
 */
(function (root) {
  "use strict";
  var registry = root.__PINVOU_TAURI_BRIDGE_FEATURES__ = root.__PINVOU_TAURI_BRIDGE_FEATURES__ || {};
  registry["monitor"] = function (context) {
    var state = context.state;
    var notify = context.notify;
    var invoke = context.invoke;
    var bt = context.bt;
    var safeConsoleInfo = context.safeConsoleInfo;
    var sessionStates = context.sessionStates;
    var monitorIntervalId = null;
    var monitorPollInFlight = false;
    var gpuUtilHistory = [];
    var maxModelLen = state.tokens.max || 32768;
    var MONITOR_BASELINE_KEY = "pinvou3.monitorStatsBaseline.self";
    var monitorBaseline = null;
    try {
      var storedBaseline = localStorage.getItem(MONITOR_BASELINE_KEY);
      if (storedBaseline) monitorBaseline = JSON.parse(storedBaseline);
    } catch (error) { monitorBaseline = null; }
  // ── Monitor ──────────────────────────────────────────────────────
  var PinvouFU = window.PinvouFormatUtils || {};
  var fmtMiB = PinvouFU.fmtMiB || function (mib) { return mib == null ? "—" : String(mib); };
  var fmtKiB = PinvouFU.fmtKiB || function (kib) { return kib == null ? "—" : String(kib); };
  var fmtDuration = PinvouFU.fmtDuration || function (secs) { return secs == null ? "—" : String(secs); };
  var fmtTok = PinvouFU.fmtTok || function (n) { return n == null ? "—" : String(n); };


  function numOr0(x) { return (typeof x === "number" && isFinite(x)) ? x : 0; }

  // 用基准点把累计 counter 换算成「自清除以来」的区间值。sp=app 自测(snap.self_perf,
  // TTFT/TPS/tokens 全从这);v=vllm(仅 KV 的本地 prefix_cache 分支要它)。无基准 → 直接
  // 用进程生命周期累计值。任一 counter 倒退（< 基准：app 或 vLLM 重启、counter 归零）
  // → 丢弃失效基准，回落到累计值，避免负数。
  // KV 命中率(混合):本地 vLLM 用 /metrics prefix_cache(vllmKvPct);拿不到再用 usage 的
  // cache token 口径(selfKvPct,给云端/D3)。二者都按区间(扣基准)重算。
  function adjustCounters(sp, v) {
    sp = sp || {};
    var kvRatio = function (hit, miss) {
      var d = hit + miss;
      return d > 0 ? (hit / d * 100) : null;
    };
    var b = monitorBaseline;
    if (b) {
      var reset =
        numOr0(sp.ttft_sum_s) < b.ttft_sum_s ||
        numOr0(sp.tps_time_s) < b.tps_time_s ||
        numOr0(sp.gen_tokens_total) < b.gen_tokens ||
        numOr0(sp.prompt_tokens_total) < b.prompt_tokens ||
        numOr0(sp.cache_hit_tokens) < b.cache_hit ||
        numOr0(sp.cache_miss_tokens) < b.cache_miss ||
        (v && numOr0(v.prefix_cache_queries) < numOr0(b.pc_queries));
      if (reset) { clearMonitorBaseline(); b = null; }
    }
    var base = function (k) { return b ? numOr0(b[k]) : 0; };
    var vllmKvPct = null;
    if (v) {
      var pcH = numOr0(v.prefix_cache_hits) - base("pc_hits");
      var pcQ = numOr0(v.prefix_cache_queries) - base("pc_queries");
      vllmKvPct = pcQ > 0 ? (pcH / pcQ * 100) : null;
    }
    return {
      cleared: !!b,
      ttft_sum_s: numOr0(sp.ttft_sum_s) - base("ttft_sum_s"),
      ttft_count: numOr0(sp.ttft_count) - base("ttft_count"),
      tps_tokens: numOr0(sp.tps_tokens) - base("tps_tokens"),
      tps_time_s: numOr0(sp.tps_time_s) - base("tps_time_s"),
      gen: numOr0(sp.gen_tokens_total) - base("gen_tokens"),
      prompt: numOr0(sp.prompt_tokens_total) - base("prompt_tokens"),
      vllmKvPct: vllmKvPct,
      selfKvPct: kvRatio(
        numOr0(sp.cache_hit_tokens) - base("cache_hit"),
        numOr0(sp.cache_miss_tokens) - base("cache_miss")
      ),
      clearedAt: b ? (b.at || null) : null,
    };
  }

  function clearMonitorBaseline() {
    monitorBaseline = null;
    try { localStorage.removeItem(MONITOR_BASELINE_KEY); } catch (e) {}
  }

  // 把当前 counter 快照存为基准点 → 监控页「后 4 项」从此刻起重新计。
  // 自测计数(TTFT/TPS/tokens/usage-cache)+ vLLM prefix_cache(供本地 KV 分支)一起存。
  function clearMonitorStats() {
    var sp = state.monitor && state.monitor.self_perf;
    if (!sp) return false;
    var v = (state.monitor && state.monitor.vllm) || {};
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
    try { localStorage.setItem(MONITOR_BASELINE_KEY, JSON.stringify(monitorBaseline)); } catch (e) {}
    pollMonitor();  // 立即刷新显示，无需等下一个轮询周期
    return true;
  }

  function appQueueSnapshot() {
    var running = 0;
    var waiting = state.queued ? state.queued.length : 0;
    var busyMap = {};
    for (var id in sessionStates) {
      if (!Object.prototype.hasOwnProperty.call(sessionStates, id)) continue;
      if (id === state.activeSessionId) continue;
      var buf = sessionStates[id] || {};
      if (buf.busy) busyMap[id] = true;
      if (Array.isArray(buf.queued)) waiting += buf.queued.length;
    }
    if (state.activeSessionId && state.busy) busyMap[state.activeSessionId] = true;
    running = Object.keys(busyMap).length;
    return { running: running, waiting: waiting };
  }

  async function pollMonitor() {
    if (monitorPollInFlight) return;
    monitorPollInFlight = true;
    try {
      var snap = await invoke("get_monitor_snapshot");
      state.monitorError = null;
      // GPU util sliding window
      if (snap.gpu) {
        gpuUtilHistory.push(snap.gpu.utilization_pct);
        if (gpuUtilHistory.length > 5) gpuUtilHistory.shift();
        snap.gpu._utilMax = Math.max.apply(null, [0].concat(gpuUtilHistory));
      }
      // 监控页「后 4 项」累计指标：TTFT/TPS/tokens 来自 app 侧自测(snap.self_perf,
      // 任何后端都有);KV 混合(本地 vLLM prefix_cache 优先,否则 usage 口径)。
      // 按「清除统计」基准点换算成区间值后再格式化。
      var sadj = adjustCounters(snap.self_perf, snap.vllm);
      // KV 显示值:本地 vLLM 的 /metrics prefix_cache 优先,拿不到用 usage cache 口径(云端)。
      var kvShown = sadj ? (sadj.vllmKvPct != null ? sadj.vllmKvPct
        : (sadj.selfKvPct != null ? sadj.selfKvPct : null)) : null;
      // Format values for display
      var vllm = snap.vllm || null;
      var metricsApplicable = vllm ? vllm.metrics_applicable !== false : false;
      var metricNotApplicableText = bt("metricNotApplicable");
      var metricUnavailableText = bt("metricUnavailable");
      var diagnostic = vllm && vllm.diagnostic ? vllm.diagnostic : null;
      var metricDiagnostic = vllm && vllm.metric_diagnostics && vllm.metric_diagnostics.length
        ? vllm.metric_diagnostics[0] : null;
      var targetKind = vllm && vllm.target_kind ? vllm.target_kind : "invalid";
      var targetKindLabel = targetKind === "remote" ? bt("targetKindRemote") : (targetKind === "local" ? bt("targetKindLocal") : bt("targetKindInvalid"));
      var vllmDisplayModel = vllm ? (vllm.model || vllm.configured_model || "—") : "—";
      var healthStatus = vllm && vllm.health_status ? vllm.health_status : (vllm ? "verified" : "offline");
      var appQueue = appQueueSnapshot();
      var cpu = snap.cpu || null;
      var cpuUsage = cpu && typeof cpu.total_usage_pct === "number" && isFinite(cpu.total_usage_pct)
        ? Math.round(Math.max(0, Math.min(100, cpu.total_usage_pct)))
        : null;
      var computeName = snap.gpu ? snap.gpu.name : (cpu && cpu.name ? cpu.name : bt("gpuUnavailable"));
      snap._fmt = {
        gpuName: computeName,
        cpuName: cpu && cpu.name ? cpu.name : "",
        cpuAvailable: !!cpu,
        computeAvailable: !!(snap.gpu || cpu),
        computeName: computeName,
        gpuVram: snap.gpu && snap.gpu.vram_total_mib > 0
          ? fmtMiB(snap.gpu.vram_used_mib) + " / " + fmtMiB(snap.gpu.vram_total_mib) : "—",
        gpuVramPct: snap.gpu && snap.gpu.vram_total_mib > 0
          ? Math.round(snap.gpu.vram_used_mib / snap.gpu.vram_total_mib * 100) : 0,
        gpuUtil: snap.gpu ? (snap.gpu._utilMax + "%") : "—",
        gpuUtilPct: snap.gpu ? snap.gpu._utilMax : 0,
        processorUtil: cpuUsage != null
          ? cpuUsage + "%"
          : (snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct + "%" : "—"),
        processorUtilPct: cpuUsage != null
          ? cpuUsage
          : (snap.gpu && snap.gpu.processor_utilization_pct != null ? snap.gpu.processor_utilization_pct : 0),
        gpuSharedMemory: snap.gpu && snap.gpu.shared_memory_used_mib != null ? fmtMiB(snap.gpu.shared_memory_used_mib) : "—",
        gpuTemp: snap.gpu && snap.gpu.temperature_c != null ? snap.gpu.temperature_c + "°C" : null,
        gpuPower: snap.gpu && snap.gpu.power_w != null ? snap.gpu.power_w.toFixed(1) + " W" : null,
        gpuAvailable: !!snap.gpu,
        gpuHasVram: !!(snap.gpu && snap.gpu.vram_total_mib > 0),
        ramUsed: snap.ram ? fmtKiB(snap.ram.used_kib) : "—",
        ramTotal: snap.ram ? fmtKiB(snap.ram.total_kib) : "—",
        ramPct: snap.ram && snap.ram.total_kib > 0 ? Math.round(snap.ram.used_kib / snap.ram.total_kib * 100) : 0,
        ramUsedGiB: snap.ram ? (snap.ram.used_kib / 1024 / 1024).toFixed(1) : "—",
        swapUsed: snap.ram ? fmtKiB(snap.ram.swap_used_kib) : "—",
        swapTotal: snap.ram ? fmtKiB(snap.ram.swap_total_kib) : "—",
        swapPct: snap.ram && snap.ram.swap_total_kib > 0 ? Math.round(snap.ram.swap_used_kib / snap.ram.swap_total_kib * 100) : 0,
        vllmModel: vllmDisplayModel,
        vllmConfiguredModel: vllm ? (vllm.configured_model || null) : null,
        vllmModelMismatch: vllm && vllm.configured_model && vllm.model
          ? vllm.configured_model !== vllm.model : false,
        vllmStatus: vllm ? vllm.status.toUpperCase() : "OFFLINE",
        vllmHealthStatus: healthStatus,
        vllmOnline: vllm ? (healthStatus === "verified" && (vllm.status === "ready" || vllm.status === "busy")) : false,
        vllmUpstream: vllm ? (vllm.upstream || "—") : "—",
        vllmTargetKind: targetKindLabel,
        // 云端(remote)不做健康探测(无 auth 的 /v1/models 必 401)→ 不显示 OFFLINE。
        // 暴露原始 kind 供前端判定(别比本地化 label)。
        vllmIsRemote: targetKind === "remote",
        vllmDiagnostic: diagnostic ? diagnostic.message : null,
        vllmDiagnosticCode: diagnostic ? diagnostic.code : null,
        vllmMetricsApplicable: metricsApplicable,
        vllmMetricDiagnostic: metricDiagnostic ? metricDiagnostic.message : null,
        vllmMaxLen: vllm ? (metricsApplicable ? (vllm.max_model_len || "—") : (vllm.max_model_len || metricUnavailableText)) : "—",
        // 本地推理引擎(target_kind=local)且探测窗口 < 128k(131072):监控卡给告警。
        // 云端(remote)/v1/models 不返回 max_model_len,自然不触发。传原始值供前端拼文案。
        vllmCtxWarn: (vllm && targetKind === "local" && vllm.max_model_len && vllm.max_model_len < 131072)
          ? vllm.max_model_len : null,
        vllmQueue: appQueue.running + " / " + appQueue.waiting,
        vllmQueueSource: "app",
        // TTFT/TPS/tokens 一律用 app 侧自测——任何后端(vLLM/LM Studio/Ollama/云端)都有值,
        // 不再受 metricsApplicable 门控。KV 见 kvShown(本地 prefix_cache / 云端 usage 口径),
        // 拿不到则 "—"。队列仍归 vLLM(见 vllmQueue)。
        vllmKv: kvShown != null ? kvShown.toFixed(1) + "%" : "0%",
        vllmKvHasData: kvShown != null,
        vllmTtft: sadj && sadj.ttft_count > 0
          ? (sadj.ttft_sum_s / sadj.ttft_count).toFixed(2) + " s" : "—",
        vllmTps: sadj && sadj.tps_time_s > 0
          ? (sadj.tps_tokens / sadj.tps_time_s).toFixed(1) + " tok/s" : "—",
        vllmTokTotal: sadj
          ? fmtTok(sadj.gen) + " / " + fmtTok(sadj.prompt) : "—",
        vllmStatsCleared: !!(sadj && sadj.cleared),
        vllmClearedAt: sadj && sadj.cleared ? (sadj.clearedAt || null) : null,
        // 区间原始数值（已扣基准），供前端「长按清除」的数字归零插值动画用。
        vllmRaw: sadj ? {
          kvPct: kvShown,
          ttftS: sadj.ttft_count > 0 ? sadj.ttft_sum_s / sadj.ttft_count : null,
          tps: sadj.tps_time_s > 0 ? sadj.tps_tokens / sadj.tps_time_s : null,
          gen: sadj.gen != null ? sadj.gen : null,
          prompt: sadj.prompt != null ? sadj.prompt : null,
        } : null,
        appVersion: snap.app ? snap.app.pinvou3_version + bt("betaVersionSuffix") : "—",
        dtVersion: snap.app ? snap.app.deepseek_tui_version : "—",
        uptime: snap.app ? fmtDuration(snap.app.session_uptime_secs) : "—",
        updatedAt: snap.generated_at_ms ? new Date(snap.generated_at_ms).toLocaleTimeString() : "—",
      };
      if (snap.vllm && snap.vllm.max_model_len) {
        maxModelLen = snap.vllm.max_model_len;
        state.tokens.max = maxModelLen;
      }
      state.monitor = snap;
      notify();
    } catch (e) {
      state.monitorError = e && e.message ? e.message : String(e || "monitor poll failed");
      console.warn("monitor poll failed", e);
      notify();
    } finally {
      monitorPollInFlight = false;
    }
  }

  function startMonitorPolling() {
    if (monitorIntervalId) return;
    gpuUtilHistory = [];
    pollMonitor();
    monitorIntervalId = setInterval(pollMonitor, 1000);
  }
  function stopMonitorPolling() {
    if (monitorIntervalId) {
      clearInterval(monitorIntervalId);
      monitorIntervalId = null;
    }
  }

  // ── Backend status (live dot) ────────────────────────────────────
  async function pollBackendStatus() {
    try {
      var s = await invoke("get_backend_status");
      state.backendOnline = !!s.vllm_online;
      // 修 token 分母时机 bug：不再依赖用户打开监控页才拿到真实 max_model_len
      if (s.max_model_len) {
        maxModelLen = s.max_model_len;
        state.tokens.max = maxModelLen;
      }
    } catch (e) {
      state.backendOnline = false;
    }
    notify();
  }

    return {
      startMonitorPolling: startMonitorPolling,
      stopMonitorPolling: stopMonitorPolling,
      clearMonitorStats: clearMonitorStats,
      pollBackendStatus: pollBackendStatus
    };
  };
})(window);
