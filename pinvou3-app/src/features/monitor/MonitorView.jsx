import React, { useCallback, useEffect, useRef, useState } from 'react';
import { AlertTriangle, Brain, BrainCircuit, CheckCircle2, Clock, Cpu, Database, RefreshCw, RotateCcw, Server } from '../../components/icons.jsx';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ListRow, ProgressBar, WidgetCard } from '../workflow/WorkflowView.jsx';

// 界面语言 → BCP 47 locale，用于时钟等本地化格式化
const MONITOR_CLOCK_LOCALE = { zh: 'zh-CN', en: 'en-US', ja: 'ja-JP' };

const ClearStatsHold = ({ theme, t, onClear }) => {
      const HOLD_MS = 850;
      const [fillPct, setFillPct] = useState(0);
      const [phase, setPhase] = useState('idle');  // idle | holding | done
      const rafRef = useRef(0);
      const holdStartRef = useRef(0);
      const committedRef = useRef(false);
      const activeRef = useRef(false);
      const resetTimerRef = useRef(0);

      const commit = () => {
        if (committedRef.current) return;
        committedRef.current = true;
        activeRef.current = false;
        cancelAnimationFrame(rafRef.current);
        setFillPct(100);
        setPhase('done');
        onClear && onClear();
        resetTimerRef.current = setTimeout(() => {
          setFillPct(0);
          setPhase('idle');
          committedRef.current = false;
        }, 900);
      };
      const tick = (now) => {
        const p = Math.min((now - holdStartRef.current) / HOLD_MS, 1);
        setFillPct(p * 100);
        if (p >= 1) { commit(); return; }
        rafRef.current = requestAnimationFrame(tick);
      };
      const begin = () => {
        if (activeRef.current || committedRef.current) return;
        activeRef.current = true;
        setPhase('holding');
        holdStartRef.current = performance.now();
        rafRef.current = requestAnimationFrame(tick);
      };
      const cancel = () => {
        if (!activeRef.current || committedRef.current) return;
        activeRef.current = false;
        cancelAnimationFrame(rafRef.current);
        setPhase('idle');
        setFillPct(0);
      };
      useEffect(() => () => { cancelAnimationFrame(rafRef.current); clearTimeout(resetTimerRef.current); }, []);

      const label = phase === 'done' ? t.clearDone : phase === 'holding' ? t.clearHolding : t.clearHold;
      const toneClass = phase === 'done'
        ? 'text-[#1f9d51] border-[#bfe7cc] dark:text-[#93D5A6] dark:border-[#2c5234]'
        : phase === 'holding'
          ? 'text-[#dc2f44] border-[#f1c4cb] dark:text-[#F28B82] dark:border-[#7a3b3b]'
          : 'text-[#5b6473] border-[#e3e7ec] hover:bg-[#fafbfc] dark:text-[#C4C7C5] dark:border-[#3c4043] dark:hover:bg-[#2a2b2d]';
      return (
        <button
          type="button"
          aria-label={t.clearHold}
          onMouseDown={(e) => { e.preventDefault(); begin(); }}
          onMouseUp={cancel}
          onMouseLeave={cancel}
          onTouchStart={(e) => { e.preventDefault(); begin(); }}
          onTouchEnd={(e) => { e.preventDefault(); cancel(); }}
          onTouchCancel={cancel}
          onKeyDown={(e) => { if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) { e.preventDefault(); begin(); } }}
          onKeyUp={(e) => { if (e.key === ' ' || e.key === 'Enter') { e.preventDefault(); cancel(); } }}
          onBlur={cancel}
          className={`relative overflow-hidden flex-shrink-0 inline-flex items-center text-[13px] font-medium px-4 py-2 rounded-[9px] border select-none transition-colors ${toneClass} bg-white dark:bg-[#1a1b1c]`}
        >
          <span
            className="absolute left-0 top-0 bottom-0 z-0 bg-[#fce7ea] dark:bg-[rgba(220,47,68,0.24)]"
            style={{ width: fillPct + '%', transition: activeRef.current ? 'none' : 'width .22s ease' }}
          ></span>
          <span className="relative z-[1] inline-flex items-center gap-1.5">
            <RotateCcw size={15} style={{ animation: phase === 'done' ? 'tsSpinner .5s ease' : 'none' }} />
            {label}
          </span>
        </button>
      );
    };

    const MONITOR_BRAND_ICONS = {
      qwen: 'brand-icons/qwen.svg',
      deepseek: 'brand-icons/deepseek.svg',
      kimi: 'brand-icons/kimi.svg',
      minimax: 'brand-icons/minimax.svg',
      glm: 'brand-icons/glm.svg',
      nvidia: 'brand-icons/nvidia.png',
      intel: 'brand-icons/intel.svg',
    };
    const monitorModelIcon = (name) => {
      const v = String(name || '').toLowerCase();
      if (v.includes('deepseek')) return MONITOR_BRAND_ICONS.deepseek;
      if (v.includes('kimi') || v.includes('moonshot')) return MONITOR_BRAND_ICONS.kimi;
      if (v.includes('minimax')) return MONITOR_BRAND_ICONS.minimax;
      if (v.includes('glm') || v.includes('chatglm') || v.includes('zhipu')) return MONITOR_BRAND_ICONS.glm;
      if (v.includes('qwen') || v.includes('tongyi') || v.includes('千问')) return MONITOR_BRAND_ICONS.qwen;
      return null;
    };
    const monitorProcessorIcon = (name) => {
      const v = String(name || '').toLowerCase();
      if (v.includes('nvidia')) return MONITOR_BRAND_ICONS.nvidia;
      if (v.includes('intel')) return MONITOR_BRAND_ICONS.intel;
      return null;
    };
    const monitorClampPct = (n) => Math.max(0, Math.min(100, Math.round(Number(n) || 0)));
    const monitorShortNum = (n) => {
      if (n == null || !isFinite(n)) return '—';
      if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
      if (n >= 1e3) return (n / 1e3).toFixed(1) + 'k';
      return String(Math.round(n));
    };
    const monitorTokenPair = (value) => {
      const parts = String(value || '').split('/').map(s => s.trim());
      return { output: parts[0] || '—', input: parts[1] || '—' };
    };
    const monitorShortProcessorName = (name) => String(name || '')
      .replace(/\(R\)|\(TM\)|\(C\)/g, '')
      .replace(/\s+/g, ' ')
      .trim();
    const MonitorBrandIcon = ({ src, className = '' }) => src ? (
      <span className={`inline-flex items-center justify-center rounded-xl bg-white shadow-[0_6px_18px_rgba(15,23,42,0.12)] ring-1 ring-black/[0.05] dark:bg-white dark:ring-white/[0.08] ${className}`}>
        <img src={src} alt="" className="w-[72%] h-[72%] object-contain" />
      </span>
    ) : null;
    const MonitorCard = ({ children, className = '', highlight = false }) => (
      <div className={`group relative rounded-[36px] p-7 overflow-hidden transition-all duration-500 ease-[cubic-bezier(0.16,1,0.3,1)] ${highlight ? 'bg-white/88 dark:bg-[#1F1F23]' : 'bg-white/78 dark:bg-[#1C1C1E]'} backdrop-blur-[80px] backdrop-saturate-[200%] shadow-[0_16px_44px_rgba(15,23,42,0.10),0_2px_8px_rgba(15,23,42,0.05)] dark:shadow-[0_38px_96px_rgba(0,0,0,0.74),0_14px_34px_rgba(0,0,0,0.58),inset_0_1px_0_rgba(255,255,255,0.08)] hover:-translate-y-1 hover:scale-[1.006] hover:shadow-[0_24px_64px_rgba(15,23,42,0.14),0_4px_14px_rgba(15,23,42,0.07)] dark:hover:!bg-[#242428] dark:hover:shadow-[0_52px_128px_rgba(0,0,0,0.82),0_18px_42px_rgba(0,0,0,0.64),inset_0_1px_0_rgba(255,255,255,0.1)] ${className}`}>
        <div className="absolute inset-0 rounded-[36px] pointer-events-none border border-black/[0.06] dark:border-white/[0.055] transition-colors duration-500 group-hover:border-black/[0.08] dark:group-hover:border-white/[0.09]" />
        <div className="absolute inset-0 rounded-[36px] pointer-events-none ring-1 ring-inset ring-white/70 dark:ring-white/[0.035]" />
        <div className="absolute inset-x-6 top-0 h-px bg-white/0 dark:bg-white/[0.09] pointer-events-none" />
        {highlight && <div className="absolute -top-24 -right-24 w-48 h-48 bg-[#0A84FF]/10 rounded-full blur-[60px] pointer-events-none" />}
        <div className="relative z-10 flex flex-col h-full">{children}</div>
      </div>
    );
    const MonitorSectionHeader = ({ icon: Icon, title, value }) => (
      <div className="flex items-center justify-between mb-6">
        <div className="flex items-center gap-2.5 text-black/50 dark:text-white/50 transition-colors duration-500">
          <div className="p-1.5 bg-black/[0.04] dark:bg-white/[0.06] rounded-full"><Icon size={16} strokeWidth={2} /></div>
          <span className="text-[12px] font-bold tracking-[0.04em]">{title}</span>
        </div>
        {value && <span className="text-[14px] font-semibold tracking-tight text-black/80 dark:text-white/80">{value}</span>}
      </div>
    );
    const MonitorComputeHeader = ({ icon: Icon, title, device, brandIcon }) => (
      <div className="mb-6 space-y-3">
        <div className="flex items-center gap-2.5 text-black/50 dark:text-white/50">
          <div className="w-8 h-8 flex-none flex items-center justify-center rounded-full bg-black/[0.045] dark:bg-white/[0.07]">
            <Icon size={17} strokeWidth={2} />
          </div>
          <span className="text-[12px] font-bold tracking-[0.04em] whitespace-nowrap">{title}</span>
        </div>
        <div className="min-w-0 flex items-center gap-2.5 pl-10">
          {brandIcon && <MonitorBrandIcon src={brandIcon} className="w-10 h-10 flex-none rounded-[12px]" />}
          <span className="min-w-0 text-[18px] leading-[1.15] font-bold tracking-[-0.01em] text-black/82 dark:text-white/86 break-words">
            {device}
          </span>
        </div>
      </div>
    );
    const MonitorSegmentedBar = ({ label, used, total, percentage, color }) => {
      const segments = 24;
      const activeSegments = Math.round((monitorClampPct(percentage) / 100) * segments);
      return (
        <div>
          <div className="flex justify-between items-end mb-2.5">
            <span className="text-[12px] font-bold tracking-[0.04em] text-black/50 dark:text-white/50">{label}</span>
            <div className="flex items-baseline gap-1 font-mono text-[13px]">
              <span className="text-black dark:text-white font-semibold">{used}</span>
              <span className="text-black/40 dark:text-white/40 text-[11px]">/ {total}</span>
              <span className="text-black/25 dark:text-white/25 text-[11px] mx-0.5">·</span>
              <span className="text-black/55 dark:text-white/55 text-[11px] font-semibold">{monitorClampPct(percentage)}%</span>
            </div>
          </div>
          <div className="relative flex gap-[3px] h-3">
            <div className="absolute left-0 right-0 h-3 flex gap-[3px] -z-10 opacity-10 dark:opacity-20">{Array.from({ length: segments }).map((_, i) => <div key={`bg-${i}`} className="flex-1 bg-black dark:bg-white rounded-sm" />)}</div>
            {Array.from({ length: segments }).map((_, i) => <div key={i} className="flex-1 rounded-sm transition-colors duration-500" style={{ backgroundColor: i < activeSegments ? color : '', opacity: i < activeSegments ? 1 : 0, boxShadow: i < activeSegments ? `0 0 8px ${color}40` : 'none' }} />)}
          </div>
        </div>
      );
    };
    const MonitorRing = ({ label, percent, color }) => {
      const size = 118, strokeWidth = 11, radius = (size - strokeWidth) / 2;
      const circumference = radius * 2 * Math.PI;
      const offset = circumference - (monitorClampPct(percent) / 100) * circumference;
      return (
        <div className="flex flex-col items-center">
          <div className="relative flex items-center justify-center" style={{ width: size, height: size }}>
            <svg className="w-full h-full -rotate-90 overflow-visible">
              <circle cx={size / 2} cy={size / 2} r={radius} stroke="currentColor" strokeWidth={strokeWidth} fill="none" className="text-black/5 dark:text-white/5" />
              <circle cx={size / 2} cy={size / 2} r={radius} stroke={color} strokeWidth={strokeWidth} fill="none" strokeDasharray={circumference} strokeDashoffset={offset} strokeLinecap="round" className="transition-all duration-1000 ease-[cubic-bezier(0.16,1,0.3,1)]" style={{ filter: `drop-shadow(0 2px 7px ${color}55)` }} />
            </svg>
            <span className="absolute text-[24px] font-bold tracking-[-0.03em] text-black dark:text-white">{monitorClampPct(percent)}%</span>
          </div>
          <span className="mt-3 text-[10px] font-bold tracking-[0.04em] text-black/50 dark:text-white/50">{label}</span>
        </div>
      );
    };
    const MonitorSparkline = ({ color, data }) => {
      const hasData = !!(data && data.length);
      const rawNums = (hasData ? data : [46, 48, 47, 50, 49, 51]).map(n => Number(n) || 0);
      const nums = rawNums.length > 1 ? rawNums : [rawNums[0] || 0, rawNums[0] || 0];
      const max = Math.max(...nums), min = Math.min(...nums), range = max - min;
      const coords = nums.map((val, i) => ({
        x: (i / (nums.length - 1)) * 100,
        y: range > 0 ? 92 - ((val - min) / range) * 76 : 54
      }));
      const linePath = coords.reduce((path, point, i) => {
        if (i === 0) return `M ${point.x},${point.y}`;
        const prev = coords[i - 1], cpx = (prev.x + point.x) / 2;
        return `${path} C ${cpx},${prev.y} ${cpx},${point.y} ${point.x},${point.y}`;
      }, '');
      const areaPath = `${linePath} L 100,100 L 0,100 Z`;
      const last = coords[coords.length - 1];
      const id = String(color).replace('#', '');
      return (
        <div className="h-16 w-full mt-2 relative">
          <svg viewBox="0 0 100 100" preserveAspectRatio="none" className="w-full h-full overflow-visible">
            <defs>
              <linearGradient id={`mon-grad-${id}`} x1="0" x2="0" y1="0" y2="1"><stop offset="0%" stopColor={color} stopOpacity="0.28" /><stop offset="55%" stopColor={color} stopOpacity="0.08" /><stop offset="100%" stopColor={color} stopOpacity="0" /></linearGradient>
              <linearGradient id={`mon-stroke-${id}`} x1="0" x2="1" y1="0" y2="0"><stop offset="0%" stopColor={color} stopOpacity="0.45" /><stop offset="65%" stopColor={color} stopOpacity="1" /><stop offset="100%" stopColor={color} stopOpacity="1" /></linearGradient>
            </defs>
            {[24, 50, 76].map((y) => <line key={y} x1="0" x2="100" y1={y} y2={y} stroke="currentColor" strokeWidth="0.7" className="text-black/10 dark:text-white/10" vectorEffect="non-scaling-stroke" />)}
            {coords.slice(1, -1).map((point) => <line key={point.x} x1={point.x} x2={point.x} y1="16" y2="96" stroke="currentColor" strokeWidth="0.45" className="text-black/[0.06] dark:text-white/[0.07]" vectorEffect="non-scaling-stroke" />)}
            <path d={areaPath} fill={`url(#mon-grad-${id})`} opacity={hasData ? 1 : 0.38} />
            <path d={linePath} fill="none" stroke={color} strokeWidth="7" strokeLinecap="round" strokeLinejoin="round" opacity={hasData ? 0.16 : 0.08} className="blur-[1px]" vectorEffect="non-scaling-stroke" />
            <path d={linePath} fill="none" stroke={`url(#mon-stroke-${id})`} strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" opacity={hasData ? 1 : 0.42} vectorEffect="non-scaling-stroke" />
            <line x1={last.x} x2={last.x} y1="16" y2="96" stroke={color} strokeWidth="0.8" strokeOpacity={hasData ? 0.35 : 0.14} strokeDasharray="2 3" vectorEffect="non-scaling-stroke" />
            <circle cx={last.x} cy={last.y} r="4.2" fill="currentColor" className="text-white dark:text-[#1C1C1E]" />
            <circle cx={last.x} cy={last.y} r="3" fill={color} opacity={hasData ? 1 : 0.48} style={{ filter: `drop-shadow(0 0 8px ${color})` }} />
          </svg>
        </div>
      );
    };
    const MonitorMetricCard = ({ label, value, unit, hint, color, data }) => (
      <div className="bg-white/58 dark:bg-[#2C2C2E] rounded-3xl p-5 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_10px_28px_rgba(15,23,42,0.06)] dark:shadow-[0_20px_48px_rgba(0,0,0,0.48),0_7px_18px_rgba(0,0,0,0.36)] flex flex-col justify-between transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/85 hover:shadow-[0_14px_36px_rgba(15,23,42,0.09)] dark:hover:!bg-[#3A3A3C] dark:hover:shadow-[0_16px_38px_rgba(0,0,0,0.34)]">
        <div>
          <span className="text-[13px] font-bold tracking-[0.02em] text-black/58 dark:text-white/62">{label}</span>
          <div className="text-[32px] font-bold tracking-[-0.03em] mt-1">{value}{unit && String(value) !== '—' && <span className="text-[14px] text-black/40 dark:text-white/40 ml-1">{unit}</span>}</div>
          <div className="text-[12px] leading-snug font-medium text-black/45 dark:text-white/45 mt-1.5">{hint}</div>
        </div>
        <MonitorSparkline color={color} data={data} />
      </div>
    );
    const getMonitorHistoryStore = () => {
      if (!window.__pinvou3MonitorHistoryStore) {
        window.__pinvou3MonitorHistoryStore = { ctx: [], queue: [], ttft: [], tps: [], kv: [], activity: [], activityGen: null };
      }
      return window.__pinvou3MonitorHistoryStore;
    };
    const cloneMonitorHistory = () => {
      const store = getMonitorHistoryStore();
      return {
        ctx: store.ctx.slice(),
        queue: store.queue.slice(),
        ttft: store.ttft.slice(),
        tps: store.tps.slice(),
        kv: store.kv.slice(),
        activity: (store.activity || []).slice(),
        activityGen: typeof store.activityGen === 'number' ? store.activityGen : null,
      };
    };
    // 运行活动柱状图:每个轮询周期(1s)实际生成的 token 数,右起最新。
    // 无对话活动时全部为低位平条——不造假动画。
    const ACTIVITY_BAR_COUNT = 18;
    const MonitorActivityBars = ({ color, data }) => {
      const samples = Array.isArray(data) ? data.slice(-ACTIVITY_BAR_COUNT) : [];
      const pad = ACTIVITY_BAR_COUNT - samples.length;
      const max = Math.max(0, ...samples.map((v) => Number(v) || 0));
      return (
        <div className="flex items-center justify-between h-12 gap-1.5 opacity-90 px-2">
          {Array.from({ length: ACTIVITY_BAR_COUNT }).map((_, i) => {
            const v = i < pad ? null : Number(samples[i - pad]) || 0;
            const height = v == null || max <= 0 ? 8 : Math.max(12, Math.round((v / max) * 82));
            return (
              <div key={i} className="w-full bg-black/5 dark:bg-white/5 rounded-full overflow-hidden h-full flex flex-col justify-end">
                <div
                  className="w-full rounded-full transition-[height,opacity,box-shadow] duration-300 ease-out"
                  style={{
                    height: `${height}%`,
                    backgroundColor: color,
                    opacity: v == null ? 0.22 : 0.68 + (height / 100) * 0.32,
                    boxShadow: `0 0 ${Math.round(height / 7)}px ${color}55`
                  }}
                />
              </div>
            );
          })}
        </div>
      );
    };

    const MonitorView = ({ theme, t, bs }) => {
      const isDark = theme === 'dark';
      const fmt = bs && bs.monitor && bs.monitor._fmt;
      const monitorError = bs && bs.monitorError;
      const monitorBridgeReady = !!(window.TauriBridge?.monitor && typeof window.TauriBridge.monitor.startMonitorPolling === 'function');
      const loadingValue = !monitorBridgeReady ? t.uiMonitor.bridgeNotReady : (monitorError ? t.uiMonitor.readFailed : t.uiMonitor.reading);

      // Start/stop polling when view mounts/unmounts
      useEffect(() => {
        const liveBridge = window.TauriBridge || bridge;
        if (liveBridge?.monitor && typeof liveBridge.monitor.startMonitorPolling === 'function') {
          liveBridge.monitor.startMonitorPolling();
        } else {
          console.warn('[monitor] TauriBridge polling API unavailable');
        }
        return () => {
          if (liveBridge?.monitor && typeof liveBridge.monitor.stopMonitorPolling === 'function') liveBridge.monitor.stopMonitorPolling();
        };
      }, []);

      const updatedAt = fmt ? fmt.updatedAt : loadingValue;
      const gpuName = fmt ? fmt.gpuName : loadingValue;
      const gpuAvailable = fmt ? fmt.gpuAvailable : false;
      const gpuHasVram = fmt ? fmt.gpuHasVram : false;
      const gpuVram = fmt ? fmt.gpuVram : loadingValue;
      const gpuVramPct = fmt ? fmt.gpuVramPct : 0;
      const gpuUtil = fmt ? fmt.gpuUtil : loadingValue;
      const gpuUtilPct = fmt ? fmt.gpuUtilPct : 0;
      const gpuTemp = fmt ? fmt.gpuTemp : null;
      const gpuPower = fmt ? fmt.gpuPower : null;
      const ramUsedGiB = fmt ? fmt.ramUsedGiB : loadingValue;
      const ramPct = fmt ? fmt.ramPct : 0;
      const ramTotal = fmt ? fmt.ramTotal : loadingValue;
      const swapPct = fmt ? fmt.swapPct : 0;
      const swapTotal = fmt ? fmt.swapTotal : loadingValue;
      const vllmModel = fmt ? fmt.vllmModel : loadingValue;
      const vllmConfiguredModel = fmt ? fmt.vllmConfiguredModel : null;
      const vllmModelMismatch = fmt ? fmt.vllmModelMismatch : false;
      const vllmStatus = fmt ? fmt.vllmStatus : 'OFFLINE';
      const vllmHealthStatus = fmt ? fmt.vllmHealthStatus : 'offline';
      const vllmOnline = fmt ? fmt.vllmOnline : false;
      const vllmUpstream = fmt ? fmt.vllmUpstream : '—';
      const vllmTargetKind = fmt ? fmt.vllmTargetKind : t.uiMonitor.configError;
      const vllmIsRemote = fmt ? fmt.vllmIsRemote : false;
      const vllmDiagnostic = fmt ? fmt.vllmDiagnostic : null;
      const vllmMetricDiagnostic = fmt ? fmt.vllmMetricDiagnostic : null;
      const vllmMaxLen = fmt ? fmt.vllmMaxLen : loadingValue;
      const vllmCtxWarn = fmt ? fmt.vllmCtxWarn : null;
      const vllmQueue = fmt ? fmt.vllmQueue : loadingValue;
      const vllmKv = fmt ? fmt.vllmKv : loadingValue;
      const vllmTtft = fmt ? fmt.vllmTtft : loadingValue;
      const vllmTps = fmt ? fmt.vllmTps : loadingValue;
      const vllmTokTotal = fmt ? fmt.vllmTokTotal : loadingValue;
      const vllmStatsCleared = fmt ? fmt.vllmStatsCleared : false;
      const vllmClearedAt = fmt ? fmt.vllmClearedAt : null;
      const vllmRaw = fmt ? fmt.vllmRaw : null;
      const appVersion = fmt ? fmt.appVersion : loadingValue;
      const dtVersion = fmt ? fmt.dtVersion : '—';
      const uptime = fmt ? fmt.uptime : loadingValue;

      // 长按清除：先放数字归零插值动画（覆盖显示），动画跑完才真正设基准点清除，
      // 之后下一次 poll 接管显示「自此刻起」的区间值（KV/TTFT/TPS → 0，tokens → 0/0）。
      const [clearOverride, setClearOverride] = useState(null);
      const clearRafRef = useRef(0);
      const reduceMotionRef = useRef(window.matchMedia('(prefers-reduced-motion: reduce)').matches);
      useEffect(() => () => cancelAnimationFrame(clearRafRef.current), []);
      const fmtTokLocal = (n) => {
        if (n == null) return '—';
        if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
        if (n >= 1e3) return (n / 1e3).toFixed(1) + 'k';
        return String(Math.round(n));
      };
      const doClear = useCallback(() => {
        const reallyClear = () => { if (bridge.available && bridge.monitor.clearMonitorStats) bridge.monitor.clearMonitorStats(); };
        const raw = vllmRaw;
        if (!raw || reduceMotionRef.current) { reallyClear(); return; }
        const from = { kv: raw.kvPct, ttftS: raw.ttftS, tps: raw.tps, gen: raw.gen || 0, prompt: raw.prompt || 0 };
        const startT = performance.now(), dur = 480;
        cancelAnimationFrame(clearRafRef.current);
        const step = (now) => {
          const p = Math.min((now - startT) / dur, 1);
          const k = Math.pow(1 - p, 3);  // easeOutCubic 的剩余比例：1 → 0
          setClearOverride({
            kv: from.kv != null ? (from.kv * k).toFixed(1) + '%' : '0%',
            ttft: from.ttftS != null ? (from.ttftS * k).toFixed(2) + ' s' : '0 s',
            tps: from.tps != null ? (from.tps * k).toFixed(1) + ' tok/s' : '0 tok/s',
            tokTotal: fmtTokLocal(Math.round(from.gen * k)) + ' / ' + fmtTokLocal(Math.round(from.prompt * k)),
          });
          if (p >= 1) { reallyClear(); setClearOverride(null); return; }
          clearRafRef.current = requestAnimationFrame(step);
        };
        clearRafRef.current = requestAnimationFrame(step);
      }, [vllmRaw]);
      const pad2 = (n) => (n < 10 ? '0' + n : '' + n);
      let vllmStatusText = t.statsLifetime;
      if (vllmStatsCleared && vllmClearedAt) {
        const d = new Date(vllmClearedAt);
        const hhmm = pad2(d.getHours()) + ':' + pad2(d.getMinutes());
        const mins = Math.max(0, Math.round((Date.now() - vllmClearedAt) / 60000));
        vllmStatusText = t.statsSince.replace('%t', hhmm) + (mins === 0 ? t.statsJustReset : t.statsAge.replace('%m', mins));
      }

      const [history, setHistory] = useState(cloneMonitorHistory);
      const [clockNow, setClockNow] = useState(new Date());
      const [isModelClearing, setIsModelClearing] = useState(false);
      const modelClearTimerRef = useRef(null);
      useEffect(() => {
        const readNum = (value) => {
          if (typeof value === 'number') return value;
          const m = String(value || '').match(/-?\d+(\.\d+)?/);
          return m ? Number(m[0]) : null;
        };
        const genNow = fmt && fmt.vllmRaw && typeof fmt.vllmRaw.gen === 'number' ? fmt.vllmRaw.gen : null;
        setHistory(prev => {
          const push = (arr, value) => value == null ? arr : [...arr, value].slice(-20);
          // 运行活动:本轮询周期实际生成的 token 增量(counter 倒退 = 清除统计/后端重启,按 0)。
          // 清除动画期间(clearOverride)不采样,避免一帧一个 0 把历史冲掉。
          let activity = prev.activity;
          let activityGen = prev.activityGen;
          if (genNow != null && !clearOverride) {
            const delta = activityGen != null && genNow >= activityGen ? genNow - activityGen : 0;
            activity = [...activity, delta].slice(-ACTIVITY_BAR_COUNT);
            activityGen = genNow;
          }
          const next = {
            ctx: push(prev.ctx, readNum(vllmMaxLen)),
            queue: push(prev.queue, readNum(vllmQueue)),
            ttft: push(prev.ttft, readNum(clearOverride ? clearOverride.ttft : vllmTtft)),
            tps: push(prev.tps, readNum(clearOverride ? clearOverride.tps : vllmTps)),
            kv: push(prev.kv, (clearOverride || (fmt && fmt.vllmKvHasData)) ? readNum(clearOverride ? clearOverride.kv : vllmKv) : null),
            activity,
            activityGen,
          };
          Object.assign(getMonitorHistoryStore(), next);
          return next;
        });
      }, [vllmMaxLen, vllmQueue, vllmTtft, vllmTps, vllmKv, clearOverride, updatedAt]);
      useEffect(() => {
        const timer = setInterval(() => setClockNow(new Date()), 1000);
        return () => clearInterval(timer);
      }, []);
      const handleModelClearStart = () => {
        setIsModelClearing(true);
        if (modelClearTimerRef.current) clearTimeout(modelClearTimerRef.current);
        modelClearTimerRef.current = setTimeout(() => {
          doClear();
          setIsModelClearing(false);
          modelClearTimerRef.current = null;
        }, 1200);
      };
      const handleModelClearEnd = () => {
        setIsModelClearing(false);
        if (modelClearTimerRef.current) {
          clearTimeout(modelClearTimerRef.current);
          modelClearTimerRef.current = null;
        }
      };
      useEffect(() => () => {
        if (modelClearTimerRef.current) clearTimeout(modelClearTimerRef.current);
      }, []);

      // isDark dynamic-value: 保留 — 配色作为 color prop 喂给图表子组件(SVG/ring/bar),非 inline-style 三元式。
      const monitorColors = {
        blue: isDark ? '#0A84FF' : '#007AFF',
        green: isDark ? '#30D158' : '#34C759',
        orange: isDark ? '#FF9F0A' : '#FF9500',
        red: isDark ? '#FF453A' : '#FF3B30',
        gray: isDark ? '#98989D' : '#8E8E93',
      };
      const cpuAvailable = fmt ? !!fmt.cpuAvailable : false;
      const computeAvailable = fmt ? !!fmt.computeAvailable : gpuAvailable;
      const processorUtilPct = fmt && fmt.processorUtilPct != null ? fmt.processorUtilPct : 0;
      const gpuSharedMemory = fmt && fmt.gpuSharedMemory ? fmt.gpuSharedMemory : loadingValue;
      const gpuHasSharedMemory = !!(fmt && fmt.gpuSharedMemory && fmt.gpuSharedMemory !== '—');
      const isLocalProcessor = !gpuAvailable && cpuAvailable;
      const isUnifiedGpu = gpuAvailable && !gpuHasVram && !isLocalProcessor;
      const unifiedMemoryPct = isUnifiedGpu ? ramPct : gpuVramPct;
      const computeDeviceName = isLocalProcessor ? monitorShortProcessorName((fmt && (fmt.computeName || fmt.cpuName)) || gpuName) : ((fmt && fmt.computeName) || gpuName);
      const processorIcon = monitorProcessorIcon(computeDeviceName);
      const modelIcon = monitorModelIcon(vllmModel);
      const tokenPair = monitorTokenPair(clearOverride ? clearOverride.tokTotal : vllmTokTotal);
      const ctxNum = typeof vllmMaxLen === 'number' ? vllmMaxLen : parseFloat(String(vllmMaxLen || '').replace(/[^\d.]/g, ''));
      const ctxValue = Number.isFinite(ctxNum) ? Math.round(ctxNum / 1024) : String(vllmMaxLen || '—');
      const ctxUnit = Number.isFinite(ctxNum) ? 'K' : '';
      const queueText = String(vllmQueue || '—').replace(/\s+/g, '');
      const ttftText = String(clearOverride ? clearOverride.ttft : vllmTtft).replace(/\s*s$/i, '');
      const tpsText = String(clearOverride ? clearOverride.tps : vllmTps).replace(/\s*tok\/s$/i, '');
      const kvText = String(clearOverride ? clearOverride.kv : vllmKv).replace('%', '');
      const statusText = vllmOnline ? t.available
        : (vllmHealthStatus === 'missing_api_key' ? t.uiMonitor.unverified
          : (vllmHealthStatus === 'auth_failed' ? t.uiMonitor.authFailed
            : (vllmHealthStatus === 'unverified' ? t.uiMonitor.unverified : t.unavailable)));
      const runModeText = vllmIsRemote ? t.remoteService : t.localRunning;
      const swapUsed = fmt && fmt.swapUsed ? fmt.swapUsed : loadingValue;

      return (
        <div className="flex-1 w-full h-full overflow-y-auto custom-scrollbar">
          <div className="min-h-full bg-white dark:bg-[#131314] text-[#1F1F1F] dark:text-[#E3E3E3] p-4 sm:p-6 lg:p-10 font-sans selection:bg-blue-500/30 relative overflow-hidden transition-colors duration-500">
            <div className="max-w-[1400px] mx-auto relative z-10 space-y-6">
              <header
                className="flex flex-col md:flex-row justify-between items-start md:items-center gap-6 pb-6 mb-4 border-b border-[rgba(198,198,200,.55)] dark:border-[rgba(255,255,255,.10)]"
              >
                <div>
                  <h1 className="text-[26px] font-normal tracking-tight text-black/90 dark:text-white/90 max-sm:hidden">{t.sysStatus}</h1>
                  {(!monitorBridgeReady || monitorError) && (
                    <div className="mt-2 flex items-center gap-2.5 bg-black/[0.04] dark:bg-white/[0.06] px-3 py-1.5 rounded-full w-fit">
                      <span className="relative flex h-2 w-2">
                        <span className="absolute h-full w-full rounded-full bg-[#8E8E93] opacity-60" />
                        <span className="relative h-2 w-2 rounded-full bg-[#8E8E93]" />
                      </span>
                      <span className="text-[11px] font-bold tracking-[0.15em] uppercase text-black/50 dark:text-white/50">{!monitorBridgeReady ? t.uiMonitor.bridgeError : t.uiMonitor.readError(monitorError)}</span>
                    </div>
                  )}
                </div>

                <div className="flex items-center gap-1.5 bg-white/60 dark:bg-[#1C1C1E] backdrop-blur-[40px] rounded-full p-1.5 shadow-[0_4px_20px_rgb(0,0,0,0.04)] dark:shadow-[0_18px_46px_rgba(0,0,0,0.5)] ring-1 ring-black/[0.03] dark:ring-white/[0.055]">
                  <div className="flex items-center gap-2 px-4 text-[14px] font-semibold font-mono tracking-wider text-black/70 dark:text-white/70">
                    <Clock size={16} className="text-black/40 dark:text-white/40" /> {clockNow.toLocaleTimeString(MONITOR_CLOCK_LOCALE[t.langTag] || 'zh-CN', { hour12: false })}
                  </div>
                </div>
              </header>

              <div className="grid grid-cols-1 md:grid-cols-12 gap-6">
                <MonitorCard className="md:col-span-12 lg:col-span-4">
                  <MonitorSectionHeader icon={Database} title={t.ram} value={`${t.totalMemory} ${ramTotal}`} />
                  <div className="flex-1 flex flex-col justify-between mt-4">
                    <div className="space-y-8 pt-8">
                      <MonitorSegmentedBar label={t.runningMemory} used={ramUsedGiB} total={ramTotal} percentage={ramPct} color={monitorColors.blue} />
                      <MonitorSegmentedBar label={t.temporaryMemory} used={swapUsed} total={swapTotal} percentage={swapPct} color={monitorColors.green} />
                    </div>
                    <div className="mt-8 rounded-2xl bg-white/55 dark:bg-[#2C2C2E] border border-black/[0.045] dark:border-white/[0.055] px-4 py-3 flex justify-between items-center text-[12px] font-semibold shadow-[0_8px_22px_rgba(15,23,42,0.05)] dark:shadow-[0_16px_34px_rgba(0,0,0,0.34)]">
                      <span className="text-black/45 dark:text-white/45">{t.memoryPressure}</span>
                      <span className="inline-flex items-center gap-1.5 text-black dark:text-white"><span className="w-1.5 h-1.5 rounded-full bg-[#34C759] dark:bg-[#30D158]" />{t.normal}</span>
                    </div>
                  </div>
                </MonitorCard>

                <MonitorCard className="md:col-span-6 lg:col-span-4">
                  <MonitorComputeHeader
                    icon={Cpu}
                    title={isLocalProcessor ? t.localProcessor : t.gpu}
                    device={computeAvailable ? computeDeviceName : t.gpuUnavail}
                    brandIcon={processorIcon}
                  />
                  <div className="flex-1 flex flex-col justify-end mt-4">
                    <div className="flex items-center justify-center gap-10 mb-7">
                      <MonitorRing label={isLocalProcessor ? t.processorLoad : t.core} percent={isLocalProcessor ? processorUtilPct : gpuUtilPct} color={monitorColors.blue} />
                      <MonitorRing label={isLocalProcessor ? t.graphicsLoad : (isUnifiedGpu ? t.unifiedMem : t.vram)} percent={isLocalProcessor ? gpuUtilPct : unifiedMemoryPct} color={monitorColors.orange} />
                    </div>
                    <div className="pt-4 border-t border-black/5 dark:border-white/5 space-y-3 text-[12px] font-semibold">
                      {(isLocalProcessor || gpuHasSharedMemory) && (
                        <div className="flex justify-between items-center text-black/40 dark:text-white/40">
                          <span>{t.sharedMemory}</span>
                          <span className="text-black dark:text-white">{gpuSharedMemory}</span>
                        </div>
                      )}
                      {(isLocalProcessor || gpuTemp) && (
                        <div className="flex justify-between items-center text-black/40 dark:text-white/40">
                          <span>{isLocalProcessor ? t.deviceTemp : t.temp}</span>
                          <span className="text-black dark:text-white">{gpuTemp || '—'}</span>
                        </div>
                      )}
                      {!isLocalProcessor && gpuPower && (
                        <div className="flex justify-between items-center text-black/40 dark:text-white/40">
                          <span>{t.power}</span>
                          <span className="text-black dark:text-white">{gpuPower}</span>
                        </div>
                      )}
                    </div>
                  </div>
                </MonitorCard>

                <MonitorCard className="md:col-span-12 lg:col-span-4 flex flex-col">
                  <MonitorSectionHeader icon={Server} title={t.app} />
                  <div className="flex-1 flex flex-col justify-between">
                    <div className="mb-6">
                      <div className="flex items-center gap-2 mb-2">
                        <PinvouLogo className="h-7 w-7 rounded-lg shadow-[0_2px_8px_rgba(0,0,0,0.12)]" />
                        <h2 className="text-xl font-bold">pinvou3-app</h2>
                      </div>
                    </div>
                    <div className="space-y-3 mb-6">
                      <div className="flex items-center justify-between rounded-2xl bg-white/65 dark:bg-[#2C2C2E] px-4 py-3 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_8px_22px_rgba(15,23,42,0.06)] dark:shadow-[0_18px_44px_rgba(0,0,0,0.46),0_6px_16px_rgba(0,0,0,0.34)] transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/90 hover:shadow-[0_12px_30px_rgba(15,23,42,0.09)] dark:hover:!bg-[#3A3A3C] dark:hover:shadow-[0_14px_34px_rgba(0,0,0,0.34)]">
                        <span className="text-[11px] font-bold tracking-[0.04em] text-black/45 dark:text-white/45">{t.curVer}</span>
                        <span className="text-[13px] font-bold font-mono text-black/70 dark:text-white/70">{appVersion}</span>
                      </div>
                      <div className="flex items-center justify-between rounded-2xl bg-white/65 dark:bg-[#2C2C2E] px-4 py-3 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_8px_22px_rgba(15,23,42,0.06)] dark:shadow-[0_18px_44px_rgba(0,0,0,0.46),0_6px_16px_rgba(0,0,0,0.34)] transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/90 hover:shadow-[0_12px_30px_rgba(15,23,42,0.09)] dark:hover:!bg-[#3A3A3C] dark:hover:shadow-[0_14px_34px_rgba(0,0,0,0.34)]">
                        <span className="text-[11px] font-bold tracking-[0.04em] text-black/45 dark:text-white/45">{t.uptime}</span>
                        <span className="text-[13px] font-bold font-mono text-black/70 dark:text-white/70">{uptime}</span>
                      </div>
                    </div>
                    <div className="mt-auto bg-white/55 dark:bg-[#2C2C2E] rounded-3xl p-4 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_10px_28px_rgba(15,23,42,0.06)] dark:shadow-[0_20px_48px_rgba(0,0,0,0.48),0_7px_18px_rgba(0,0,0,0.36)] transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/80 dark:hover:!bg-[#3A3A3C]">
                      <span className="text-[10px] font-bold tracking-[0.04em] text-black/50 dark:text-white/50 block mb-3 px-2">{t.uiMonitor.activity}</span>
                      <MonitorActivityBars color={monitorColors.blue} data={history.activity} />
                    </div>
                  </div>
                </MonitorCard>

                <MonitorCard className="md:col-span-12" highlight>
                  <MonitorSectionHeader icon={Brain} title={t.currentModel} />
                  <div className="flex justify-between items-start mb-6">
                    <div>
                      <div className="flex items-center gap-3 mb-2">
                        {modelIcon && <MonitorBrandIcon src={modelIcon} className="w-9 h-9" />}
                        <h2 className="text-2xl font-bold tracking-tight">{vllmModel}</h2>
                      </div>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="px-3.5 py-2 rounded-full bg-black/[0.04] dark:bg-[#2C2C2E] text-[12px] font-bold tracking-[0.04em] text-black/55 dark:text-white/60 border border-black/[0.04] dark:border-white/[0.055]">{runModeText}</span>
                      <button className="bg-black/5 dark:bg-[#2C2C2E] hover:bg-black/10 dark:hover:bg-[#3A3A3C] px-4 py-2 rounded-full text-[12px] font-bold tracking-[0.04em] transition-colors">{statusText}</button>
                      <button
                        onMouseDown={handleModelClearStart}
                        onMouseUp={handleModelClearEnd}
                        onMouseLeave={handleModelClearEnd}
                        onTouchStart={handleModelClearStart}
                        onTouchEnd={handleModelClearEnd}
                        className="relative group flex items-center gap-2 px-4 py-2 rounded-full bg-black/5 dark:bg-white/10 overflow-hidden active:scale-95 transition-transform"
                      >
                        <div
                          className="absolute inset-0 bg-[#FF3B30]/20 z-0"
                          style={{ width: isModelClearing ? '100%' : '0%', transition: isModelClearing ? 'width 1.2s ease' : 'width 0.3s' }}
                        />
                        <RotateCcw size={14} className={`relative z-10 ${isModelClearing ? 'text-[#FF3B30] animate-spin' : 'text-black/60 dark:text-white/70'}`} />
                        <span className={`relative z-10 text-[12px] font-bold tracking-[0.04em] ${isModelClearing ? 'text-[#FF3B30]' : 'text-black/70 dark:text-white/80'}`}>{t.resetCount}</span>
                      </button>
                    </div>
                  </div>

                  <div className="grid grid-cols-2 xl:grid-cols-5 gap-6 mb-6">
                    <MonitorMetricCard label={t.ctx} value={ctxValue} unit={ctxUnit} hint={t.contextHint} color={monitorColors.orange} data={history.ctx} />
                    <MonitorMetricCard label={t.queue} value={queueText} unit="" hint={t.queueHint} color={monitorColors.green} data={history.queue} />
                    <MonitorMetricCard label={t.ttft} value={ttftText} unit="s" hint={t.ttftHint} color={monitorColors.orange} data={history.ttft} />
                    <MonitorMetricCard label={t.tps} value={tpsText} unit="tok/s" hint={t.tpsHint} color={monitorColors.blue} data={history.tps} />
                    <MonitorMetricCard label={t.historyReuse} value={kvText} unit="%" hint={t.reuseHint} color={monitorColors.green} data={history.kv} />
                  </div>

                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                      <div className="rounded-3xl bg-white/62 dark:bg-[#2C2C2E] px-5 py-5 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_8px_24px_rgba(15,23,42,0.06)] dark:shadow-[0_18px_44px_rgba(0,0,0,0.46),0_6px_16px_rgba(0,0,0,0.34)] transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/90 hover:shadow-[0_12px_32px_rgba(15,23,42,0.09)] dark:hover:!bg-[#3A3A3C] dark:hover:shadow-[0_14px_34px_rgba(0,0,0,0.34)]">
                        <div className="flex items-start justify-between gap-4">
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="w-2 h-2 rounded-full bg-[#007AFF] dark:bg-[#0A84FF]" />
                              <div className="text-[14px] font-bold tracking-[0.02em] text-black/62 dark:text-white/68">{t.modelReadAmount}</div>
                            </div>
                            <div className="text-[12px] leading-snug font-semibold text-black/45 dark:text-white/45 mt-2">{t.modelReadHint}</div>
                          </div>
                          <div className="text-[30px] leading-none font-bold tracking-[-0.03em]">{tokenPair.input}</div>
                        </div>
                      </div>
                      <div className="rounded-3xl bg-white/62 dark:bg-[#2C2C2E] px-5 py-5 border border-black/[0.055] dark:border-white/[0.055] shadow-[0_8px_24px_rgba(15,23,42,0.06)] dark:shadow-[0_18px_44px_rgba(0,0,0,0.46),0_6px_16px_rgba(0,0,0,0.34)] transition-all duration-300 hover:-translate-y-0.5 hover:bg-white/90 hover:shadow-[0_12px_32px_rgba(15,23,42,0.09)] dark:hover:!bg-[#3A3A3C] dark:hover:shadow-[0_14px_34px_rgba(0,0,0,0.34)]">
                        <div className="flex items-start justify-between gap-4">
                          <div>
                            <div className="flex items-center gap-2">
                              <span className="w-2 h-2 rounded-full bg-[#34C759] dark:bg-[#30D158]" />
                              <div className="text-[14px] font-bold tracking-[0.02em] text-black/62 dark:text-white/68">{t.modelOutputAmount}</div>
                            </div>
                            <div className="text-[12px] leading-snug font-semibold text-black/45 dark:text-white/45 mt-2">{t.modelOutputHint}</div>
                          </div>
                          <div className="text-[30px] leading-none font-bold tracking-[-0.03em]">{tokenPair.output}</div>
                        </div>
                      </div>
                  </div>
                </MonitorCard>
              </div>
            </div>
          </div>
        </div>
      );

    };

    // ==========================================
    // Settings View (Material 3 Style)
    // ==========================================
    // 统一排版原语：卡片 / 行(label 左 + 控件右) / 纵向输入字段 / 分段选择 / 改动操作条

export { ClearStatsHold, MONITOR_BRAND_ICONS, monitorModelIcon, monitorProcessorIcon, monitorClampPct, monitorShortNum, monitorTokenPair, monitorShortProcessorName, MonitorBrandIcon, MonitorCard, MonitorSectionHeader, MonitorComputeHeader, MonitorSegmentedBar, MonitorRing, MonitorSparkline, MonitorMetricCard, getMonitorHistoryStore, cloneMonitorHistory, MonitorActivityBars, MonitorView };
