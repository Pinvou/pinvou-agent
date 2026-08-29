import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { BookOpen, Check, ChevronLeft, Download, Globe, Package, Search, Server, Settings, Upload, XIcon, Zap } from '../../components/icons.jsx';
import { resolveOAuthInstallOutcome } from './oauth-marketplace-logic.js';
import { notifyComposerToolsChanged } from './tool-events.js';
import { localizeTool, mergeConfigFields, TsActionBtn, tsCategories, tsSkillIconByName, tsSkillsData, tsToolsData, tsToolWelcomeData, TOOL_TYPE_GROUPS, getToolTypeGroup, TOOL_BUSINESS_GROUPS, getToolBusinessGroup } from './tool-common.jsx';
import { MAX_SKILL_ZIP_BYTES, pickSkillDrop, fileToBase64 } from './skill-import-logic.js';
import { invokeTauri, isTauriAvailable, tauriEvents } from '../../platform/tauri/client.js';
import { can } from '../../shared/platform.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';

const OAUTH_UI_TIMEOUT_MS = 90_000;

const canStartExternalAuth = () => can('oauth') && can('externalAuth');

const isRestrictedExternalAuthTool = (tool) => !!tool && !!(
  tool.authRequired
  || tool.oauthMcp
  || tool.feishuCli
  || tool.wecomCli
  || tool.dingtalkCli
  || tool.tmeetCli
  || tool.imaOpenapi
);

const PlatformToolAction = ({ copy, t, ...props }) => {
  if (!can('toolStoreMutations')) {
    if (!props.tool?.installed) return null;
    const label = isRestrictedExternalAuthTool(props.tool) ? copy.connected : copy.installed;
    return (
      <span className={`${props.size === 'lg' ? 'px-6 py-2.5 text-[15px]' : 'px-4 py-1.5 text-[13px]'} rounded-full font-bold bg-emerald-50 dark:bg-emerald-500/10 text-emerald-700 dark:text-emerald-300 whitespace-nowrap`}>
        {label}
      </span>
    );
  }
  if (!canStartExternalAuth() && isRestrictedExternalAuthTool(props.tool)) {
    if (!props.tool.installed) return null;
    return (
      <span className={`${props.size === 'lg' ? 'px-6 py-2.5 text-[15px]' : 'px-4 py-1.5 text-[13px]'} rounded-full font-bold bg-emerald-50 dark:bg-emerald-500/10 text-emerald-700 dark:text-emerald-300 whitespace-nowrap`}>
        {copy.connected}
      </span>
    );
  }
  return <TsActionBtn {...props} t={t} />;
};

const THIRD_PARTY_TOOL_LOGOS = {
  weather: 'assets/tool-icons/amap-user-v3.png',
  iwencai: 'assets/tool-icons/iwencai-user-v3.png',
  feishu: 'assets/tool-icons/wb-feishu.svg',
  wecom: 'assets/tool-icons/wecom-user.png',
  'wecom-bot': 'assets/tool-icons/wecom-user.png',
  dingtalk: 'assets/tool-icons/dingtalk-user-v2.png',
  tmeet: 'assets/tool-icons/wb-tencent-meeting.png',
  qcc: 'assets/tool-icons/qcc-user.png',
  'patsnap-search': 'assets/tool-icons/wb-patsnap-search.png',
  'tencent-docs': 'assets/tool-icons/wb-tencent-docs.png',
  ima: 'assets/tool-icons/wb-ima-mcp.png',
  obsidian: 'assets/tool-icons/obsidian.ico',
  'yuandian-mcp': 'assets/tool-icons/wb-yuandian-mcp.svg',
  3: 'assets/tool-icons/wb-qq-mail.png',
  4: 'assets/tool-icons/wb-ima-mcp.png',
  5: 'assets/tool-icons/wb-lexiang.png',
  6: 'assets/tool-icons/wb-tencent-docs.png',
  8: 'assets/tool-icons/wecom-user.png',
  11: 'assets/tool-icons/wb-tapd.png',
  12: 'assets/tool-icons/wb-cnb-api.svg',
};

const FULL_TILE_LOGOS = new Set(['assets/tool-icons/amap-user-v3.png', 'assets/tool-icons/dingtalk-user-v2.png', 'assets/tool-icons/iwencai-user-v3.png', 'assets/tool-icons/qcc-user.png', 'assets/tool-icons/wb-ima-mcp.png', 'assets/tool-icons/wb-tencent-meeting.png', 'assets/tool-icons/wb-yuandian-mcp.svg', 'assets/tool-icons/wecom-user.png']);
const CROPPED_TILE_LOGOS = new Set(['assets/tool-icons/wb-yuandian-mcp.svg']);

const TsToolIcon = ({ tool, className = '', imageClassName = 'h-8 w-8', fallbackSize = 30, fallbackStrokeWidth = 1.5, children }) => {
  const Icon = tool.icon;
  const isFullTileLogo = tool.logoSrc && FULL_TILE_LOGOS.has(tool.logoSrc);
  const cropTileLogo = tool.logoSrc && CROPPED_TILE_LOGOS.has(tool.logoSrc);
  const logoBg = tool.logoSrc ? (isFullTileLogo ? 'bg-transparent' : 'bg-white dark:bg-white') : '';
  const logoFg = tool.logoSrc ? 'text-slate-900' : `${tool.color} text-white`;
  const logoBox = tool.logoSrc ? `${logoBg} ${logoFg}` : `${tool.color} text-white`;
  return (
    <div className={`relative flex items-center justify-center overflow-hidden ${logoBox} ${className}`}>
      {tool.logoSrc ? (
        <img
          src={tool.logoSrc}
          alt=""
          className={isFullTileLogo ? `h-full w-full rounded-[inherit] object-cover ${cropTileLogo ? 'scale-[1.22]' : ''}` : `object-contain ${imageClassName}`}
          loading="lazy"
        />
      ) : (
        <Icon size={fallbackSize} strokeWidth={fallbackStrokeWidth} />
      )}
      {children}
    </div>
  );
};

const oauthUiTimeoutResult = (serverName) => ({
  status: 'timeout',
  message: '',
  server_name: serverName,
});

const oauthServerNameForTool = (tool) => tool?.oauthServerName || tool?.serverName || null;

const withUiTimeout = (promise, timeoutMs, fallbackResult) => {
  let timeoutId = null;
  const timeoutPromise = new Promise(resolve => {
    timeoutId = setTimeout(() => resolve(fallbackResult), timeoutMs);
  });
  return Promise.race([promise, timeoutPromise]).finally(() => {
    if (timeoutId) clearTimeout(timeoutId);
  });
};

    const FeishuStepIcon = ({ st }) => {
      if (st === 'done') return <span className="w-5 h-5 rounded-full bg-emerald-500 grid place-items-center text-white text-[11px]">✓</span>;
      if (st === 'active') return <span className="w-5 h-5 rounded-full bg-blue-600 grid place-items-center text-white text-[10px] animate-pulse">●</span>;
      if (st === 'error') return <span className="w-5 h-5 rounded-full bg-rose-500 grid place-items-center text-white text-[11px]">✕</span>;
      return <span className="w-5 h-5 rounded-full border-2 border-slate-300 dark:border-white/20 inline-block" />;
    };
    const FeishuBar = ({ pct, creep }) => (
      <div className="mt-1.5 h-1.5 w-full rounded-full bg-slate-200 dark:bg-white/10 overflow-hidden">
        <div className={`h-full rounded-full transition-all ${creep ? 'bg-blue-500' : 'bg-emerald-500'}`} style={{ width: (pct || 0) + '%' }} />
      </div>
    );
    // 稳定空数组/空对象默认值：内联 [] {} 每次渲染都是新引用，会让 memo 化子组件反复重渲染。
    const EMPTY_STEPS = [];
    const EMPTY_COPY = {};
    const FeishuFlowCard = ({ flow, onRetry, onCancel, name = '', twoStep = true, browserAuth = false, steps = EMPTY_STEPS, copy = EMPTY_COPY }) => {
      if (!flow) return null;
      const isErr = flow.phase === 'error';
      return (
        <div className="mb-8 rounded-2xl border border-slate-200 dark:border-white/10 bg-slate-50 dark:bg-white/5 overflow-hidden">
          <div className="flex items-center gap-3 px-5 pt-4 pb-2">
            <span className={`w-2 h-2 rounded-full ${isErr ? 'bg-rose-500' : 'bg-blue-500 animate-pulse'}`} />
            <span className="font-semibold text-[14px] text-slate-900 dark:text-slate-100">{isErr ? copy.incomplete(name) : (flow.phase === 'done' ? copy.connected(name) : copy.connecting(name))}</span>
            <span className="flex-1" />
            {(flow.phase === 'running' || flow.phase === 'qr') && <button type="button" onClick={onCancel} className="text-[12px] text-slate-400 hover:text-slate-600 dark:hover:text-slate-200">{copy.cancel}</button>}
          </div>
          <div className="px-5 pb-4 space-y-1">
            {steps.map(s => {
              const st = (flow.steps && flow.steps[s.key]) || 'wait';
              const active = st === 'active';
              return (
                <div key={s.key} className={`flex gap-3 py-1.5 ${st === 'wait' ? 'opacity-45' : ''}`}>
                  <div className="pt-0.5"><FeishuStepIcon st={st} /></div>
                  <div className="flex-1 min-w-0">
                    <div className={`text-[13.5px] font-medium ${st === 'done' ? 'text-slate-400 line-through decoration-slate-300' : 'text-slate-900 dark:text-slate-100'}`}>{s.label}</div>
                    {active && s.key === 'runtime' && (<><FeishuBar pct={flow.pct} /><div className="text-[11px] text-slate-400 mt-1">{copy.extracting(Math.round(flow.pct || 0))}</div></>)}
                    {active && s.key === 'cli' && (<><FeishuBar pct={flow.pct} creep /><div className="flex items-center justify-between mt-1"><div className="text-[11px] text-slate-400 truncate max-w-[260px] font-mono">{flow.log || copy.installStarting}</div><div className="text-[11px] text-slate-400 tabular-nums">{copy.elapsed(flow.sec || 0)}</div></div></>)}
                    {!active && <div className="text-[11.5px] text-slate-400">{s.sub}</div>}
                  </div>
                </div>
              );
            })}
          </div>
          {flow.phase === 'qr' && browserAuth && (
            <div className="px-5 pb-5">
              <div className="flex items-center gap-3 rounded-xl bg-white dark:bg-black/30 border border-slate-200 dark:border-white/10 px-4 py-3">
                <span className="w-2.5 h-2.5 rounded-full bg-blue-500 animate-pulse shrink-0" />
                <div className="min-w-0 flex-1">
                  <div className="font-medium text-[14px] text-slate-900 dark:text-slate-100">{copy.browserOpened}</div>
                  <div className="text-[12px] text-slate-500 dark:text-slate-400 mt-0.5">{copy.browserHint}</div>
                </div>
                {flow.qrUrl && (
                  <button type="button"
                    onClick={() => invokeTauri('open_external_url', { url: flow.qrUrl })}
                    className="shrink-0 text-[13px] text-blue-600 dark:text-blue-400 hover:underline"
                  >
                    {copy.reopen}
                  </button>
                )}
              </div>
            </div>
          )}
          {flow.phase === 'qr' && !browserAuth && flow.qr && (
            <div className="px-5 pb-5">
              <div className="flex items-center gap-5 p-4 rounded-xl bg-white dark:bg-black/30 border border-slate-200 dark:border-white/10">
                <img src={flow.qr} alt={copy.qrAlt(name)} loading="lazy" decoding="async" className="w-36 h-36 rounded-xl border border-slate-200 bg-white shrink-0" />
                <div>
                  <div className="font-medium text-[14px] mb-1 text-slate-900 dark:text-slate-100">{twoStep ? (flow.qrPhase === 'authorize' ? copy.authorizeStep : copy.registerStep) : copy.scanLogin(name)}</div>
                  <div className="text-[12px] text-slate-500 dark:text-slate-400 mb-3">{copy.scanHint(name)}</div>
                  {flow.userCode && (
                    <div className="mb-3 inline-flex flex-col gap-1 rounded-lg bg-slate-100 dark:bg-white/10 px-3 py-2">
                      <span className="text-[11px] text-slate-500 dark:text-slate-400">{copy.userCode}</span>
                      <span className="font-mono text-[18px] font-bold tracking-wider text-slate-900 dark:text-white">{flow.userCode}</span>
                    </div>
                  )}
                  {flow.qrUrl && <button type="button" onClick={() => invokeTauri('open_external_url', { url: flow.qrUrl })} className="text-[13px] text-blue-600 dark:text-blue-400 hover:underline">{copy.openBrowser}</button>}
                </div>
              </div>
            </div>
          )}
          {isErr && (
            <div className="px-5 pb-5">
              <div className="rounded-xl border border-rose-200 dark:border-rose-500/30 bg-rose-50 dark:bg-rose-500/10 p-3">
                <div className="text-[13px] font-medium text-rose-700 dark:text-rose-300 mb-1.5">{copy.connectionIncomplete}</div>
                <pre className="text-[11.5px] leading-relaxed text-rose-800/80 dark:text-rose-200/70 whitespace-pre-wrap max-h-28 overflow-auto font-mono">{flow.err}</pre>
                <div className="flex gap-2 mt-3 justify-end">
                  <button type="button" onClick={onCancel} className="px-3 py-1.5 rounded-lg bg-slate-200 dark:bg-white/10 text-slate-700 dark:text-slate-100 text-[13px]">{copy.close}</button>
                  <button type="button" onClick={onRetry} className="px-3 py-1.5 rounded-lg bg-blue-600 text-white text-[13px]">{copy.retry}</button>
                </div>
              </div>
            </div>
          )}
        </div>
      );
    };
    // 商店列表行内的迷你进度（详情弹窗关掉后，后台仍在跑）
    const FeishuMini = ({ flow, onClick, copy }) => {
      const label = flow.phase === 'qr' ? copy.scan
        : (flow.active === 'cli' ? copy.install(Math.round(flow.pct || 0))
        : (flow.active === 'runtime' ? copy.extract(Math.round(flow.pct || 0)) : copy.connecting));
      return (
        <button type="button" onClick={(e) => { e.stopPropagation(); onClick(); }} title={copy.title} className="shrink-0 flex items-center gap-1.5 pl-1.5 pr-2.5 py-1.5 rounded-full bg-blue-50 dark:bg-blue-500/10 border border-blue-200 dark:border-blue-500/30 text-blue-600 dark:text-blue-300 text-[12px] font-medium">
          <span className="w-3 h-3 rounded-full border-2 border-blue-500 border-t-transparent animate-spin inline-block shrink-0" />
          <span className="tabular-nums whitespace-nowrap">{label}</span>
        </button>
      );
    };

    // ── 飞书连接流程 · 跨视图持久 store ──
    // ToolStoreView 随左栏切换会卸载；连接是长流程（装 CLI ~40s + 扫码），进度/监听/秒表
    // 若放组件 useState，一离开工具商店就全丢 → 回来按钮又变“连接”。故挂在模块级单例，
    // 活在组件生命周期之外；组件只订阅它做镜像渲染。
    /* eslint-disable unicorn/no-this-outside-of-class -- module-level connection store singleton; object-literal methods reference itself via this, and converting to a class would just move the same complexity */
    const feishuConn = {
      flow: null,
      tick: null,
      listenersReady: false,
      subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch { /* silent: one failing subscriber must not affect the rest of the broadcast */ } }); },
      startTick() {
        this.stopTick();
        this.tick = setInterval(() => this.setFlow(f => {
          if (!f || f.phase !== 'running') return f;
          const nf = { ...f, sec: (f.sec || 0) + 1 };
          if (f.active === 'cli') nf.pct = Math.min(90, (f.pct || 0) + (90 - (f.pct || 0)) * 0.06 + 1);
          return nf;
        }), 1000);
      },
      stopTick() { if (this.tick) { clearInterval(this.tick); this.tick = null; } },
    };
    /* eslint-enable unicorn/no-this-outside-of-class -- module-level connection store singleton */
    // 后端连接事件只注册一次（幂等，跨 ToolStoreView 多次挂载不重复注册）。
    function ensureFeishuListeners(copy = {}) {
      if (feishuConn.listenersReady) return;
      const connFailed = copy.connFailed;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      feishuConn.listenersReady = true;
      ev.listen('feishu:progress', (e) => {
        const p = e.payload || {};
        feishuConn.setFlow(f => {
          const nf = f ? { ...f, steps: { ...f.steps } } : { phase: 'running', steps: {}, active: null, pct: 0, sec: 0, log: '' };
          if (p.step) { nf.active = p.step; nf.steps[p.step] = p.status === 'done' ? 'done' : 'active'; }
          if (typeof p.pct === 'number') nf.pct = p.pct;
          if (p.log) nf.log = p.log;
          if (nf.phase !== 'error') nf.phase = 'running';
          return nf;
        });
      });
      ev.listen('feishu:qr', (e) => {
        const p = e.payload || {};
        feishuConn.stopTick();
        feishuConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return {
            ...f, phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), connect: 'done', qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase,
          };
        });
      });
      ev.listen('feishu:connected', () => {
        feishuConn.stopTick();
        feishuConn.setFlow(f => ({ ...f, phase: 'done', steps: { ...(f && f.steps), qr: 'done' } }));
        // 连上 → 按规则写技能（默认启用）+ 广播刷新；跟视图无关，放全局做。
        invokeTauri('feishu_apply_skills').catch(() => {});
        // 稍后自动收起流程卡（详情里的“已连接”态改由 feishuConnected 驱动）
        setTimeout(() => feishuConn.setFlow(null), 1800);
      });
      ev.listen('feishu:error', (e) => {
        const p = e.payload || {};
        feishuConn.stopTick();
        feishuConn.setFlow(f => {
          const step = (f && f.active) || 'cli';
          return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || connFailed), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } };
        });
      });
    }

    // ── 企业微信连接流程 · 跨视图持久 store(镜像 feishuConn;企微纯扫码单段）──
    /* eslint-disable unicorn/no-this-outside-of-class -- same as feishuConn: module-level singleton whose methods reference itself via this */
    const wecomConn = {
      flow: null, tick: null, listenersReady: false, subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch { /* silent: one failing subscriber must not affect the rest of the broadcast */ } }); },
      startTick() {
        this.stopTick();
        this.tick = setInterval(() => this.setFlow(f => {
          if (!f || f.phase !== 'running') return f;
          const nf = { ...f, sec: (f.sec || 0) + 1 };
          if (f.active === 'cli') nf.pct = Math.min(90, (f.pct || 0) + (90 - (f.pct || 0)) * 0.06 + 1);
          return nf;
        }), 1000);
      },
      stopTick() { if (this.tick) { clearInterval(this.tick); this.tick = null; } },
    };
    /* eslint-enable unicorn/no-this-outside-of-class -- same as feishuConn */
    function ensureWecomListeners(copy = {}) {
      if (wecomConn.listenersReady) return;
      const connFailed = copy.connFailed;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      wecomConn.listenersReady = true;
      ev.listen('wecom:qr', (e) => {
        const p = e.payload || {};
        wecomConn.stopTick();
        wecomConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return { ...f, phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase };
        });
      });
      ev.listen('wecom:connected', () => {
        wecomConn.stopTick();
        wecomConn.setFlow(f => ({ ...f, phase: 'done', steps: { ...(f && f.steps), qr: 'done' } }));
        invokeTauri('wecom_apply_skills').catch(() => {});
        setTimeout(() => wecomConn.setFlow(null), 1800);
      });
      ev.listen('wecom:error', (e) => {
        const p = e.payload || {};
        wecomConn.stopTick();
        wecomConn.setFlow(f => { const step = (f && f.active) || 'cli'; return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || connFailed), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } }; });
      });
    }

    // ── 钉钉连接流程 · 跨视图持久 store(镜像企微;纯扫码单段）──
    /* eslint-disable unicorn/no-this-outside-of-class -- same as feishuConn: module-level singleton whose methods reference itself via this */
    const dingtalkConn = {
      flow: null, tick: null, listenersReady: false, subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch { /* silent: one failing subscriber must not affect the rest of the broadcast */ } }); },
      startTick() {
        this.stopTick();
        this.tick = setInterval(() => this.setFlow(f => {
          if (!f || f.phase !== 'running') return f;
          const nf = { ...f, sec: (f.sec || 0) + 1 };
          if (f.active === 'cli') nf.pct = Math.min(90, (f.pct || 0) + (90 - (f.pct || 0)) * 0.06 + 1);
          return nf;
        }), 1000);
      },
      stopTick() { if (this.tick) { clearInterval(this.tick); this.tick = null; } },
    };
    /* eslint-enable unicorn/no-this-outside-of-class -- same as feishuConn */
    function ensureDingtalkListeners(copy = {}) {
      if (dingtalkConn.listenersReady) return;
      const connFailed = copy.connFailed;
      const skillsFailed = copy.dingtalkSkillsFailed;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      dingtalkConn.listenersReady = true;
      ev.listen('dingtalk:qr', (e) => {
        const p = e.payload || {};
        dingtalkConn.stopTick();
        dingtalkConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return { ...f, phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase, userCode: p.user_code };
        });
      });
      ev.listen('dingtalk:connected', async () => {
        dingtalkConn.stopTick();
        try {
          await invokeTauri('dingtalk_apply_skills');
          dingtalkConn.setFlow(f => ({ ...f, phase: 'done', steps: { ...(f && f.steps), qr: 'done' } }));
          setTimeout(() => dingtalkConn.setFlow(null), 1800);
        } catch (e) {
          dingtalkConn.setFlow(f => ({ ...f, phase: 'error', err: skillsFailed(String(e).slice(0, 220)), errStep: 'qr', steps: { ...(f && f.steps), qr: 'error' } }));
        }
      });
      ev.listen('dingtalk:error', (e) => {
        const p = e.payload || {};
        dingtalkConn.stopTick();
        dingtalkConn.setFlow(f => { const step = (f && f.active) || 'cli'; return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || connFailed), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } }; });
      });
    }

    // ── 腾讯会议连接流程 · 跨视图持久 store(镜像钉钉;纯 OAuth 扫码单段）──
    /* eslint-disable unicorn/no-this-outside-of-class -- same as feishuConn: module-level singleton whose methods reference itself via this */
    const tmeetConn = {
      flow: null, tick: null, listenersReady: false, subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch { /* silent: one failing subscriber must not affect the rest of the broadcast */ } }); },
      startTick() {
        this.stopTick();
        this.tick = setInterval(() => this.setFlow(f => {
          if (!f || f.phase !== 'running') return f;
          const nf = { ...f, sec: (f.sec || 0) + 1 };
          if (f.active === 'cli') nf.pct = Math.min(90, (f.pct || 0) + (90 - (f.pct || 0)) * 0.06 + 1);
          return nf;
        }), 1000);
      },
      stopTick() { if (this.tick) { clearInterval(this.tick); this.tick = null; } },
    };
    /* eslint-enable unicorn/no-this-outside-of-class -- same as feishuConn */
    function ensureTmeetListeners(copy = {}) {
      if (tmeetConn.listenersReady) return;
      const connFailed = copy.connFailed;
      const authIncomplete = copy.tmeetAuthIncomplete;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      tmeetConn.listenersReady = true;
      ev.listen('tmeet:qr', (e) => {
        const p = e.payload || {};
        tmeetConn.stopTick();
        if (p.url) {
          invokeTauri('open_external_url', { url: p.url }).catch(err => {
            console.error('open tmeet auth url failed:', err);
          });
        }
        tmeetConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return { ...f, phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase, browserAuth: true };
        });
      });
      ev.listen('tmeet:connected', async () => {
        tmeetConn.stopTick();
        try {
          // 二次确认真实登录态（统一 readiness；原 tmeet_status 调用已退役）
          const status = await invokeTauri('bundle_readiness', { bundleId: 'tmeet' });
          if (!(status && status.ready)) {
            throw new Error(authIncomplete);
          }
          await invokeTauri('tmeet_apply_skills');
          tmeetConn.setFlow(f => ({ ...f, phase: 'done', steps: { ...(f && f.steps), qr: 'done' } }));
          setTimeout(() => tmeetConn.setFlow(null), 1800);
        } catch (e) {
          tmeetConn.setFlow(f => ({ ...f, phase: 'error', err: String(e && e.message ? e.message : e).slice(0, 220), errStep: 'qr', steps: { ...(f && f.steps), qr: 'error' } }));
        }
      });
      ev.listen('tmeet:error', (e) => {
        const p = e.payload || {};
        tmeetConn.stopTick();
        tmeetConn.setFlow(f => { const step = (f && f.active) || 'cli'; return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || connFailed), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } }; });
      });
    }

    // iOS 风格弹窗（安装/卸载后提示需新建会话生效）
    const TsAlert = ({ alert, _theme, onDismiss, onNewChat, onCancelLoading, copy }) => { // eslint-disable-line no-unused-vars -- theme is kept for the existing props contract
      if (!alert.visible && !alert.loading) return null;
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div
            className="w-[280px] rounded-[20px] overflow-hidden shadow-2xl transition-transform duration-200 scale-100 bg-white/95 backdrop-blur-xl dark:bg-[#2C2C2E]"
            style={{ animation: 'tsAlertIn .2s ease-out' }}
          >
            {alert.loading ? (
              <>
                <div className="px-6 py-8 text-center">
                  <div className="flex justify-center mb-4">
                    <div className={`w-6 h-6 rounded-full border-[2.5px] border-t-transparent border-[#007AFF] dark:border-[#0A84FF]`}
                      style={{ animation: 'tsSpinner .8s linear infinite' }} />
                  </div>
                  <div className={`text-[17px] font-semibold mb-1.5 text-slate-900 dark:text-white`}>
                    {alert.title}
                  </div>
                  {alert.subtitle && (
                    <div className={`text-[13px] leading-relaxed text-slate-500 dark:text-slate-400`}>
                      {alert.subtitle}
                    </div>
                  )}
                </div>
                {alert.cancelable && (
                  <div className={`border-t border-slate-200 dark:border-white/10`}>
                    <button type="button"
                      onClick={() => onCancelLoading && onCancelLoading(alert)}
                      className={`w-full py-3 text-[17px] font-normal text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5`}
                    >
                      {copy.cancel}
                    </button>
                  </div>
                )}
              </>
            ) : (
              <>
                <div className="px-6 pt-6 pb-5 text-center">
                  <div className={`text-[17px] font-semibold mb-1.5 text-slate-900 dark:text-white`}>
                    {alert.title}
                  </div>
                  {alert.subtitle ? (
                    <div className={`text-[13px] leading-relaxed text-slate-500 dark:text-slate-400`}>
                      {alert.subtitle}
                    </div>
                  ) : !alert.isError && (
                    <div className={`text-[13px] leading-relaxed text-slate-500 dark:text-slate-400`}>
                      {alert.isInstall ? copy.installHint : copy.removeHint}
                    </div>
                  )}
                </div>
                <div className={`border-t border-slate-200 dark:border-white/10`}>
                  <button type="button"
                    onClick={onDismiss}
                    className="w-full py-3 text-[17px] font-normal text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5"
                  >
                    {copy.ok}
                  </button>
                </div>
                {!alert.isError && (
                  <div className={`border-t border-slate-200 dark:border-white/10`}>
                    <button type="button"
                      onClick={onNewChat}
                      className={`w-full py-3 text-[17px] font-semibold text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5`}
                    >
                      {copy.newChat}
                    </button>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      );
    };

    // API Key 配置弹窗（需要 config_fields 的工具安装前弹出）
    // eslint-disable-next-line no-unused-vars -- theme is kept for the existing props contract
    const TsConfigDialog = ({ config, _theme, onConfirm, onCancel, copy }) => {
      if (!config) return null;
      const [values, setValues] = useState({}); // eslint-disable-line react-hooks/rules-of-hooks -- when config is null the component returns null before any other hook; for one instance config only goes null→object, so the hook count is stable
      const fields = config.fields || [];
      // required:false 的字段可留空；required:true 字段必须填写后才能连接。
      const canSubmit = fields.every(f => f.required === false || (values[f.key] || '').trim().length > 0);
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div
            className={`w-[300px] rounded-[20px] overflow-hidden shadow-2xl bg-white/95 backdrop-blur-xl dark:bg-[#2C2C2E]`}
            style={{ animation: 'tsAlertIn .2s ease-out' }}
          >
            <div className="px-6 pt-6 pb-4 text-center max-h-[70vh] overflow-y-auto">
              <div className={`text-[17px] font-semibold mb-3 text-slate-900 dark:text-white`}>
                {config.configTitle || copy.configTitle(config.name)}
              </div>
              {config.configDescription && (
                <div className={`text-[12px] leading-relaxed mb-3 text-slate-500 dark:text-slate-400`}>
                  {config.configDescription}
                </div>
              )}
              {config.configDocUrl && (
                <button type="button"
                  onClick={() => invokeTauri('open_external_url', { url: config.configDocUrl })}
                  className={`text-[13px] mb-4 inline-block text-[#007AFF] dark:text-[#0A84FF] hover:underline`}
                >
                  {config.configDocLabel || copy.configDocDefault} →
                </button>
              )}
              {/* 引导链接放最上,不夹在输入框中间 */}
              {fields.find(f => f.helpUrl) && (
                <button type="button"
                  onClick={() => invokeTauri('open_external_url', { url: fields.find(f => f.helpUrl).helpUrl })}
                  className={`text-[13px] mb-4 inline-block text-[#007AFF] dark:text-[#0A84FF] hover:underline`}
                >
                  {copy.configHelpFeishu}
                </button>
              )}
              {/* 所有输入框紧挨着 */}
              {fields.map((field) => (
                <div key={field.key} className="text-left mb-3">
                  {/* biome-ignore lint/a11y/noLabelWithoutControl: field name and input are siblings; the label has no htmlFor target, and switching to span would diverge from the existing structure */}
                  <label className={`text-[13px] font-medium mb-1.5 block text-slate-600 dark:text-slate-300`}>
                    {field.label}
                  </label>
                  <input
                    type={field.secret ? 'password' : 'text'}
                    placeholder={field.placeholder || "sk-..."}
                    value={values[field.key] || ''}
                    onChange={e => setValues(v => ({ ...v, [field.key]: e.target.value }))}
                    className="w-full px-3 py-2 rounded-lg text-[14px] outline-none transition-colors border bg-slate-50 border-slate-200 text-slate-900 placeholder-slate-400 focus:border-[#007AFF] dark:bg-[#1C1C1E] dark:border-[#3A3A3C] dark:text-white dark:placeholder-slate-500 dark:focus:border-[#0A84FF]"
                  />
                  {field.helpText && (
                    <div className={`text-[11px] mt-1 leading-snug text-slate-400 dark:text-slate-500`}>
                      {field.helpText}
                    </div>
                  )}
                </div>
              ))}
            </div>
            <div className={`border-t border-slate-200 dark:border-white/10`}>
              <button type="button"
                onClick={onCancel}
                className={`w-full py-3 text-[17px] font-normal text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5`}
              >
                {copy.cancel}
              </button>
            </div>
            <div className={`border-t border-slate-200 dark:border-white/10`}>
              <button type="button"
                onClick={() => canSubmit && onConfirm(values)}
                disabled={!canSubmit}
                className={`w-full py-3 text-[17px] font-semibold text-center transition-colors ${
                  canSubmit
                    ? 'text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5'
                    : 'text-slate-300 dark:text-slate-600'
                }`}
              >
                {config.backendId === 'feishu' || fields.length > 0 ? copy.configConnect : copy.configInstall}
              </button>
            </div>
          </div>
        </div>
      );
    };

    // 上传包展示名/说明编辑弹窗（extra 覆盖，只改 UI 展示；预填当前覆盖值，
    // 留空即清除覆盖回退默认）。调用方按 updateConfirm 同款模式条件挂载，
    // useState 初值即当前覆盖值。错误留在弹窗内联展示（输入保留不丢），
    // maxLength 与后端 64/240 字符上限一致，Escape 关闭与 backdrop 一致。
    const MAX_DISPLAY_NAME_CHARS = 64;
    const MAX_DISPLAY_DESCRIPTION_CHARS = 240;
    // 与后端 is_display_unsafe_char 同一拒绝集（store.rs）：Cc 控制字符（含
    // Tab/换行/DEL/C1，JS 侧用 \p{Cc} 属性转义表达，与 Rust char::is_control
    // 同一 Unicode 类别且不触发 no-control-regex 规则）+ 软连字符 + 零宽字
    // 符 + 行段/段落分隔符 + bidi 控制符 + BOM。只剥 \r\n 的话，表格里复制
    // 的 TSV（含 Tab）等输入仍会前端放行、后端必败——在这里剥掉，保证能输
    // 入的就能保存。
    const DISPLAY_UNSAFE_CHARS = /[\p{Cc}\u00AD\u200B-\u200D\u2028-\u2029\u202A-\u202E\u2066-\u2069\uFEFF]/gu;
    const stripDisplayUnsafe = s => s.replaceAll(DISPLAY_UNSAFE_CHARS, '');
    const TsEditDisplayDialog = ({ dialog, onConfirm, onCancel, copy }) => {
      const [name, setName] = useState(dialog.name || '');
      const [desc, setDesc] = useState(dialog.description || '');
      const [error, setError] = useState(null);
      // 保存进行中：防重复点击双发请求（后端写幂等但避免重复 IPC/刷新）。
      const [saving, setSaving] = useState(false);
      const inputCls = "w-full px-3 py-2 rounded-lg text-[14px] outline-none transition-colors border bg-slate-50 border-slate-200 text-slate-900 placeholder-slate-400 focus:border-[#007AFF] dark:bg-[#1C1C1E] dark:border-[#3A3A3C] dark:text-white dark:placeholder-slate-500 dark:focus:border-[#0A84FF]";
      const handleConfirm = async () => {
        if (saving) return;
        setSaving(true);
        // 关闭/提交由调用方决定：确认失败时返回错误文案，弹窗保留输入。
        try {
          const err = await onConfirm({ name, description: desc });
          if (err) setError(err);
        } catch (e) {
          setError(String(e));
        } finally {
          setSaving(false);
        }
      };
      return (
        // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container
        <div
          className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm"
          onClick={onCancel}
          // IME 合成中的 Escape 是「取消候选词」，不能连带关闭整个弹窗丢弃输入
          //（同 textarea 的 Enter 守卫，isImeComposing 为合成中标志）。
          // 这是合成期守卫而非键盘快捷键，useKeyWithClickEvents 只认 key handler，不在此报。
          onKeyDown={e => { if (e.key === 'Escape' && !isImeComposing(e)) onCancel(); }}
        >
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; keyboard events need no bubbling here */}
          {/* biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container */}
          <div
            data-testid="edit-display-dialog"
            className={`w-[300px] rounded-[20px] overflow-hidden shadow-2xl bg-white/95 backdrop-blur-xl dark:bg-[#2C2C2E]`}
            style={{ animation: 'tsAlertIn .2s ease-out' }}
            onClick={e => e.stopPropagation()}
          >
            <div className="px-6 pt-6 pb-4 text-center max-h-[70vh] overflow-y-auto">
              <div className={`text-[17px] font-semibold mb-3 text-slate-900 dark:text-white`}>
                {copy.editDisplayTitle(dialog.cardTitle || dialog.backendId)}
              </div>
              <div className="text-left mb-3">
                {/* biome-ignore lint/a11y/noLabelWithoutControl: label and input are siblings; adding htmlFor would require a generated id and diverge from the existing dialog structure */}
                <label className={`text-[13px] font-medium mb-1.5 block text-slate-600 dark:text-slate-300`}>
                  {copy.displayNameLabel}
                </label>
                <input
                  data-testid="edit-display-name"
                  type="text"
                  // biome-ignore lint/a11y/noAutofocus: opening the dialog focuses the name input; focus is the rename intent
                  autoFocus
                  maxLength={MAX_DISPLAY_NAME_CHARS}
                  placeholder={copy.displayNamePlaceholder}
                  value={name}
                  onChange={e => setName(stripDisplayUnsafe(e.target.value))}
                  className={inputCls}
                />
              </div>
              <div className="text-left mb-3">
                {/* biome-ignore lint/a11y/noLabelWithoutControl: label and input are siblings; adding htmlFor would require a generated id and diverge from the existing dialog structure */}
                <label className={`text-[13px] font-medium mb-1.5 block text-slate-600 dark:text-slate-300`}>
                  {copy.displayDescriptionLabel}
                </label>
                <textarea
                  data-testid="edit-display-description"
                  rows={3}
                  maxLength={MAX_DISPLAY_DESCRIPTION_CHARS}
                  placeholder={copy.displayDescriptionPlaceholder}
                  value={desc}
                  onChange={e => setDesc(stripDisplayUnsafe(e.target.value))}
                  // 粘贴多行文本会绕过 Enter 拦截（paste 不派发 keydown），留进
                  // 值里就是必败保存——在粘贴处同样剥离（与 onChange 同口径，
                  // 顺带兜住任何残留来源）。
                  onPaste={e => {
                    const text = (e.clipboardData || window.clipboardData)?.getData('text/plain');
                    if (text == null) return;
                    e.preventDefault();
                    // 按码点（Array.from 按 code point 切分）拼接与截断：直接
                    // slice 按 UTF-16 单元会把粘贴位置上的代理对劈成两半（后端
                    // chars() 校验对半个代理报错，报错文案不可读）；长度上限也
                    // 按码点计，与后端 chars().count() 同口径（maxLength 的
                    // UTF-16 计数只影响可输入上限的松紧，不会超后端限）。
                    const clamp = s => [...stripDisplayUnsafe(s)].slice(0, MAX_DISPLAY_DESCRIPTION_CHARS).join('');
                    const pos = e.target.selectionStart ?? e.target.value.length;
                    const head = clamp(e.target.value.slice(0, pos));
                    const tail = e.target.value.slice(e.target.selectionEnd ?? pos);
                    setDesc(clamp(head + clamp(text) + tail));
                  }}
                  // 后端展示说明只接受单行（控制字符校验拒换行），Enter 在此
                  // 只会换来一次必败的保存——直接拦截，避免用户按回车后困惑。
                  // IME 合成中的 Enter 是确认候选词（中/日文输入法），不得拦截。
                  onKeyDown={e => { if (e.key === 'Enter' && !isImeComposing(e)) e.preventDefault(); }}
                  className={`${inputCls} resize-none`}
                />
              </div>
              <div className={`text-[11px] text-left leading-snug text-slate-400 dark:text-slate-500`}>
                {copy.editDisplayHint}
              </div>
              {error && (
                <div data-testid="edit-display-error" className="mt-2 text-left text-[12px] leading-snug text-red-500 dark:text-red-400">
                  {copy.operationFailedWith(error)}
                </div>
              )}
            </div>
            <div className={`border-t border-slate-200 dark:border-white/10`}>
              <button
                type="button"
                onClick={onCancel}
                className={`w-full py-3 text-[17px] font-normal text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5`}
              >
                {copy.cancel}
              </button>
            </div>
            <div className={`border-t border-slate-200 dark:border-white/10`}>
              <button
                type="button"
                data-testid="edit-display-save"
                onClick={handleConfirm}
                disabled={saving}
                className={`w-full py-3 text-[17px] font-semibold text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5 disabled:opacity-50`}
              >
                {copy.editDisplaySave}
              </button>
            </div>
          </div>
        </div>
      );
    };

    // Obsidian 连接前探测引导卡：未安装 → 引导下载；没库 / 库丢失 → 引导建库/重开
    const TsObsidianGuide = ({ guide, _theme, onCancel, onDownload, onRetry, allowDownload = true, copy }) => { // eslint-disable-line no-unused-vars -- theme is kept for the existing props contract
      if (!guide) return null;
      const COPY = copy.obsidianGuide;
      const c = COPY[guide.state] || COPY.not_installed;
      const btn = (label, on, cls) => (
        <div className={`border-t border-slate-200 dark:border-white/10`}>
          <button type="button" onClick={on} className={`w-full py-3 text-center transition-colors ${cls}`}>{label}</button>
        </div>
      );
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div className={`w-[300px] rounded-[20px] overflow-hidden shadow-2xl bg-white/95 backdrop-blur-xl dark:bg-[#2C2C2E]`} style={{ animation: 'tsAlertIn .2s ease-out' }}>
            <div className="px-6 pt-6 pb-4 text-center">
              <div className="text-[34px] mb-2">📖</div>
              <div className={`text-[17px] font-semibold mb-2 text-slate-900 dark:text-white`}>{c.title}</div>
              <div className={`text-[13px] leading-relaxed text-slate-500 dark:text-slate-400`}>{!allowDownload && guide.state === 'not_installed' ? COPY.desktopHint : c.body}</div>
            </div>
            {allowDownload && c.primary && btn(c.primary, onDownload, `text-[17px] font-semibold text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5`)}
            {btn(c.retry, onRetry, `text-[15px] text-slate-600 active:bg-slate-100 dark:text-slate-300 dark:active:bg-white/5`)}
            {btn(copy.cancel, onCancel, `text-[15px] text-slate-400 active:bg-slate-100 dark:text-slate-500 dark:active:bg-white/5`)}
          </div>
        </div>
      );
    };

    /* eslint-disable sonarjs/cognitive-complexity -- tool store main view (list/detail/install/OAuth flows);legacy view; tracked separately */
    const ToolStoreView = ({ theme, t, onNewChat }) => {
      const storeCopy = t.uiToolStore;
      const detailCopy = t.uiToolDetails;
      // 数据文件(tool-common.jsx)里技能/分类/精选的中文 label/title/subtitle/desc:
      // 按 localizeTool() 同款 overlay 模式,从 uiToolStore 词条做三语覆盖,数据文件本身不改。
      const storeData = storeCopy.storeData || {};
      const localizeSkill = (s) => {
        const ov = (storeData.skills || {})[s.backendId || s.id];
        return ov ? { ...s, ...ov } : s;
      };
      const externalAuthAvailable = canStartExternalAuth();
      const canMutateToolStore = can('toolStoreMutations');
      const [searchQuery, setSearchQuery] = useState('');
      const [activeCategory, setActiveCategory] = useState('all');
      const [selectedTool, setSelectedTool] = useState(null);
      const [toolStates, setToolStates] = useState({});
      // list_marketplace_tools 原始返回：自定义 MCP（不在内置 tsToolsData 里的）
      // 据此动态合成卡片（安装时显示、卸载后消失），不依赖前端硬编码。
      const [toolBackend, setToolBackend] = useState([]);
      const [toolAuthStates, setToolAuthStates] = useState({});
      // 配套技能 id → 所属 MCP id(由 list_marketplace_tools 的 companion_skills 反建,manifest 单一真源;
      // 映射与安装态无关)。「安装」始终走组合包(装 MCP 联动装技能);卡片安装态/卸载/可见性
      // 按 MCP 是否已装分流,与后端条件认领(skill_owner_package)对齐。
      const [skillToMcp, setSkillToMcp] = useState({});
      const [busyId, setBusyId] = useState(null);
      const busyRef = useRef(null); // 拖放 controller 经 ref 读最新 busyId(闭包不刷新)
      busyRef.current = busyId;
      // 展示编辑弹窗开着时拒绝拖放导入（同经 ref 读最新值）：导入成功会按新包
      // 自动预填并整体替换 editDisplay（key 重挂载），A 包未保存的输入被静默
      // 丢弃——模态弹窗期间拖放应视为不可达输入。声明在 editDisplay 之前仅为
      // 就近聚合 ref，赋值同步在其 useState 声明处。
      const editDisplayRef = useRef(null);
      const [dropActive, setDropActive] = useState(false); // 拖放 overlay 可见性
      // 页面级拖放导入技能包:capture 阶段接管 document,隔离全局附件通道
      // (见 attachment-drop-controller.js;canAccept 经 busyRef 读最新值)。
      useEffect(() => {
        const ctrl = window.PinvouAttachmentDropController;
        if (!ctrl) return;
        return ctrl.install({
          document,
          capture: true,
          canAccept: () => canMutateToolStore && !busyRef.current && !editDisplayRef.current,
          onActiveChange: setDropActive,
          onFiles: handleZipDrop,
        });
      // eslint-disable-next-line react-hooks/exhaustive-deps -- drag-drop listener mounts once; canMutateToolStore/handleZipDrop are evaluated at call time through the canAccept/onFiles closures
      }, []);
      const [alert, setAlert] = useState({ visible: false, loading: false, title: '', subtitle: '', isInstall: false, isError: false });
      const oauthRequestRef = useRef({});
      const [configDialog, setConfigDialog] = useState(null); // { backendId, name, fields }
      const [obsidianGuide, setObsidianGuide] = useState(null); // {backendId,name,state,vault_path} 未安装/没库引导
      const [groupBy, setGroupBy] = useState('type'); // 列表视图主维度:'type'(按类型) | 'business'(按业务)
      const [installedOnly, setInstalledOnly] = useState(false); // 头像入口:只看已安装
      const [skillBackend, setSkillBackend] = useState([]); // list_marketplace_skills 原始返回
      // 按会话模式配置工具可见性（预过滤，与开关正交）：managingVisibility = 编辑态；
      // hiddenByMode = { plain: Set, code: Set }——每个模式被设为「不可见」的包 id。
      const [managingVisibility, setManagingVisibility] = useState(false);
      const [hiddenByMode, setHiddenByMode] = useState({ plain: new Set(), code: new Set() });
      // ref 镜像：setState updater 执行时机不保证同步，后端载荷以 ref 为基准，
      // 与 UI 同一份 prev，快速连续勾选不丢写。
      const hiddenByModeRef = useRef({ plain: new Set(), code: new Set() });
      // 插件指南弹窗：拖入安装说明 + 插件包介绍 + 规范文档下载。
      const [showGuide, setShowGuide] = useState(false);
      // 下载规范文档（桌面端走保存对话框；web 平台无此命令，静默忽略）。
      const downloadSpec = () => {
        invokeTauri('export_plugin_spec').catch(() => {});
      };
      const [visibilityLoaded, setVisibilityLoaded] = useState(false);
      const loadHiddenByMode = () => {
        Promise.all([
          invokeTauri('get_bundle_visibility', { scope: 'plain' }),
          invokeTauri('get_bundle_visibility', { scope: 'code' }),
        ]).then(([plain, code]) => {
          const loaded = { plain: new Set(plain || []), code: new Set(code || []) };
          hiddenByModeRef.current = loaded;
          setHiddenByMode(loaded);
          setVisibilityLoaded(true);
        }).catch(() => {
          // 读失败不静默清空（后端整集覆盖语义下会把用户已配置的可见性规则冲掉）：
          // 保留现有状态并提示；加载成功前勾选入口不可用（二轮评审）。
          setVisibilityLoaded(false);
          setAlert({ visible: true, loading: false, title: storeCopy.visibilityLoadFailed, isInstall: false, isError: true });
        });
      };
      useEffect(() => {
        if (managingVisibility) loadHiddenByMode();
      // eslint-disable-next-line react-hooks/exhaustive-deps -- fetch only at the moment edit mode is entered; loadHiddenByMode is an in-component closure
      }, [managingVisibility]);
      // 勾选/取消某工具在某模式的可见性：checked = 可见。
      const toggleModeVisibility = (id, mode, checked) => {
        if (!canMutateToolStore || !visibilityLoaded || !id) return;
        // 可见性按包 id 落库（后端 save_hidden_bundles_for 经 to_package_id 归一为包
        // id，scope.rs；读回经 normalize_stored_pkg_ids 按当前认领再归一）：companion
        // 技能卡（government-writing→gongwen 等）在 MCP 已装时映射为所属包 id；
        // MCP 未装（技能独立态）时按技能 id 直接落库——与后端条件认领同口径。
        const companionMcpId = skillToMcp[id];
        const companionBs = companionMcpId ? bundleStates[companionMcpId] : null;
        const companionInstalled = companionMcpId
          ? (companionBs ? companionBs.installed : !!toolStates[companionMcpId])
          : false;
        const pkgId = companionInstalled ? companionMcpId : id;
        const prev = hiddenByModeRef.current;
        const next = new Set(prev[mode] || []);
        // 恢复可见时同时删包 id 与原始技能 id：兼容历史版本按独立技能 id 落库的
        // hidden 条目（未装→装边界），该条目随本次整集写回被后端归一清理。
        if (checked) { next.delete(pkgId); next.delete(id); } else next.add(pkgId);
        const nextState = { ...prev, [mode]: next };
        hiddenByModeRef.current = nextState;
        setHiddenByMode(nextState);
        invokeTauri('set_bundle_visibility', { bundleIds: [...next], scope: mode })
          .catch((e) => {
            // 写失败：回滚本地勾选态（重读后端为基准）并提示，不静默吞错（三轮评审）。
            loadHiddenByMode();
            setAlert({ visible: true, loading: false, title: storeCopy.operationFailedWith(String(e)), isInstall: false, isError: true });
          });
      };
      // bundle_readiness 统一取数（Phase 2 第八刀，§3.3）：逐连接器 status 命令
      // （feishu/wecom/dingtalk/tmeet/ima_status）的前端调用全部移除，installed /
      // ready / actions 均来自统一命令。CLI/ima 的「已连接」= ready（授权现算）。
      const [bundleStates, setBundleStates] = useState({}); // bundleId → BundleReadinessResult
      // 空 actions 视为「无动作数据」回退旧分支（后端实现对每个状态至少下发一个
      // 动作，空数组只可能是异常中间态——不能让卡片没有按钮）。
      const actionsOf = (bs) => (bs && Array.isArray(bs.actions) && bs.actions.length > 0 ? bs.actions : undefined);
      const feishuConnected = !!bundleStates.feishu?.ready;
      const wecomConnected = !!bundleStates.wecom?.ready;
      const dingtalkConnected = !!bundleStates.dingtalk?.ready;
      const tmeetConnected = !!bundleStates.tmeet?.ready;
      const imaConnected = !!bundleStates.ima?.ready;
      // 飞书连接流程状态机（取代旧阻塞式扫码浮层）：null=idle
      // { phase:'running'|'qr'|'error'|'done', steps:{runtime,cli,connect,qr}, active, pct, sec, log, err, qr, qrUrl, qrPhase }
      const [feishuFlow, setFeishuFlow] = useState(feishuConn.flow); // 从跨视图 store 水合：切走再回来不丢进度

      // 企业微信(CLI 路线)连接流程卡(跨视图水合);连接态由 bundleStates 派生(见上)
      const [wecomQr, setWecomQr] = useState(null); // { qr: dataUrl, url } 扫码弹窗(单段)
      const [wecomFlow, setWecomFlow] = useState(wecomConn.flow); // 企微连接流程卡(跨视图水合)

      // 钉钉(CLI 路线)连接流程卡;连接态由 bundleStates 派生
      const [dingtalkFlow, setDingtalkFlow] = useState(dingtalkConn.flow);

      // 腾讯会议(CLI 路线)连接流程卡;连接态由 bundleStates 派生
      const [tmeetFlow, setTmeetFlow] = useState(tmeetConn.flow);

      // 从后端加载已安装状态 + 统一 readiness（Phase 2 第八刀：逐连接器 status
      // 命令退役，installed/ready/actions 统一经 bundle_readiness 取数）。
      // 前置声明在所有订阅副作用之前（它们在事件回调里调用本函数）。
      const loadBackendState = async () => {
        // 提升为函数级声明：下方 readiness 批量取数（独立的内层 try）也要用
        // 本次刚拉回的 list——try 块内 const 出了块就是 ReferenceError，整个
        // readiness 批次会被内层 catch 静默吞掉（eslint no-undef 能钉住）。
        let list = [];
        try {
          const fetched = await invokeTauri('list_marketplace_tools');
          list = Array.isArray(fetched) ? fetched : [];
          const states = {};
          const s2m = {}; // 配套技能 → 所属 MCP(manifest companion_skills 反建,单一真源)。
          // 映射与安装态无关：组合包语义要求 companion 卡的「安装」始终路由到
          // 所属 MCP（装 MCP 联动装技能）；「卸载」与卡片安装态在下游按 MCP
          // 是否已装分流（MCP 已装 → 包级卸载；仅技能独立已装 → 技能级卸载，
          // 后端 uninstall 按实际物理位置删除，G3）。
          list.forEach(t => {
            states[t.id] = t.installed;
            (t.companion_skills || []).forEach(sid => { s2m[sid] = t.id; });
          });
          setToolStates(states);
          setSkillToMcp(s2m);
          setToolBackend(list);
          const authEntries = await Promise.all(tsToolsData
            .filter(tool => tool.oauthMcp && tool.backendId)
            .map(async (tool) => {
              try {
                const status = await invokeTauri('get_marketplace_tool_auth_status', { toolId: tool.backendId });
                return [tool.backendId, status];
              } catch (err) {
                console.error('get_marketplace_tool_auth_status failed:', tool.backendId, err);
                return null;
              }
            }));
          setToolAuthStates(prev => {
            const next = { ...prev };
            authEntries.filter(Boolean).forEach(([id, status]) => { next[id] = status; });
            return next;
          });
        } catch (e) {
          console.error('list_marketplace_tools failed:', e);
        }
        let skillList = [];
        try {
          const skills = await invokeTauri('list_marketplace_skills');
          skillList = Array.isArray(skills) ? skills : [];
          setSkillBackend(skillList);
        } catch (e) {
          console.error('list_marketplace_skills failed:', e);
        }
        // 统一 readiness 批量取数：未知包（companion 被认领后不再独立成包等）
        // 单条失败只记日志、该包回退旧来源（list_marketplace_* 的状态位）。
        try {
          // 上传 MCP/组合包不在 tsToolsData（内置）也不在 skillList（技能），
          // 但 edit_display 动作下发与卡面展示名覆盖都来自 bundle_readiness，
          // 必须并入批量取数（否则上传 MCP 卡永远拿不到 actions/覆盖值）。
          // 用本次刚拉回的 list（而非 toolBackend state）：闭包里的 state 是
          // 渲染时快照，首挂载/刚上传后为空 → 上传 MCP 卡漏批直到下次无关刷新。
          const customToolIds = (Array.isArray(list) ? list : [])
            .map(x => x.id)
            .filter(id => id && tsToolsData.every(t => t.backendId !== id));
          // 三源并集去重（组合包 mcpId 可能同时进 tool/skill 列表）
          const ids = [...new Set([
            ...tsToolsData.map(x => x.backendId).filter(Boolean),
            ...customToolIds,
            ...skillList.map(x => x.id),
          ])];
          const entries = await Promise.all(ids.map(async (id) => {
            try {
              return [id, await invokeTauri('bundle_readiness', { bundleId: id })];
            } catch (err) {
              console.error('bundle_readiness failed:', id, err);
              return null;
            }
          }));
          setBundleStates(Object.fromEntries(entries.filter(Boolean)));
        } catch (e) {
          console.error('bundle_readiness batch failed:', e);
        }
      };

      useEffect(() => { loadBackendState(); }, []);

      // 订阅跨视图 store：把 store 状态镜像进本组件渲染，并在完成/失败时做组件级收尾
      //（弹窗、刷新连接态）。真正的事件监听/秒表在模块级 feishuConn 里，切视图不丢。
      useEffect(() => {
        if (!externalAuthAvailable) return;
        ensureFeishuListeners(storeCopy);
        let prevPhase = feishuConn.flow && feishuConn.flow.phase;
        const unsub = feishuConn.subscribe((flow) => {
          setFeishuFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setBusyId(null);
              loadBackendState();
              setAlert({ visible: true, loading: false, title: storeCopy.connectedTool(storeCopy.toolNames.feishu), subtitle: detailCopy.actions.enabled, isInstall: true, isError: false, toolId: 'feishu' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setFeishuFlow(feishuConn.flow); // (重)挂载即水合当前进度
        return unsub;
      // eslint-disable-next-line react-hooks/exhaustive-deps -- subscription mounts/unmounts only with externalAuthAvailable; the copy snapshot is read on demand by the callback, so resubscribing is unnecessary
      }, [externalAuthAvailable]);

      // 订阅企业微信 store(镜像飞书):镜像进渲染 + 完成/失败收尾
      useEffect(() => {
        if (!externalAuthAvailable) return;
        ensureWecomListeners(storeCopy);
        let prevPhase = wecomConn.flow && wecomConn.flow.phase;
        const unsub = wecomConn.subscribe((flow) => {
          setWecomFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setBusyId(null);
              loadBackendState();
              setAlert({ visible: true, loading: false, title: storeCopy.connectedTool(storeCopy.toolNames.wecom), subtitle: detailCopy.actions.enabled, isInstall: true, isError: false, toolId: 'wecom' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setWecomFlow(wecomConn.flow);
        return unsub;
      // eslint-disable-next-line react-hooks/exhaustive-deps -- subscription mounts/unmounts only with externalAuthAvailable; the copy snapshot is read on demand by the callback, so resubscribing is unnecessary
      }, [externalAuthAvailable]);

      // 订阅钉钉 store(镜像企微):镜像进渲染 + 完成/失败收尾
      useEffect(() => {
        if (!externalAuthAvailable) return;
        ensureDingtalkListeners(storeCopy);
        let prevPhase = dingtalkConn.flow && dingtalkConn.flow.phase;
        const unsub = dingtalkConn.subscribe((flow) => {
          setDingtalkFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setBusyId(null);
              loadBackendState();
              setAlert({ visible: true, loading: false, title: storeCopy.connectedTool(storeCopy.toolNames.dingtalk), subtitle: detailCopy.actions.enabled, isInstall: true, isError: false, toolId: 'dingtalk' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setDingtalkFlow(dingtalkConn.flow);
        return unsub;
      // eslint-disable-next-line react-hooks/exhaustive-deps -- subscription mounts/unmounts only with externalAuthAvailable; the copy snapshot is read on demand by the callback, so resubscribing is unnecessary
      }, [externalAuthAvailable]);

      // 订阅腾讯会议 store(镜像钉钉):镜像进渲染 + 完成/失败收尾
      useEffect(() => {
        if (!externalAuthAvailable) return;
        ensureTmeetListeners(storeCopy);
        let prevPhase = tmeetConn.flow && tmeetConn.flow.phase;
        const unsub = tmeetConn.subscribe((flow) => {
          setTmeetFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setBusyId(null);
              loadBackendState();
              setAlert({ visible: true, loading: false, title: detailCopy.actions.connectedTmeet, subtitle: detailCopy.actions.enabled, isInstall: true, isError: false, toolId: 'tmeet' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setTmeetFlow(tmeetConn.flow);
        return unsub;
      // eslint-disable-next-line react-hooks/exhaustive-deps -- subscription mounts/unmounts only with externalAuthAvailable; the copy snapshot is read on demand by the callback, so resubscribing is unnecessary
      }, [externalAuthAvailable]);

      // 企微连接编排事件:后端推进度,前端驱动 UI。
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return;
        const unlisten = [];
        ev.listen('wecom:qr', (e) => {
          const p = e.payload || {};
          // 二维码到了 → 清掉一直显示的"正在生成…"loading,再弹出二维码弹窗。
          setAlert(a => ({ ...a, visible: false, loading: false }));
          setWecomQr({ qr: p.qr_data_url, url: p.url, phase: p.phase });
        }).then(u => { unlisten.push(u); });
        ev.listen('wecom:connected', () => {
          setWecomQr(null); setBusyId(null);
          // 连上 → 按规则写技能(默认启用),企微技能即刻对模型可见;连接态经 readiness 重取。
          invokeTauri('wecom_apply_skills').catch(() => {});
          loadBackendState();
          setAlert({ visible: true, loading: false, title: storeCopy.connectedTool(storeCopy.toolNames.wecom), subtitle: '', isInstall: true, isError: false, toolId: 'wecom' });
          notifyComposerToolsChanged();
        }).then(u => { unlisten.push(u); });
        ev.listen('wecom:error', (e) => {
          const p = e.payload || {};
          setWecomQr(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: storeCopy.connectFailed(storeCopy.toolNames.wecom), subtitle: String(p.message || '').slice(0, 240), isError: true });
        }).then(u => { unlisten.push(u); });
        return () => { unlisten.forEach(u => { try { u(); } catch { /* silent: listeners may already be stale at unmount */ } }); };
      // eslint-disable-next-line react-hooks/exhaustive-deps -- subscription mounts/unmounts only with externalAuthAvailable; the copy snapshot is read on demand by the callback, so resubscribing is unnecessary
      }, [externalAuthAvailable]);

      // 合并后端安装状态到 mock 数据(飞书/企微/钉钉的 installed = 已连接)
      // 业务分类直接取条目数据 category(tool-common.jsx 已落业务类 id),不再按 id 硬编码映射。
      const builtinTools = tsToolsData.map(baseTool => localizeTool(baseTool, t)).map(t => {
        const authState = t.oauthMcp && t.backendId ? toolAuthStates[t.backendId] : null;
        // 统一 readiness（刀8）：CLI/ima 卡的「已连接」= ready；其余包 = installed
        // （BundleStore 真相源）。readiness 缺失（未知包/取数失败）回退旧来源。
        const bs = t.backendId ? bundleStates[t.backendId] : null;
        // 功能事实（刀9）：版本号与配置弹窗字段定义切后端 bundle 源——版本取
        // lock 表钉住值（飞书从腐化的 v1.0.56 修正为 1.0.65）；tmeet（npm 自报）/
        // ima（无版本概念）后端为空时回退 overlay。desc/category 维持 overlay：
        // desc 已由 uiToolDetails 三语本地化，category 的 manifest 中文词汇与前端
        // 分组 id 不兼容（统一词汇属后续清理）。
        const bf = bs && bs.bundle ? bs.bundle : null;
        return {
          ...t,
          version: bf && bf.version ? `v${String(bf.version).replace(/^v/i, '')}` : t.version,
          configFields: mergeConfigFields(bf ? bf.config_fields : null, t.configFields),
          logoSrc: THIRD_PARTY_TOOL_LOGOS[t.backendId] || THIRD_PARTY_TOOL_LOGOS[t.id] || null,
          installed: t.feishuCli
            ? feishuConnected
            : t.wecomCli
            ? wecomConnected
            : t.dingtalkCli
            ? dingtalkConnected
            : t.tmeetCli
            ? tmeetConnected
            : t.imaOpenapi
            ? imaConnected
            : t.oauthMcp
            ? authState?.status === 'connected'
            : (t.backendId ? (bs ? bs.installed : (toolStates[t.backendId] || false)) : false),
          authStatus: authState?.status || 'not_installed',
          authMessage: authState?.message || '',
          mcpConfigured: !!authState?.mcp_configured,
          oauthTokenPresent: !!authState?.oauth_token_present,
          // OAuth MCP 暂不下发 actions（后端 ready 未纳入 token 态），走 TsActionBtn
          // 旧分支；其余包动作驱动渲染。
          actions: t.oauthMcp ? undefined : actionsOf(bs),
          // ima 连接器卡认领 ima-skills(不单独成卡):其可更新态挂在连接器卡上,
          // 更新时映射回 update_marketplace_skill('ima-skills')(见 handleSkillUpdate)。
          updateAvailable: t.imaOpenapi
            ? !!(skillBackend.find(x => x.id === 'ima-skills') || {}).update_available
            : false,
        };
      });
      // 自定义 MCP（上传的，不在内置 tsToolsData 里的）动态合成卡片：安装时显示、
      // 卸载后从 list_marketplace_tools 消失，不依赖前端硬编码。展示文案走 i18n
      // overlay（localizeTool，按 backendId 三语覆盖），后端 name/desc 兜底。
      const customMcpTools = toolBackend
        .filter(x => tsToolsData.every(t => t.backendId !== x.id))
        .map(x => {
          const bs = bundleStates[x.id] || null;
          // 卡面标题/说明优先取 readiness bundle 的生效值（后端已应用 extra
          // 展示名/说明覆盖）；无 readiness 回退 list_marketplace_tools 现状。
          const bf = (bs && bs.bundle) || null;
          const base = {
            id: 'mcp-' + x.id, backendId: x.id, title: (bf && bf.name) || x.name || x.id, subtitle: '',
            category: 'other', type: 'MCP Server', mcpServer: true,
            version: x.version ? `v${String(x.version).replace(/^v/i, '')}` : '—',
            latency: storeCopy.localLatency, desc: (bf && bf.description) || x.description || '',
            icon: Server, color: 'bg-gradient-to-b from-slate-400 to-slate-600',
            installed: bs ? bs.installed : !!x.installed,
            authRequired: false, userUploaded: true,
            actions: actionsOf(bs),
          };
          return localizeTool(base, t);
        });
      const tools = [...builtinTools, ...customMcpTools];
      // 按 backendId 取已 localize 的工具卡;兜底分支也走 localizeTool,避免 en/ja 下漏出中文原文。
      const findLocalizedTool = (backendId) =>
        tools.find(x => x.backendId === backendId) || localizeTool(tsToolsData.find(x => x.backendId === backendId), t);
      const isToolVisibleOnPlatform = (tool) => (
        externalAuthAvailable
        || !isRestrictedExternalAuthTool(tool)
        || !!tool.installed
      );
      // 技能卡 = 预置(静态卡合并安装状态) + companion 技能(后端数据合成) + 用户上传
      const presetSkills = tsSkillsData.map(localizeSkill).map(s => {
        if (s.builtin) return { ...s, installed: true };
        // 有配套 MCP 的技能(公文=gongwen,manifest companion_skills 声明)→ 跟随该 MCP 工具态;
        // 否则读统一 readiness(store 真相源),缺失回退技能后端。
        // 可更新态始终读技能后端(内容与嵌入资源比对),与安装态来源无关。
        const be = skillBackend.find(x => x.id === s.backendId);
        const updateAvailable = !!(be && be.update_available);
        const mcpId = skillToMcp[s.backendId] || null;
        if (mcpId) {
          const mcpBs = bundleStates[mcpId];
          const mcpInstalled = mcpBs ? mcpBs.installed : !!toolStates[mcpId];
          // 安装态：MCP 或技能任一已装即显示已装（G3：独立安装的 companion
          // 技能可见可管）。动作与安装态同源拆分：MCP 已装、或两者皆未装（安装
          // 入口始终路由到所属 MCP 包）走包级 readiness——配置字段（配置/连接
          // 按钮）只有包级 readiness 知道；仅技能独立已装（G3 混合态）改走技能级
          // readiness 给出卸载入口，与 handleAction 的技能级卸载分流一致。
          const skillBs = s.backendId ? bundleStates[s.backendId] : null;
          const skillInstalled = skillBs ? skillBs.installed : (be ? be.installed : false);
          return {
            ...s,
            installed: mcpInstalled || skillInstalled,
            updateAvailable,
            actions: (!mcpInstalled && skillInstalled) ? actionsOf(skillBs) : actionsOf(mcpBs),
          };
        }
        const bs = s.backendId ? bundleStates[s.backendId] : null;
        return {
          ...s,
          installed: bs ? bs.installed : (be ? be.installed : false),
          updateAvailable,
          actions: actionsOf(bs),
        };
      });
      // 后端技能卡合成(统一路径):预置技能中无静态卡的(公文/PPT/可视化等)由
      // list_marketplace_skills 数据合成——真实预置技能取代前端空壳卡,
      // 展示文案走 i18n overlay(localizeSkill),精选位图片/版式走 tsSkillFeaturedAssets,
      // 安装态:有配套 MCP(companion 声明)跟随 MCP 工具态,纯技能读后端 installed。
      const staticSkillIds = new Set(tsSkillsData.map(s => s.backendId).filter(Boolean));
      // 已被非 MCP 连接器认领的技能不再单独成卡:ima-skills 归 ima 连接器包(后端注册表 V5 预认领;
      // ima 非 MCP、不在 companion_skills 反建范围),其连接器卡即包卡,避免「一个产品两张卡」。
      const CONNECTOR_CLAIMED_SKILLS = new Set(['ima-skills']);
      const companionSkillCards = skillBackend
        .filter(x => !x.user_uploaded && !staticSkillIds.has(x.id) && !CONNECTOR_CLAIMED_SKILLS.has(x.id))
        .map(x => {
          const mcpId = skillToMcp[x.id] || null;
          // 组合包卡的业务分类跟随配套 MCP(公文=gongwen→docs;pptx 连接器卡已删,元数据在 tsToolWelcomeData)
          const mcpEntry = mcpId
            ? (tsToolsData.find(t => t.backendId === mcpId) || tsToolWelcomeData.find(t => t.backendId === mcpId))
            : null;
          // 安装态：MCP 或技能任一已装即显示已装（G3：独立安装的 companion
          // 技能可見、可卸载）。动作与安装态同源拆分：MCP 已装、或两者皆未装
          // （安装入口始终路由到所属 MCP 包）走包级 readiness——配置/连接按钮
          // 依赖包级配置字段（如腾讯文档 Token），技能级 readiness 给不出；仅
          // 技能独立已装（G3 混合态）改走技能级 readiness 给出卸载入口，
          // 与 handleAction 的技能级卸载分流一致。
          const mcpBs = mcpId ? bundleStates[mcpId] : null;
          const skillBs = bundleStates[x.id];
          const mcpInstalled = mcpId ? (mcpBs ? mcpBs.installed : !!toolStates[mcpId]) : false;
          const skillInstalled = skillBs ? skillBs.installed : !!x.installed;
          return localizeSkill({
            id: 'mcp-skill-' + x.id, backendId: x.id, title: x.title, subtitle: x.subtitle || '',
            // 有配套 MCP(companion)的卡 = 工具包:徽标与分组归 bundle,安装态跟随 MCP。
            // 业务类 category 跟随 companion MCP(mcpEntry 查 tsToolsData/tsToolWelcomeData);
            // 查不到 MCP 或其无 category 时标 'skill'——仅作类型维度标记
            // (getToolTypeGroup 据此归 Skill 组),业务维度由 getToolBusinessGroup
            // 落「其他」(初步设计,见 tool-common 注释)。
            category: mcpEntry ? (mcpEntry.category || 'skill') : 'skill',
            type: mcpId ? ((storeCopy.typeGroups || {}).bundle || 'Bundle') : 'Skill',
            companionBundle: !!mcpId,
            version: '—', latency: storeCopy.localLatency, desc: x.description || '',
            icon: tsSkillIconByName[x.icon] || Package, color: x.color || 'bg-gradient-to-b from-slate-400 to-slate-600',
            installed: mcpId ? (mcpInstalled || skillInstalled) : skillInstalled,
            updateAvailable: !!x.update_available,
            actions: mcpId
              ? ((!mcpInstalled && skillInstalled) ? actionsOf(skillBs) : actionsOf(mcpBs))
              : actionsOf(skillBs),          });
        });
      const uploadedSkills = skillBackend.filter(x => x.user_uploaded).map(x => ({
        id: 'up-' + x.id, backendId: x.id, title: x.title, subtitle: x.subtitle || storeCopy.uploadedSkill,
        // 用户上传技能无业务归属元数据,业务分组落「其他」;类型分组仍按 userUploaded 归 Skill。
        category: 'other', type: 'Skill', version: '—', latency: storeCopy.localLatency, desc: x.description || '',
        icon: Package, color: 'bg-gradient-to-b from-slate-400 to-slate-600', installed: true, userUploaded: true,
        actions: actionsOf(bundleStates[x.id]),
      }));
      const skillCards = [...presetSkills, ...companionSkillCards, ...uploadedSkills];

      // 双维度分组:主维度(groupBy)决定二级筛选集合,另一维度决定下方分区(section)。
      // 含 companion_skills 的 MCP = 工具包(skillToMcp 的值即其 id,manifest 反建,单一真源)。
      const bundleMcpIds = Object.values(skillToMcp);
      // 组合包的 MCP 连接器卡不进列表:包由 companion 合成卡唯一代表(取代已删的 LOCAL_TOOLS 硬编码)。
      // 条目数据仍保留在 tsToolsData(详情/安装/配置流程经 findLocalizedTool 消费)。
      const connectorTools = tools.filter(t => !bundleMcpIds.includes(t.backendId) && isToolVisibleOnPlatform(t));
      const listItems = [...connectorTools, ...skillCards]; // 连接器 + 技能全放一起
      // 搜索全局:有搜索词时跨「连接器 + 全部技能」检索,不受分类限制(「我的工具」内搜索仍限已安装)
      const searching = searchQuery.trim() !== '';
      const isLaunchedTool = tool => !!tool.backendId || !!tool.builtin || !!tool.userUploaded;
      const typeGroupOf = tool => getToolTypeGroup(tool, bundleMcpIds);
      const catLabel = id => (storeData.categories || {})[id] || (tsCategories.find(c => c.id === id) || {}).label || id;
      const typeLabel = id => ((storeCopy.typeGroups || {})[id]) || id;
      const primaryGroupOf = groupBy === 'type' ? typeGroupOf : getToolBusinessGroup;
      const sectionGroupOf = groupBy === 'type' ? getToolBusinessGroup : typeGroupOf;
      // 业务分区顺序即 TOOL_BUSINESS_GROUPS('skill' 已不再是业务分组——仅标 'skill'
      // 的条目由 getToolBusinessGroup 落 'other';类型维度仍有 Skill 组)。
      const sectionOrder = groupBy === 'type' ? TOOL_BUSINESS_GROUPS : TOOL_TYPE_GROUPS;
      const sectionLabelOf = groupBy === 'type' ? catLabel : typeLabel;
      // 二级筛选 chips:第一项恒为「全部」,其余只展示当前列表里有内容的组。
      // eslint-disable-next-line react-hooks/exhaustive-deps -- groupChips is rebuilt on every render; using it as the dep of the effect below is the existing contract, and useMemo would change list render behavior
      const groupChips = [{ id: 'all', label: catLabel('all') },
        ...(groupBy === 'type' ? TOOL_TYPE_GROUPS : TOOL_BUSINESS_GROUPS)
          .map(id => ({ id, label: groupBy === 'type' ? typeLabel(id) : catLabel(id) }))
          .filter(chip => listItems.some(tool => primaryGroupOf(tool) === chip.id))];
      const filteredTools = listItems.filter(tool => {
        // 即将上线占位卡(无 backendId)在「仅显示已安装」下隐藏；其余照常检索、进分区。
        if (!isLaunchedTool(tool) && installedOnly) return false;
        const q = searchQuery.toLowerCase();
        const matchesSearch = tool.title.toLowerCase().includes(q) || (tool.desc || '').toLowerCase().includes(q);
        const matchesCategory = searching || activeCategory === 'all' || primaryGroupOf(tool) === activeCategory;
        const matchesInstalled = !installedOnly || tool.installed;
        return matchesSearch && matchesCategory && matchesInstalled;
      }).sort((a, b) => {
        // 已上线(有 backendId 或内置)排在未上线(即将上线)之前
        const onA = !!a.backendId || !!a.builtin, onB = !!b.backendId || !!b.builtin;
        if (onA !== onB) return onA ? -1 : 1;
        if (a.installed && !b.installed) return -1;
        if (!a.installed && b.installed) return 1;
        return 0;
      });
      // 分区:非搜索即分区(仅显示已安装仍保持原分区视图,只是过滤到已装条目)。组内沿用 filteredTools 排序。
      const sectioned = !searching;
      // 即将上线占位卡(无 backendId)单拎到独立栏，不参与类型/业务分组。
      const upcomingTools = filteredTools.filter(t => !isLaunchedTool(t));
      const launchedTools = filteredTools.filter(t => isLaunchedTool(t));
      const listSections = [];
      if (sectioned) {
        const buckets = new Map();
        launchedTools.forEach(tool => {
          const key = sectionGroupOf(tool);
          if (!buckets.has(key)) buckets.set(key, []);
          buckets.get(key).push(tool);
        });
        sectionOrder.forEach(key => {
          if (buckets.has(key)) listSections.push({ id: key, label: sectionLabelOf(key), items: buckets.get(key) });
          buckets.delete(key);
        });
        buckets.forEach((items, key) => { listSections.push({ id: key, label: sectionLabelOf(key), items }); });
        if (upcomingTools.length) {
          listSections.push({ id: 'upcoming', label: typeLabel('upcoming'), items: upcomingTools });
        }
      }
      // 左侧二级分类快速导航 = 分区列表（含「即将上线」独立栏）。
      const navSections = sectioned ? listSections : [];
      const scrollToSection = (id) => {
        document.querySelector(`#store-section-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'start' });
      };
      useEffect(() => {
        if (!installedOnly && !searching && activeCategory !== 'all' && groupChips.every(chip => chip.id !== activeCategory)) {
          setActiveCategory('all');
        }
      }, [activeCategory, installedOnly, searching, groupChips]);

      const beginOAuthRequest = (backendId) => {
        // OAuth 请求关联 ID 需不可预测，避免用 Math.random()（CodeQL js/insecure-randomness）。
        const randomHex = Array.from(
          window.crypto.getRandomValues(new Uint8Array(8)),
          (b) => b.toString(16).padStart(2, '0'),
        ).join('');
        const requestId = `${Date.now()}-${randomHex}`;
        oauthRequestRef.current[backendId] = requestId;
        return requestId;
      };

      const isCurrentOAuthRequest = (backendId, requestId) => (
        !!requestId && oauthRequestRef.current[backendId] === requestId
      );

      const clearOAuthRequest = (backendId, requestId) => {
        if (isCurrentOAuthRequest(backendId, requestId)) {
          delete oauthRequestRef.current[backendId];
        }
      };

      useEffect(() => () => {
        const activeRequests = Object.entries(oauthRequestRef.current);
        oauthRequestRef.current = {};
        activeRequests.forEach(([toolId, requestId]) => {
          invokeTauri('cancel_marketplace_tool_oauth_login', { toolId, requestId })
            .catch(err => console.error('cancel marketplace oauth on unmount failed:', err));
        });
      }, []);

      const cancelOAuthLoading = async (activeAlert) => {
        const backendId = activeAlert?.toolId;
        const requestId = activeAlert?.requestId;
        if (!backendId || !isCurrentOAuthRequest(backendId, requestId)) return;
        setAlert(prev => ({
          ...prev,
          cancelable: false,
          subtitle: storeCopy.stoppingAuth,
        }));
        try {
          await invokeTauri('cancel_marketplace_tool_oauth_login', {
            toolId: backendId,
            requestId,
          });
          if (isCurrentOAuthRequest(backendId, requestId)) {
            const tool = findLocalizedTool(backendId);
            const name = tool ? tool.title : backendId;
            clearOAuthRequest(backendId, requestId);
            setBusyId(null);
            const outcome = resolveOAuthInstallOutcome(
              name,
              { status: 'cancelled', message: storeCopy.authWaitCancelled },
              {
                installed: true,
                mcp_configured: true,
                oauth_required: true,
                oauth_token_present: false,
                status: 'config_installed_auth_pending',
              },
              storeCopy.oauthOutcome
            );
            setToolAuthStates(prev => ({ ...prev, [backendId]: outcome.authState }));
            setAlert({ ...outcome.alert, toolId: backendId });
            if (selectedTool && selectedTool.backendId === backendId) {
              setSelectedTool(prev => ({ ...prev, ...outcome.selectedToolPatch }));
            }
          }
        } catch (err) {
          console.error('cancel_marketplace_tool_oauth_login failed:', err);
          if (isCurrentOAuthRequest(backendId, requestId)) {
            setAlert(prev => ({
              ...prev,
              cancelable: true,
              subtitle: storeCopy.cancelFailed,
            }));
          }
        }
      };

      // 执行安装（已拿到 config 或无需 config）
      const doInstall = async (backendId, userConfig) => {
        if (!canMutateToolStore) return;
        const t = findLocalizedTool(backendId);
        if (!externalAuthAvailable && isRestrictedExternalAuthTool(t)) return;
        const name = t ? t.title : backendId;
        const hasConfig = Boolean(t?.configFields?.length);
        const hasPipDeps = !hasConfig; // 无 config 的本地工具可能有 pip deps
        const oauthServerName = t?.oauthMcp ? oauthServerNameForTool(t) : null;
        if (t?.oauthMcp && !oauthServerName) {
          setAlert({ visible: true, loading: false, title: storeCopy.oauthConfigError, subtitle: storeCopy.oauthNoServerName(name), isInstall: false, isError: true });
          return;
        }
        const oauthRequestId = t?.oauthMcp ? beginOAuthRequest(backendId) : null;
        setBusyId(backendId);
        if (t?.oauthMcp) {
          setAlert({ loading: true, visible: false, title: storeCopy.connectingTool(name), subtitle: storeCopy.writingMcpConfig, isInstall: true, isError: false, cancelable: false, toolId: backendId, requestId: oauthRequestId });
        } else if (hasConfig) {
          setAlert({ loading: true, visible: false, title: storeCopy.connectingTool(name), subtitle: storeCopy.validatingApiKey, isInstall: true, isError: false });
        } else if (hasPipDeps) {
          setAlert({ loading: true, visible: false, title: storeCopy.installingTool(name), subtitle: storeCopy.downloadingDeps, isInstall: true, isError: false });
        }
        try {
          const args = { toolId: backendId };
          if (userConfig && Object.keys(userConfig).length > 0) {
            args.config = userConfig;
          }
          await invokeTauri('install_marketplace_tool', args);
          if (t?.oauthMcp) {
            if (!isCurrentOAuthRequest(backendId, oauthRequestId)) return;
            setToolAuthStates(prev => ({
              ...prev,
              [backendId]: {
                installed: true,
                mcp_configured: true,
                oauth_required: true,
                oauth_token_present: false,
                status: 'auth_in_progress',
                message: storeCopy.waitingBrowserAuth,
              },
            }));
            const loginPromise = invokeTauri('start_marketplace_tool_oauth_login', { toolId: backendId, requestId: oauthRequestId })
              .catch(err => ({
                status: 'failed',
                message: String(err).slice(0, 240),
                server_name: oauthServerName,
              }));
            setAlert({
              loading: true,
              visible: false,
              title: storeCopy.connectingTool(name),
              subtitle: storeCopy.browserOpenedWaiting,
              isInstall: true,
              isError: false,
              cancelable: true,
              toolId: backendId,
              requestId: oauthRequestId,
            });
            const loginResult = await withUiTimeout(
              loginPromise,
              OAUTH_UI_TIMEOUT_MS,
              { ...oauthUiTimeoutResult(oauthServerName), message: storeCopy.oauthBrowserTimeout }
            );
            if (!isCurrentOAuthRequest(backendId, oauthRequestId)) return;
            if (loginResult?.status === 'timeout') {
              await invokeTauri('cancel_marketplace_tool_oauth_login', { toolId: backendId, requestId: oauthRequestId })
                .catch(err => console.error('cancel marketplace oauth after UI timeout failed:', err));
            }
            if (!isCurrentOAuthRequest(backendId, oauthRequestId)) return;
            const authStatus = await invokeTauri('get_marketplace_tool_auth_status', { toolId: backendId })
              .catch((err) => {
                console.error('get_marketplace_tool_auth_status after oauth failed:', err);
                return null;
              });
            if (!isCurrentOAuthRequest(backendId, oauthRequestId)) return;
            await loadBackendState();
            if (!isCurrentOAuthRequest(backendId, oauthRequestId)) return;

            const outcome = resolveOAuthInstallOutcome(name, loginResult, authStatus, storeCopy.oauthOutcome);
            if (!outcome.connected) {
              setToolAuthStates(prev => ({ ...prev, [backendId]: outcome.authState }));
              setAlert(outcome.alert);
              if (selectedTool && selectedTool.backendId === backendId) {
                setSelectedTool(prev => ({ ...prev, ...outcome.selectedToolPatch }));
              }
              return;
            }

            setToolAuthStates(prev => ({ ...prev, [backendId]: outcome.authState }));
            setAlert({ ...outcome.alert, toolId: backendId });
            if (selectedTool && selectedTool.backendId === backendId) {
              setSelectedTool(prev => ({ ...prev, ...outcome.selectedToolPatch }));
            }
            notifyComposerToolsChanged();
            return;
          }
          await loadBackendState();
          setAlert({
            visible: true,
            loading: false,
            title: hasConfig ? storeCopy.connectedQuoted(name) : storeCopy.installedQuoted(name),
            isInstall: true,
            isError: false,
            toolId: backendId,
          });
          if (selectedTool && selectedTool.backendId === backendId) {
            setSelectedTool(prev => ({ ...prev, installed: true }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          if (t?.oauthMcp && !isCurrentOAuthRequest(backendId, oauthRequestId)) return;
          console.error('install failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, subtitle: String(e && e.message ? e.message : e).slice(0, 240), isInstall: false, isError: true });
        } finally {
          if (t?.oauthMcp) {
            if (isCurrentOAuthRequest(backendId, oauthRequestId)) {
              clearOAuthRequest(backendId, oauthRequestId);
              setBusyId(null);
            }
          } else {
            setBusyId(null);
          }
        }
      };

      // 技能安装/卸载(无 configFields,直接装/卸)
      const handleSkillAction = async (backendId, isInstalled) => {
        if (!canMutateToolStore) return;
        const t = skillCards.find(x => x.backendId === backendId);
        const name = t ? t.title : backendId;
        setBusyId(backendId);
        try {
          const cmd = isInstalled ? 'uninstall_marketplace_skill' : 'install_marketplace_skill';
          await invokeTauri(cmd, { skillId: backendId });
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: isInstalled ? storeCopy.uninstalledQuoted(name) : storeCopy.installedQuoted(name), isInstall: !isInstalled, isError: false });
          if (selectedTool && selectedTool.backendId === backendId) {
            setSelectedTool(prev => ({ ...prev, installed: !isInstalled }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('skill action failed:', e);
          setAlert({ visible: true, loading: false, title: storeCopy.operationFailedWith(String(e)), isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 预置技能更新:先弹二次确认(覆盖式更新会丢本地修改),确认后调
      // update_marketplace_skill 复用原子覆盖管线;保留技能启用状态(后端不重置 scope)。
      const [updateConfirm, setUpdateConfirm] = useState(null); // { backendId, skillId, name }
      const handleSkillUpdate = (backendId) => {
        if (!canMutateToolStore) return;
        // ima 连接器卡的更新落在其认领的 ima-skills 上(该技能不单独成卡)
        const skillId = backendId === 'ima' ? 'ima-skills' : backendId;
        const card = skillCards.find(x => x.backendId === backendId) || tools.find(x => x.backendId === backendId);
        setUpdateConfirm({ backendId, skillId, name: card ? card.title : skillId });
      };
      const doSkillUpdate = async () => {
        if (!updateConfirm) return;
        const { backendId, skillId, name } = updateConfirm;
        setUpdateConfirm(null);
        setBusyId(backendId);
        try {
          await invokeTauri('update_marketplace_skill', { skillId });
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: storeCopy.updatedQuoted(name), isInstall: true, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('skill update failed:', e);
          setAlert({ visible: true, loading: false, title: storeCopy.operationFailedWith(String(e)), isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 上传包展示名/说明编辑：后端动作下发 edit_display（actions.rs，source=Upload
      // 的已装包），点击弹编辑对话框；预填当前覆盖值（bundle 事实的 display_name/
      // display_description 原值），留空 = 清覆盖回退默认。保存调
      // update_bundle_display_meta 后按现有模式 loadBackendState 刷新。
      const [editDisplay, setEditDisplay] = useState(null); // { backendId, cardTitle, name, description }
      editDisplayRef.current = editDisplay; // 拖放 canAccept 经 ref 读最新弹窗态
      // 预填口径（与导入弹窗不同，是有意的）：编辑入口预填 extra 覆盖**原值**
      // （未设 = 空，留空保存即清覆盖回退默认）；导入入口预填**生效默认名**
      // （回退链取值），让用户直接保存固定默认或改名——见 doImportSkillZip。
      const handleEditDisplay = (backendId) => {
        if (!canMutateToolStore) return;
        const bf = (bundleStates[backendId] || {}).bundle || null;
        const card = skillCards.find(x => x.backendId === backendId) || tools.find(x => x.backendId === backendId);
        setEditDisplay({
          backendId,
          cardTitle: card ? card.title : backendId,
          name: (bf && bf.display_name) || '',
          description: (bf && bf.display_description) || '',
        });
      };
      // 返回值约定：成功 → null（调用方关弹窗）；失败 → 错误文案（弹窗保留
      // 输入内联展示，用户改完可直接重存，不必重新输入）。
      const doEditDisplaySave = async (values) => {
        const dlg = editDisplay;
        if (!dlg) return null;
        // 拖放导入进行中（busyId 槽位被 '__upload__' 占用）不得保存：busyId
        // 唯一，此处 setBusyId 会覆盖导入态、finally 提前放开拖放闸，成功后
        // 的 setEditDisplay(null) 还会误关导入刚自动弹出的预填对话框。返回
        // 错误文案让弹窗保留输入，导入完成后再保存。
        if (busyRef.current) return storeCopy.importingSkill;
        setBusyId(dlg.backendId);
        try {
          await invokeTauri('update_bundle_display_meta', {
            id: dlg.backendId,
            displayName: values.name,
            displayDescription: values.description,
          });
          setEditDisplay(null);
          await loadBackendState();
          // 展示名进了 list_marketplace_tools（composer 菜单数据源），
          // 与其它安装/卸载动作同款通知 composer 刷新，否则菜单滞留旧名。
          notifyComposerToolsChanged();
          // 详情弹窗持的是卡对象快照，loadBackendState 只刷新列表数据源；
          // 打开中的详情须单独补拉生效值（覆盖后的 name/description）。
          if (selectedTool && selectedTool.backendId === dlg.backendId) {
            try {
              const bs = await invokeTauri('bundle_readiness', { bundleId: dlg.backendId });
              const b = (bs && bs.bundle) || null;
              if (b) setSelectedTool(prev => ({ ...prev, title: b.name || prev.title, desc: b.description == null ? prev.desc : b.description }));
            } catch (err) {
              console.error('bundle_readiness refresh failed:', dlg.backendId, err);
            }
          }
          setAlert({ visible: true, loading: false, title: storeCopy.editDisplaySaved, isInstall: true, isError: false });
          return null;
        } catch (e) {
          console.error('update bundle display meta failed:', e);
          return String(e);
        } finally {
          setBusyId(null);
        }
      };

      // 上传 zip 技能包:按钮走 Rust 原生 dialog,拖放走 base64 字节通道,
      // 成功/取消/失败/loading 处理统一在这里。导入命令返回新包 id（None/null=
      // 用户取消）；成功后立即打开展示信息编辑弹窗，名称预填当前生效默认名
      // （extra 覆盖 > 上传文件名/manifest 回退），用户可直接保存或改名，
      // 取消则不设覆盖、保留默认展示。
      const doImportSkillZip = async (invokeFn) => {
        if (!canMutateToolStore) return;
        setBusyId('__upload__');
        setAlert({ loading: true, visible: false, title: storeCopy.importingSkill, subtitle: storeCopy.validatingSkillPackage, isInstall: true, isError: false });
        try {
          const newId = await invokeFn();
          if (newId) {
            await loadBackendState();
            setAlert({ visible: false, loading: false, title: '', isInstall: false, isError: false });
            // 预填默认名取后端生效值（bundle_readiness 已应用覆盖/回退口径）；
            // 拉取失败退化为 id 预填，弹窗照开。
            let bf = null;
            try {
              const bs = await invokeTauri('bundle_readiness', { bundleId: newId });
              bf = (bs && bs.bundle) || null;
            } catch (err) {
              console.error('bundle_readiness after import failed:', newId, err);
            }
            setEditDisplay({
              backendId: newId,
              cardTitle: (bf && bf.name) || newId,
              name: (bf && (bf.display_name || bf.name)) || newId,
              description: (bf && bf.display_description) || '',
            });
          } else {
            setAlert({ visible: false, loading: false, title: '', isInstall: false, isError: false }); // 用户取消
          }
        } catch (e) {
          console.error('import skill failed:', e);
          setAlert({ visible: true, loading: false, title: storeCopy.importFailedWith(String(e)), isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };
      const handleUploadSkill = () => doImportSkillZip(() => invokeTauri('import_plugin_package_cmd'));
      const handleZipDrop = (files) => {
        const picked = pickSkillDrop(files);
        if (!picked) return Promise.resolve();
        const { file, kind } = picked;
        if (file.size > MAX_SKILL_ZIP_BYTES) {
          setAlert({ visible: true, loading: false, title: storeCopy.importFailedWith(storeCopy.zipTooLarge(MAX_SKILL_ZIP_BYTES / 1024 / 1024)), isInstall: false, isError: true });
          return Promise.resolve();
        }
        // 单 .md 技能文件走 import_skill_md_bytes；zip 插件包走统一导入字节通道。
        const command = kind === 'md' ? 'import_skill_md_bytes' : 'import_plugin_package_bytes_cmd';
        return doImportSkillZip(async () =>
          invokeTauri(command, { filename: file.name, dataBase64: await fileToBase64(file) }));
      };

      const connectIma = async (values = {}) => {
        if (!canMutateToolStore) return;
        const clientId = (values.IMA_CLIENT_ID || '').trim();
        const apiKey = (values.IMA_API_KEY || '').trim();
        setBusyId('ima');
        setAlert({ loading: true, visible: false, title: detailCopy.actions.connectingIma, subtitle: detailCopy.actions.validatingIma, isInstall: true, isError: false });
        try {
          await invokeTauri('ima_connect', { clientId, apiKey });
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: detailCopy.actions.connectedIma, subtitle: detailCopy.actions.imaEnabled, isInstall: true, isError: false, toolId: 'ima' });
          if (selectedTool && selectedTool.backendId === 'ima') {
            setSelectedTool(prev => ({ ...prev, installed: true }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('ima connect failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.imaFailed, subtitle: detailCopy.actions.operationFailed, isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      const disconnectIma = async () => {
        if (!canMutateToolStore) return;
        setBusyId('ima');
        try {
          await invokeTauri('ima_logout');
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: detailCopy.actions.disconnectedIma, isInstall: false, isError: false });
          if (selectedTool && selectedTool.backendId === 'ima') {
            setSelectedTool(prev => ({ ...prev, installed: false }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('ima logout failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, subtitle: detailCopy.actions.operationFailed, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接飞书(config init --new 自建 app,两段扫码):事件驱动。
      // 进度走后端事件 feishu:qr / feishu:phase / feishu:connected / feishu:error
      //(监听见下方 useEffect);这里只 ensure cli + 触发 begin。busyId 在事件里清。
      const connectFeishu = async () => {
        setBusyId('feishu');
        ensureFeishuListeners(storeCopy);
        // 开流程卡（无阻塞弹窗）：先起“准备运行时”步。写进跨视图 store，切走不丢。
        feishuConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        // 客户端秒表 + 爬行条：后端 feishu:progress 有真实 pct 时会覆盖；没有也不至于像卡死。
        feishuConn.startTick();
        try {
          // ① 确保 CLI（首次使用在线安装）
          feishuConn.setFlow(f => ({ ...f, active: 'cli', pct: 0, log: detailCopy.flow.installStarting, steps: { ...(f && f.steps), runtime: 'done', cli: 'active' } }));
          await invokeTauri('feishu_ensure_cli');
          feishuConn.setFlow(f => ({ ...f, active: 'connect', pct: 100, steps: { ...(f && f.steps), cli: 'done', connect: 'active' } }));
          // ② 连接编排（后端 emit feishu:qr / connected / error）
          await invokeTauri('feishu_connect_begin');
        } catch (e) {
          console.error('feishu connect failed:', e);
          feishuConn.stopTick();
          setBusyId(null);
          feishuConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } };
          });
        }
      };
      // 取消/关闭流程卡：置取消 + kill 子进程 + 清状态。
      const feishuResetFlow = () => {
        feishuConn.stopTick();
        invokeTauri('feishu_cancel').catch(() => {});
        feishuConn.setFlow(null); setBusyId(null);
      };
      // 重试：ensure_cli 幂等，直接重跑整个连接流程。
      const feishuRetry = () => { connectFeishu(); };
      const disconnectFeishu = async () => {
        setBusyId('feishu');
        try {
          await invokeTauri('feishu_logout');
          // 断开 → 撤掉技能(should_show 变 false)+ 广播刷新；连接态经 readiness 重取。
          await invokeTauri('feishu_apply_skills').catch(() => {});
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: storeCopy.disconnectedTool(storeCopy.toolNames.feishu), isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('feishu logout failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接企业微信(单段扫码):流程卡驱动(镜像飞书),进度走 wecom:* 事件。
      const connectWecom = async () => {
        setBusyId('wecom');
        ensureWecomListeners(storeCopy);
        // 开流程卡(无阻塞弹窗):先起"准备运行时"步,写进跨视图 store,切走不丢。
        wecomConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        wecomConn.startTick();
        try {
          // ① 确保 CLI(首次联网装 wecom-cli ~40s)
          wecomConn.setFlow(f => ({ ...f, active: 'cli', pct: 0, log: detailCopy.flow.installStarting, steps: { ...(f && f.steps), runtime: 'done', cli: 'active' } }));
          await invokeTauri('wecom_ensure_cli');
          wecomConn.setFlow(f => ({ ...f, pct: 100, steps: { ...(f && f.steps), cli: 'done' } }));
          // ② 连接编排(后端 emit wecom:qr / connected / error)
          await invokeTauri('wecom_connect_begin');
        } catch (e) {
          console.error('wecom connect failed:', e);
          wecomConn.stopTick();
          setBusyId(null);
          wecomConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } };
          });
        }
      };
      const wecomResetFlow = () => {
        wecomConn.stopTick();
        invokeTauri('wecom_cancel').catch(() => {});
        wecomConn.setFlow(null); setBusyId(null);
      };
      const wecomRetry = () => { connectWecom(); };
      const disconnectWecom = async () => {
        setBusyId('wecom');
        try {
          await invokeTauri('wecom_logout');
          // 断开 → 撤掉技能(should_show 变 false)；连接态经 readiness 重取。
          await invokeTauri('wecom_apply_skills').catch(() => {});
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: storeCopy.disconnectedTool(storeCopy.toolNames.wecom), isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('wecom logout failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接钉钉(单段扫码):流程卡驱动(镜像企微),进度走 dingtalk:* 事件。
      const connectDingtalk = async () => {
        setBusyId('dingtalk');
        ensureDingtalkListeners(storeCopy);
        dingtalkConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        dingtalkConn.startTick();
        try {
          dingtalkConn.setFlow(f => ({ ...f, active: 'cli', pct: 0, log: detailCopy.flow.installStarting, steps: { ...(f && f.steps), runtime: 'done', cli: 'active' } }));
          await invokeTauri('dingtalk_ensure_cli');
          dingtalkConn.setFlow(f => ({ ...f, pct: 100, steps: { ...(f && f.steps), cli: 'done' } }));
          await invokeTauri('dingtalk_connect_begin');
        } catch (e) {
          console.error('dingtalk connect failed:', e);
          dingtalkConn.stopTick();
          setBusyId(null);
          dingtalkConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } };
          });
        }
      };
      const dingtalkResetFlow = () => {
        dingtalkConn.stopTick();
        invokeTauri('dingtalk_cancel').catch(() => {});
        dingtalkConn.setFlow(null); setBusyId(null);
      };
      const dingtalkRetry = () => { connectDingtalk(); };
      const disconnectDingtalk = async () => {
        setBusyId('dingtalk');
        try {
          await invokeTauri('dingtalk_logout');
          await invokeTauri('dingtalk_apply_skills').catch(() => {});
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: storeCopy.disconnectedTool(storeCopy.toolNames.dingtalk), isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('dingtalk logout failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接腾讯会议(单段 OAuth 授权):流程卡驱动(镜像钉钉),进度走 tmeet:* 事件。
      const connectTmeet = async () => {
        setBusyId('tmeet');
        ensureTmeetListeners(storeCopy);
        tmeetConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        tmeetConn.startTick();
        try {
          tmeetConn.setFlow(f => ({ ...f, active: 'cli', pct: 0, log: detailCopy.flow.installStarting, steps: { ...(f && f.steps), runtime: 'done', cli: 'active' } }));
          await invokeTauri('tmeet_ensure_cli');
          tmeetConn.setFlow(f => ({ ...f, pct: 100, steps: { ...(f && f.steps), cli: 'done' } }));
          await invokeTauri('tmeet_connect_begin');
        } catch (e) {
          console.error('tmeet connect failed:', e);
          tmeetConn.stopTick();
          setBusyId(null);
          tmeetConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...(f && f.steps), [step]: 'error' } };
          });
        }
      };
      const tmeetResetFlow = () => {
        tmeetConn.stopTick();
        invokeTauri('tmeet_cancel').catch(() => {});
        tmeetConn.setFlow(null); setBusyId(null);
      };
      const tmeetRetry = () => { connectTmeet(); };
      const disconnectTmeet = async () => {
        setBusyId('tmeet');
        try {
          await invokeTauri('tmeet_logout');
          await invokeTauri('tmeet_apply_skills').catch(() => {});
          await loadBackendState();
          setAlert({ visible: true, loading: false, title: detailCopy.actions.disconnectedTmeet, isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('tmeet logout failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 安装/卸载入口
      const handleAction = async (backendId, isInstalled) => {
        if (!canMutateToolStore) return;
        // 有配套 MCP 的技能(公文=gongwen、PPT=pptx,manifest companion_skills 声明)：
        // - 安装 → 始终改走所属 MCP 安装（组合包语义：装 MCP 联动装 companion 技能）;
        // - 卸载 → MCP 已装走包级卸载（companion 随包联动删除）;MCP 未装但技能
        //   独立已装（历史遗留/单独安装）走技能级卸载（后端按实际物理位置删除,G3）;
        // 纯技能(无配套 MCP:如 visualizer、上传技能)才走 handleSkillAction。
        // 用 skillCards(含后端合成卡与 userUploaded 卡)而非静态 tsSkillsData 判定——
        // 静态卡已删除且上传技能不在静态表里,漏判会落到通用工具分支报「未知工具」。
        const companionMcpId = skillToMcp[backendId];
        if (companionMcpId) {
          const mcpBs = bundleStates[companionMcpId];
          const mcpInstalled = mcpBs ? mcpBs.installed : !!toolStates[companionMcpId];
          if (isInstalled && !mcpInstalled) return handleSkillAction(backendId, isInstalled);
          backendId = companionMcpId;
        }
        else if (skillCards.some(s => s.backendId === backendId) && tsToolsData.every(t => t.backendId !== backendId)) return handleSkillAction(backendId, isInstalled);
        const requestedTool = findLocalizedTool(backendId);
        if (!externalAuthAvailable && isRestrictedExternalAuthTool(requestedTool)) return;
        // 飞书走 CLI 连接流程,不走 marketplace install
        if (backendId === 'feishu') {
          if (isInstalled) return disconnectFeishu();
          // 未连接 → 弹详情弹窗（里面有进度卡）+ 触发 config init --new(浏览器自动建 app + 两段扫码,不收表单)
          const ft = tools.find(x => x.feishuCli) || localizeTool(tsToolsData.find(x => x.backendId === 'feishu'), t);
          if (ft) setSelectedTool(ft);
          return connectFeishu();
        }
        // 企微同走 CLI 连接流程(单段扫码)
        if (backendId === 'wecom') {
          if (isInstalled) return disconnectWecom();
          // 打开详情弹窗(里面有流程卡)+ 触发连接
          const wt = tools.find(x => x.wecomCli) || localizeTool(tsToolsData.find(x => x.backendId === 'wecom'), t);
          if (wt) setSelectedTool(wt);
          return connectWecom();
        }
        // 钉钉同走 CLI 连接流程(单段扫码)
        if (backendId === 'dingtalk') {
          if (isInstalled) return disconnectDingtalk();
          const dt = tools.find(x => x.dingtalkCli) || localizeTool(tsToolsData.find(x => x.backendId === 'dingtalk'), t);
          if (dt) setSelectedTool(dt);
          return connectDingtalk();
        }
        // 腾讯会议同走 CLI 连接流程(单段 OAuth 授权)
        if (backendId === 'tmeet') {
          if (isInstalled) return disconnectTmeet();
          const tt = tools.find(x => x.tmeetCli) || localizeTool(tsToolsData.find(x => x.backendId === 'tmeet'), t);
          if (tt) setSelectedTool(tt);
          return connectTmeet();
        }
        // IMA 是 OpenAPI Skill 连接器:校验凭据 + 安装 skill,不写 mcp.json。
        if (backendId === 'ima') {
          if (isInstalled) return disconnectIma();
          const it = tools.find(x => x.backendId === 'ima') || localizeTool(tsToolsData.find(x => x.backendId === 'ima'), t);
          if (!it) return;
          setConfigDialog({
            backendId,
            name: it.title,
            fields: it.configFields || [],
            configTitle: it.configTitle,
            configDescription: it.configDescription,
            configDocUrl: it.configDocUrl,
            configDocLabel: it.configDocLabel,
          });
          return;
        }
        const tool = findLocalizedTool(backendId);
        // 组合包化的本地能力(pptx)只有 companion 技能卡、无连接器卡,名称回退到技能卡
        const name = tool ? tool.title : ((skillCards.find(x => x.backendId === backendId) || {}).title || backendId);

        // 安装：有 configFields 的工具先弹配置弹窗
        if (!isInstalled) {
          // Obsidian：连接前先探测本机状态——没装/没库就引导，不默默装个用不了的连接器
          if (backendId === 'obsidian') {
            let st = null;
            try { st = await invokeTauri('detect_obsidian'); } catch { /* silent: treat not-installed as a probe failure */ }
            if (st && st.state && st.state !== 'ok') { setObsidianGuide({ backendId, name, ...st }); return; }
            return doInstall(backendId, {});
          }
          if (tool?.configFields && tool.configFields.length > 0) {
            setConfigDialog({
              backendId,
              name,
              fields: tool.configFields,
              configTitle: tool.configTitle,
              configDescription: tool.configDescription,
              configDocUrl: tool.configDocUrl,
              configDocLabel: tool.configDocLabel,
            });
            return;
          }
          return doInstall(backendId, {});
        }

        // 卸载
        setBusyId(backendId);
        try {
          await invokeTauri('uninstall_marketplace_tool', { toolId: backendId });
          await loadBackendState();
          if (tool?.oauthMcp) {
            setToolAuthStates(prev => ({
              ...prev,
              [backendId]: {
                installed: false,
                mcp_configured: false,
                oauth_required: true,
                oauth_token_present: false,
                status: 'not_installed',
                message: storeCopy.notConnectedYet(name),
              },
            }));
          }
          setAlert({ visible: true, loading: false, title: storeCopy.uninstalledQuoted(name), isInstall: false, isError: false });
          if (selectedTool && selectedTool.backendId === backendId) {
            setSelectedTool(prev => ({ ...prev, installed: false, authStatus: 'not_installed', authMessage: '' }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('uninstall failed:', e);
          setAlert({ visible: true, loading: false, title: detailCopy.actions.operationFailed, isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      useEffect(() => {
        if (selectedTool) document.body.style.overflow = 'hidden';
        else document.body.style.overflow = 'unset';
        return () => { document.body.style.overflow = 'unset'; };
      }, [selectedTool]);

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 overflow-hidden antialiased selection:bg-blue-200 dark:selection:bg-blue-900">
          {createPortal(<TsAlert alert={alert} theme={theme} copy={storeCopy} onDismiss={() => setAlert(a => ({ ...a, visible: false }))} onCancelLoading={cancelOAuthLoading} onNewChat={() => { const tid = alert.toolId; setAlert(a => ({ ...a, visible: false })); if (onNewChat) onNewChat(tid); }} />, document.body)}
          {/* 拖放技能包 overlay:可接受拖放期间全屏提示(pointer-events-none 不挡点击) */}
          {dropActive && canMutateToolStore && (
            <div data-testid="tool-store-drop-overlay" className="fixed inset-0 z-[80] flex items-center justify-center pointer-events-none bg-blue-500/10">
              <div className="rounded-3xl border-2 border-dashed border-blue-500 bg-white/90 dark:bg-[#1C1C1E]/90 px-8 py-6 text-center shadow-2xl">
                <Upload size={28} className="mx-auto mb-3 text-blue-500" />
                <p className="text-[15px] font-semibold">{storeCopy.dropSkillZipHere}</p>
              </div>
            </div>
          )}
          {createPortal(<TsConfigDialog
            config={externalAuthAvailable ? configDialog : null}
            theme={theme}
            copy={storeCopy}
            onCancel={() => setConfigDialog(null)}
            onConfirm={(values) => { const bid = configDialog.backendId; setConfigDialog(null); if (bid === 'ima') connectIma(values); else doInstall(bid, values); }}
          />, document.body)}
          {createPortal(<TsObsidianGuide
            guide={obsidianGuide}
            theme={theme}
            copy={storeCopy}
            allowDownload={can('localModelSetup')}
            onCancel={() => setObsidianGuide(null)}
            onDownload={() => invokeTauri('open_external_url', { url: 'https://obsidian.md/' }).catch(() => {})}
            onRetry={async () => {
              let st = null;
              try { st = await invokeTauri('detect_obsidian'); } catch { /* silent: treat not-installed as a probe failure */ }
              if (st && st.state === 'ok') { const bid = obsidianGuide.backendId; setObsidianGuide(null); doInstall(bid, {}); }
              else setObsidianGuide(g => g ? { ...g, ...st } : g);
            }}
          />, document.body)}
          {/* 预置技能更新二次确认:覆盖为商店最新版本,本地修改会丢失(WebView2 下
              window.confirm 不弹,应用内自绘,风格对齐 TsAlert) */}
          {updateConfirm && createPortal((
            // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is covered by the dialog's cancel/confirm buttons
            // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container
            <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm" onClick={() => setUpdateConfirm(null)}>
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; keyboard events need no bubbling here */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container */}
              <div className="w-[300px] rounded-[20px] overflow-hidden shadow-2xl bg-white/95 backdrop-blur-xl dark:bg-[#2C2C2E]" onClick={e => e.stopPropagation()}>
                <div className="px-6 pt-6 pb-5 text-center">
                  <div className="text-[17px] font-semibold mb-1.5 text-slate-900 dark:text-white">{storeCopy.updateSkillTitle(updateConfirm.name)}</div>
                  <div className="text-[13px] leading-relaxed text-slate-500 dark:text-slate-400">{storeCopy.updateSkillOverwriteHint}</div>
                </div>
                <div className="border-t border-slate-200 dark:border-white/10 flex">
                  <button type="button" onClick={() => setUpdateConfirm(null)} className="flex-1 py-3 text-[17px] text-center transition-colors text-slate-500 active:bg-slate-100 dark:text-slate-400 dark:active:bg-white/5 border-r border-slate-200 dark:border-white/10">
                    {storeCopy.cancel}
                  </button>
                  <button type="button" onClick={doSkillUpdate} className="flex-1 py-3 text-[17px] font-semibold text-center transition-colors text-[#007AFF] active:bg-slate-100 dark:text-[#0A84FF] dark:active:bg-white/5">
                    {(t.uiToolCommon || {}).update}
                  </button>
                </div>
              </div>
            </div>
          ), document.body)}
          {/* 上传包展示名/说明编辑弹窗（edit_display 动作触发；条件挂载，state 初值即当前覆盖值）。
              key 按 backendId 强制重挂载：编辑目标切换（重开/自动预填）时不得复用旧包的
              useState 输入初值、保存却写进新包 id（串台）。弹窗开着时的拖放导入已在
              canAccept 处拒绝（editDisplayRef），原位替换路径不再可达。 */}
          {editDisplay && createPortal(<TsEditDisplayDialog
            key={editDisplay.backendId}
            dialog={editDisplay}
            copy={storeCopy}
            onCancel={() => setEditDisplay(null)}
            onConfirm={doEditDisplaySave}
          />, document.body)}
          {/* 飞书扫码二维码已内联进 FeishuFlowCard（详情弹窗内），不再单独浮层 */}
          {wecomQr && (() => {
            const cancel = () => { invokeTauri('wecom_cancel').catch(() => {}); setWecomQr(null); setBusyId(null); };
            return createPortal((
            // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is covered by the dialog's cancel control
            // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container
            <div className="fixed inset-0 z-[200] flex items-center justify-center p-4" style={{ backgroundColor: 'rgba(0,0,0,0.5)', WebkitBackdropFilter: 'blur(8px)', backdropFilter: 'blur(8px)' }} onClick={cancel}>
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; keyboard events need no bubbling here */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container */}
              <div className="bg-white dark:bg-[#1C1C1E] rounded-3xl p-7 w-full max-w-[440px] flex flex-col items-center text-center shadow-2xl" onClick={e => e.stopPropagation()}>
                <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-4">{storeCopy.connectTitle(storeCopy.toolNames.wecom)}</h3>
                {/* 文案精简(方案A):扫码指引交给内嵌页自己说，这里不重复。直接内嵌企微登录页
                    （其 JS 动态渲染真正的登录码）——避免把 gen 网页地址编码成二维码导致的二次扫码。 */}
                {wecomQr.url
                  ? <iframe src={wecomQr.url} title={storeCopy.loginFrameTitle(storeCopy.toolNames.wecom)} className="w-full h-[440px] rounded-2xl border border-slate-200 dark:border-white/10 bg-white" scrolling="no" />
                  : <div className="w-52 h-52 rounded-2xl border border-dashed border-slate-300 dark:border-white/10 flex items-center justify-center text-[12px] text-slate-400 px-4">{storeCopy.loginPageLoadFailed}</div>}
                <div className="flex items-center gap-1.5 mt-4 text-[13px] text-slate-500 dark:text-slate-400">
                  <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse"></span> {storeCopy.waitingAuth}
                </div>
                <button type="button" onClick={() => { if (wecomQr.url) invokeTauri('open_external_url', { url: wecomQr.url }); }} className="mt-4 text-[13px] text-blue-600 dark:text-blue-400 hover:underline">{storeCopy.openInBrowser}</button>
                <button type="button" onClick={cancel} className="mt-3 px-6 py-2 rounded-full text-[14px] font-semibold bg-slate-100 dark:bg-[#2C2C2E] text-slate-600 dark:text-slate-300">{storeCopy.cancel}</button>
              </div>
            </div>
            ), document.body);
          })()}
          <div className="flex-1 flex flex-col bg-white dark:bg-[#131314] text-slate-900 dark:text-white transition-colors duration-300 font-sans overflow-y-auto custom-scrollbar p-4 sm:p-6 lg:p-10">

            {/* Header */}
            <header className="z-30 bg-white/80 dark:bg-[#131314]/80 backdrop-blur-2xl transition-colors">
              <div className="max-w-[1400px] mx-auto border-b border-slate-200/50 pb-6 dark:border-white/10">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
                  <div className="flex items-center justify-between sm:block sm:shrink-0">
                    <h1 className="shrink-0 text-[26px] font-normal tracking-tight">{storeCopy.title}</h1>
                  </div>
                  <div className="flex min-w-0 flex-wrap items-center justify-end gap-3 sm:ml-8 sm:flex-1 sm:flex-nowrap">
                    <div className="relative group min-w-0 basis-full flex-1 sm:basis-auto sm:max-w-[520px]">
                      <Search className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[#8E8E93] group-focus-within:text-blue-500 transition-colors" size={18} />
                      <input
                        data-testid="tool-store-search"
                        type="text"
                        placeholder={storeCopy.search}
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                        className="h-9 w-full rounded-[14px] border-none bg-slate-100 pl-10 pr-4 text-[13px] font-normal outline-none transition-all placeholder:text-[#8E8E93] focus:ring-0 dark:bg-[rgba(118,118,128,.24)] text-slate-900 dark:text-white"
                      />
                    </div>
                    <div className="flex shrink-0 items-center justify-end gap-3">
                      <button type="button" data-testid="tool-store-guide" onClick={() => setShowGuide(true)} title={storeCopy.guide.title}
                        className="inline-flex h-9 items-center rounded-full bg-slate-100 px-4 text-[13px] font-semibold shadow-sm transition-colors hover:bg-slate-200 dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C]">
                        <BookOpen size={14} className="mr-2 opacity-70" />
                        <span>{storeCopy.guide.title}</span>
                      </button>
                      {canMutateToolStore && (
                        <button type="button" data-testid="tool-store-manage-visibility" onClick={() => setManagingVisibility(v => !v)} title={storeCopy.modeVisibilityHint}
                          className={`inline-flex h-9 items-center rounded-full px-4 text-[13px] font-semibold shadow-sm transition-colors ${managingVisibility ? 'bg-blue-600 text-white hover:bg-blue-700' : 'bg-slate-100 text-slate-700 hover:bg-slate-200 dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C]'}`}>
                          <Settings size={14} className="mr-2 opacity-70" />
                          <span>{managingVisibility ? storeCopy.doneManagingVisibility : storeCopy.manageVisibility}</span>
                        </button>
                      )}
                      {canMutateToolStore && (
                        <button type="button" data-testid="tool-store-upload-btn" onClick={handleUploadSkill} title={storeCopy.uploadSkillPackage} disabled={busyId === '__upload__'}
                          className="inline-flex h-9 items-center rounded-full bg-slate-100 px-4 text-[13px] font-semibold shadow-sm transition-colors hover:bg-slate-200 disabled:opacity-50 dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C]">
                          <Upload size={14} className="mr-2 opacity-70" />
                          <span>{storeCopy.uploadSkillPackage}</span>
                        </button>
                      )}
                    </div>
                  </div>
                </div>
              </div>
            </header>

            {/* Main scrollable area */}
            <main className="flex-1">
              <div className="max-w-[1400px] mx-auto pt-5 pb-8 flex gap-6 items-start">

                {/* 左侧二级分类快速导航（仅分区浏览态显示，点击跳转到对应分区） */}
                {navSections.length > 0 && (
                  <aside className="hidden lg:block w-40 shrink-0 sticky top-6">
                    <nav className="flex flex-col gap-1">
                      {navSections.map(s => (
                        <button type="button" key={s.id} onClick={() => scrollToSection(s.id)}
                          className="flex items-center justify-between gap-2 w-full px-3 py-2 rounded-xl text-left text-[13px] font-medium text-slate-600 hover:bg-slate-100 hover:text-slate-900 dark:text-slate-300 dark:hover:bg-white/5 dark:hover:text-white transition-colors">
                          <span className="truncate">{s.label}</span>
                          <span className="text-[11px] text-slate-400 dark:text-slate-500 tabular-nums shrink-0">{s.items.length}</span>
                        </button>
                      ))}
                    </nav>
                  </aside>
                )}

                {/* Category filter + tool list */}
                <section className="flex-1 min-w-0">
                  <div className={`flex flex-col gap-4 mb-6 pb-5 ${searching ? 'sm:flex-row sm:items-end justify-between' : ''}`}>
                    {searching && (
                      <div className="flex items-center gap-3">
                        <button type="button" onClick={() => setSearchQuery('')} title={storeCopy.back}
                          className="w-9 h-9 rounded-full bg-slate-100 dark:bg-white/10 hover:bg-slate-200 dark:hover:bg-white/20 flex items-center justify-center text-slate-600 dark:text-slate-300 transition-colors shrink-0">
                          <ChevronLeft size={20} />
                        </button>
                        <h2 className="text-[13px] font-bold uppercase tracking-wider text-[#3C3C43]/60 dark:text-[#EBEBF5]/60">
                          {storeCopy.results}
                        </h2>
                      </div>
                    )}
                    <div className="flex flex-col gap-3">
                        {/* 主维度切换:按类型 / 按业务,决定二级筛选集合;下方列表始终按另一维度分区 */}
                        <div className="flex h-9 shrink-0 items-center self-start rounded-full bg-slate-100 p-1 shadow-sm dark:bg-[#2C2C2E]">
                          {[{ key: 'type', label: storeCopy.groupByType }, { key: 'business', label: storeCopy.groupByBusiness }].map(seg => (
                            <button type="button" key={seg.key} onClick={() => { setGroupBy(seg.key); setActiveCategory('all'); }}
                              className={`inline-flex h-7 items-center rounded-full px-3 text-[13px] font-semibold transition-colors whitespace-nowrap ${
                                groupBy === seg.key
                                  ? 'bg-white text-slate-900 shadow-sm dark:bg-[#3A3A3C] dark:text-white'
                                  : 'text-slate-700 hover:bg-slate-200 dark:text-white dark:hover:bg-[#3A3A3C]'
                              }`}>
                              {seg.label}
                            </button>
                          ))}
                        </div>
                        <div className="flex gap-2 overflow-x-auto no-scrollbar scroll-smooth">
                          {groupChips.map((chip) => {
                            const isActive = activeCategory === chip.id;
                            return (
                              <button type="button"
                                key={chip.id}
                                onClick={() => { setActiveCategory(chip.id); }}
                                className={`h-9 whitespace-nowrap shrink-0 text-[13px] px-3.5 rounded-full font-semibold transition-colors ${isActive
                                  ? 'bg-[#3A3A3C] text-[#fff] dark:bg-[#fff] dark:text-[#000]'
                                  : 'bg-[#F2F2F7] text-[#000] dark:bg-[#2C2C2E] dark:text-[#fff]'}`}
                              >
                                {chip.label}
                              </button>
                            );
                          })}
                          <button type="button" data-testid="tool-store-installed-only" onClick={() => setInstalledOnly(v => !v)} title={storeCopy.installedOnly}
                            className={`ml-auto h-9 whitespace-nowrap shrink-0 inline-flex items-center rounded-full px-3.5 text-[13px] font-semibold transition-colors ${installedOnly
                              ? 'bg-blue-600 text-[#fff] hover:bg-blue-700'
                              : 'bg-[#F2F2F7] text-[#000] dark:bg-[#2C2C2E] dark:text-[#fff]'}`}>
                            <Check size={14} className="mr-1.5 opacity-70" />
                            <span>{storeCopy.installedOnly}</span>
                          </button>
                          <span className="shrink-0 hidden sm:flex items-center gap-1.5 text-[12px] text-slate-400 dark:text-slate-500 pl-1">
                            {storeCopy.guide.dragHintShort}
                            <button type="button" onClick={() => setShowGuide(true)} aria-label={storeCopy.guide.title} title={storeCopy.guide.title}
                              className="w-[18px] h-[18px] rounded-full bg-slate-200 dark:bg-white/10 text-slate-500 dark:text-slate-400 hover:bg-slate-300 dark:hover:bg-white/20 flex items-center justify-center text-[11px] font-bold leading-none">?</button>
                          </span>
                        </div>
                      </div>
                  </div>

                  {filteredTools.length > 0 ? (
                    <div key="tool-store-list-grid" className={sectioned ? 'pb-7 space-y-8' : 'grid grid-cols-1 lg:grid-cols-2 gap-4 pb-7'}>
                      {(sectioned ? listSections : [{ id: 'flat', label: null, items: filteredTools }]).map((section) => (
                        <div key={`section-${section.id}`} id={sectioned ? `store-section-${section.id}` : undefined} className="scroll-mt-24">
                          {section.label && (
                            <div className="flex items-baseline gap-2 mb-2 px-3">
                              <h3 className="text-[13px] font-bold uppercase tracking-wider text-[#3C3C43]/60 dark:text-[#EBEBF5]/60">{section.label}</h3>
                              <span className="text-[12px] font-semibold text-slate-400 dark:text-slate-500 tabular-nums">{section.items.length}</span>
                            </div>
                          )}
                          <div className={sectioned ? 'grid grid-cols-1 lg:grid-cols-2 gap-4' : 'contents'}>
                            {section.items.map((tool) => (
                              // biome-ignore lint/a11y/useKeyWithClickEvents: row click is a shortcut; the keyboard path is covered by the row's real buttons
                              // biome-ignore lint/a11y/noStaticElementInteractions: row click hot zone, not a standalone interactive control
                              <div
                                key={`list-${tool.id}`}
                                onClick={() => setSelectedTool(tool)}
                                className="group flex items-center gap-4 py-3 cursor-pointer px-3 border-b border-slate-100 dark:border-white/5 last:border-0"
                              >
                                <TsToolIcon tool={tool} className="h-16 w-16 flex-shrink-0 rounded-[16px] border border-black/5 shadow-sm transition-shadow group-hover:shadow dark:border-white/5" imageClassName="h-11 w-11" fallbackSize={30} />
                                <div className="flex-1 min-w-0 flex flex-col justify-center py-1">
                                  <h3 className="text-[17px] font-semibold text-slate-900 dark:text-white truncate tracking-tight">{tool.title}</h3>
                                  <p className="text-[13px] text-slate-500 dark:text-slate-400 truncate mt-0.5 font-medium">{tool.subtitle}</p>
                                  <div className="flex items-center gap-2 mt-1.5">
                                    <span className="text-[10px] font-semibold text-slate-400 dark:text-slate-500 bg-slate-100 dark:bg-slate-800 px-1.5 py-0.5 rounded uppercase tracking-wide">{tool.type}</span>
                                    {tool.internal ? (
                                      <span className="text-[10px] font-semibold text-sky-700 dark:text-sky-300 bg-sky-100 dark:bg-sky-500/15 px-1.5 py-0.5 rounded-full">{storeCopy.internalDirect}</span>
                                    ) : tool.authRequired && (
                                      <span className="text-[10px] text-amber-500/80 dark:text-amber-400/80 flex items-center gap-0.5">
                                        <Zap size={10} /> {storeCopy.keyRequired}
                                      </span>
                                    )}
                                  </div>
                                </div>
                                <div className="flex flex-col items-center justify-center gap-1.5 pl-2">
                                  {(() => {
                                    const cf = tool.feishuCli ? feishuFlow : tool.wecomCli ? wecomFlow : tool.dingtalkCli ? dingtalkFlow : tool.tmeetCli ? tmeetFlow : null;
                                    if (externalAuthAvailable && cf && (cf.phase === 'running' || cf.phase === 'qr')) {
                                      return <FeishuMini flow={cf} onClick={() => setSelectedTool(tool)} copy={storeCopy.mini} />;
                                    }
                                    // 管理可见性编辑态：卡片出现每个模式的勾选框，勾选 = 可见。
                                    if (managingVisibility) {
                                      return (
                                        // biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; the keyboard path is covered by the checkbox itself
                                        // biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container
                                        <div className="flex flex-col items-start gap-1" onClick={(e) => e.stopPropagation()}>
                                          {[{ key: 'plain', label: storeCopy.modePlain }, { key: 'code', label: storeCopy.modeCode }].map((m) => {
                                            // 无 backendId 的卡（占位卡/内置 s5）不参与可见性配置：禁用勾选；
                                            // 可见性读取未成功（visibilityLoaded=false）时同样禁用——handler 虽有
                                            // 早退，但可点而无反馈的勾选框会误导用户以为配置已生效（四轮评审）。
                                            const checkDisabled = !tool.backendId || !visibilityLoaded;
                                            // 读回比对与写入同口径：后端 hidden 集按包 id 返回，companion 卡先经
                                            // skillToMcp 映射为所属包 id；同时回退比对原始技能 id，兼容历史版本
                                            // 按独立技能 id 落库的条目（未装→装边界）（五轮评审）。
                                            const visPkgId = skillToMcp[tool.backendId] || tool.backendId;
                                            const hiddenSet = hiddenByMode[m.key] || new Set();
                                            const visible = !hiddenSet.has(visPkgId) && !hiddenSet.has(tool.backendId);
                                            return (
                                              <label key={m.key} className={`flex items-center gap-2 ${checkDisabled ? 'opacity-40 cursor-not-allowed' : 'cursor-pointer'}`}>
                                                <input
                                                  type="checkbox"
                                                  checked={visible}
                                                  disabled={checkDisabled}
                                                  onChange={() => toggleModeVisibility(tool.backendId, m.key, !visible)}
                                                  className="h-4 w-4 rounded border-slate-300 accent-blue-600"
                                                />
                                                <span className={`text-[12px] font-medium ${visible ? 'text-slate-700 dark:text-slate-200' : 'text-slate-400 dark:text-slate-500'}`}>{m.label}</span>
                                              </label>
                                            );
                                          })}
                                        </div>
                                      );
                                    }
                                    return <PlatformToolAction tool={tool} busy={busyId === tool.backendId} onAction={handleAction} onUpdate={handleSkillUpdate} onEditDisplay={handleEditDisplay} copy={storeCopy} t={t} />;
                                  })()}
                                </div>
                              </div>
                            ))}
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : (
                    <div className="py-24 text-center flex flex-col items-center">
                      <div className="w-16 h-16 mb-4 rounded-full bg-slate-100 dark:bg-slate-800 flex items-center justify-center text-slate-400">
                        <Server size={28} />
                      </div>
                      <h3 className="text-xl font-semibold text-slate-800 dark:text-slate-200 mb-2">{searching ? storeCopy.emptyNoMatch : (installedOnly ? storeCopy.emptyNoInstalled : storeCopy.emptyNoTools)}</h3>
                      <p className="text-slate-500 dark:text-slate-400">{searching ? storeCopy.emptyNoMatchHint : (installedOnly ? (canMutateToolStore ? storeCopy.emptyNoInstalledHint : storeCopy.emptyNoInstalledHintReadonly) : storeCopy.emptyNoToolsHint)}</p>
                      {!searching && !installedOnly && canMutateToolStore && (
                        <button type="button" data-testid="tool-store-empty-upload-btn" onClick={handleUploadSkill}
                          className="mt-5 inline-flex h-9 items-center rounded-full bg-blue-600 px-5 text-[13px] font-semibold text-white shadow-sm transition-colors hover:bg-blue-700">
                          <Upload size={14} className="mr-2" />{storeCopy.uploadSkillPackage}
                        </button>
                      )}
                    </div>
                  )}
                </section>

              </div>
            </main>
          </div>

          {/* 插件指南弹窗：拖入安装说明 + 插件包介绍 + 规范文档下载 */}
          {showGuide && createPortal((
            // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is covered by the dialog header close button
            // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container
            <div
              className="fixed inset-0 z-[90] flex items-center justify-center p-4 sm:p-6 bg-slate-900/40 dark:bg-black/60 backdrop-blur-md transition-all duration-300"
              onClick={() => setShowGuide(false)}
            >
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; keyboard events need no bubbling here */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container */}
              <div
                className="relative w-full max-w-2xl bg-white dark:bg-[#1C1C1E] rounded-[28px] shadow-2xl overflow-hidden flex flex-col max-h-[90vh] border border-slate-200/50 dark:border-white/10"
                onClick={(e) => e.stopPropagation()}
              >
                <div className="flex items-center justify-between px-6 py-4 border-b border-slate-200/50 dark:border-white/10">
                  <h2 className="text-[18px] font-semibold">{storeCopy.guide.title}</h2>
                  <button type="button" onClick={() => setShowGuide(false)} aria-label={storeCopy.guide.close}
                    className="w-8 h-8 rounded-full hover:bg-slate-100 dark:hover:bg-white/10 flex items-center justify-center text-slate-500 dark:text-slate-400">
                    <XIcon size={16} />
                  </button>
                </div>
                <div className="px-6 py-5 overflow-y-auto custom-scrollbar space-y-5">
                  <section>
                    <h3 className="text-[14px] font-semibold mb-1.5">{storeCopy.guide.dragTitle}</h3>
                    <p className="text-[13px] text-slate-600 dark:text-slate-300 leading-relaxed">{storeCopy.guide.dragDesc}</p>
                    <h4 className="text-[12px] font-semibold text-slate-400 dark:text-slate-500 mt-3 mb-1.5 uppercase tracking-wide">{storeCopy.guide.typesTitle}</h4>
                    <ul className="flex flex-wrap gap-1.5">
                      {storeCopy.guide.types.map(t => (
                        <li key={t} className="text-[12px] px-2.5 py-1 rounded-full bg-slate-100 dark:bg-white/10 text-slate-700 dark:text-slate-200">{t}</li>
                      ))}
                    </ul>
                    <p className="mt-2.5 text-[12px] text-blue-600 dark:text-blue-400 leading-relaxed">{storeCopy.guide.formatsNote}</p>
                  </section>
                  <section>
                    <h3 className="text-[14px] font-semibold mb-1.5">{storeCopy.guide.introTitle}</h3>
                    <p className="text-[13px] text-slate-600 dark:text-slate-300 leading-relaxed">{storeCopy.guide.introDesc}</p>
                  </section>
                  <section>
                    <h3 className="text-[14px] font-semibold mb-1.5">{storeCopy.guide.specTitle}</h3>
                    <p className="text-[13px] text-slate-600 dark:text-slate-300 leading-relaxed">{storeCopy.guide.specDesc}</p>
                    <button type="button" onClick={downloadSpec} data-testid="tool-store-download-spec"
                      className="mt-3 inline-flex h-9 items-center rounded-full bg-blue-600 px-4 text-[13px] font-semibold text-white shadow-sm transition-colors hover:bg-blue-700">
                      <Download size={14} className="mr-2" />{storeCopy.guide.downloadSpec}
                    </button>
                    <p className="mt-2 text-[11px] text-slate-400 dark:text-slate-500">{storeCopy.guide.downloadHint}</p>
                  </section>
                </div>
              </div>
            </div>
          ), document.body)}

          {/* Detail modal — portal 到 body：否则被主内容区 backdrop-blur 祖先造的包含块困住，
              fixed inset-0 只盖住右侧内容区、盖不到左侧栏。portal 后蒙层铺满整个视口。 */}
          {selectedTool && createPortal((
            // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close layer; the keyboard path is covered by the dialog header close button
            // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click-to-close layer, non-interactive container
            <div
              className="fixed inset-0 z-[90] flex items-center justify-center p-4 sm:p-6 bg-slate-900/40 dark:bg-black/60 backdrop-blur-md transition-all duration-300"
              onClick={() => setSelectedTool(null)}
            >
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-propagation stop layer; keyboard events need no bubbling here */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: click-propagation stop layer, non-interactive container */}
              <div
                className="ts-modal-in relative w-full max-w-2xl bg-white dark:bg-[#1C1C1E] rounded-[32px] shadow-2xl overflow-hidden flex flex-col max-h-[90vh] border border-slate-200/50 dark:border-white/10"
                onClick={(e) => e.stopPropagation()}
              >
                <div className="absolute top-0 right-0 w-full px-6 py-5 flex items-center justify-end z-20 pointer-events-none">
                  <button type="button"
                    onClick={() => setSelectedTool(null)}
                    className="pointer-events-auto w-8 h-8 flex items-center justify-center rounded-full bg-slate-100/80 dark:bg-black/50 backdrop-blur text-slate-500 dark:text-slate-300 hover:bg-slate-200 dark:hover:bg-black transition-colors"
                  >
                    <XIcon size={18} />
                  </button>
                </div>

                <div className="overflow-y-auto p-6 sm:p-10 no-scrollbar pt-12">
                  <div className="flex flex-col sm:flex-row items-start gap-6 sm:gap-8 mb-8">
                    <TsToolIcon tool={selectedTool} className="h-28 w-28 flex-shrink-0 rounded-[28px] border border-black/5 shadow-md sm:h-32 sm:w-32 sm:rounded-[32px] dark:border-white/5" imageClassName="h-20 w-20 sm:h-24 sm:w-24" fallbackSize={56} />
                    <div className="flex-1">
                      <h2 className="text-2xl sm:text-3xl font-extrabold text-slate-900 dark:text-white mb-2 tracking-tight">{selectedTool.title}</h2>
                      <p className="text-[17px] text-slate-500 dark:text-slate-400 mb-5 font-medium">{selectedTool.subtitle}</p>
                      <div className="flex flex-col items-end gap-1.5">
                        {(() => { const sf = selectedTool.feishuCli ? feishuFlow : selectedTool.wecomCli ? wecomFlow : selectedTool.dingtalkCli ? dingtalkFlow : selectedTool.tmeetCli ? tmeetFlow : null; return (externalAuthAvailable && sf && (sf.phase === 'running' || sf.phase === 'qr'))
                          ? <FeishuMini flow={sf} onClick={() => {}} copy={storeCopy.mini} />
                          : <PlatformToolAction tool={selectedTool} busy={busyId === selectedTool.backendId} onAction={handleAction} onUpdate={handleSkillUpdate} onEditDisplay={handleEditDisplay} size="lg" copy={storeCopy} t={t} />; })()}
                        {((selectedTool.feishuCli && !feishuConnected) || (selectedTool.wecomCli && !wecomConnected) || (selectedTool.dingtalkCli && !dingtalkConnected) || (selectedTool.tmeetCli && !tmeetConnected)) && <span className="text-[11px] text-slate-400">{storeCopy.firstUseOnlineInstall}</span>}
                      </div>
                    </div>
                  </div>

                  <div className="flex items-center justify-between py-5 mb-8 border-y border-slate-100 dark:border-white/5 overflow-x-auto no-scrollbar gap-8">
                    <div className="flex flex-col flex-shrink-0">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">{storeCopy.detailInterfaceType}</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.type}</span>
                      <span className="text-[12px] text-slate-400 mt-1 flex items-center gap-1"><Server size={12}/> {storeCopy.detailOfficialSupport}</span>
                    </div>
                    <div className="w-px h-12 bg-slate-200 dark:bg-slate-800 flex-shrink-0" />
                    <div className="flex flex-col flex-shrink-0">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">{storeCopy.detailVersion}</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.version}</span>
                      <span className="text-[12px] text-slate-400 mt-1">{storeCopy.detailStableRelease}</span>
                    </div>
                    <div className="w-px h-12 bg-slate-200 dark:bg-slate-800 flex-shrink-0" />
                    <div className="flex flex-col flex-shrink-0 pr-4">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">{storeCopy.detailLatency}</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.latency}</span>
                      <span className="text-[12px] text-slate-400 mt-1 flex items-center gap-1"><Globe size={12}/> {storeCopy.detailGlobalAccel}</span>
                    </div>
                  </div>

                  {externalAuthAvailable && selectedTool.feishuCli && feishuFlow && (
                    <FeishuFlowCard flow={feishuFlow} steps={storeCopy.feishuSteps} name={storeCopy.toolNames.feishu} copy={detailCopy.flow} onRetry={feishuRetry} onCancel={feishuResetFlow} />
                  )}
                  {externalAuthAvailable && selectedTool.wecomCli && wecomFlow && (
                    <FeishuFlowCard flow={wecomFlow} steps={storeCopy.wecomSteps} name={storeCopy.toolNames.wecom} copy={detailCopy.flow} twoStep={false} onRetry={wecomRetry} onCancel={wecomResetFlow} />
                  )}
                  {externalAuthAvailable && selectedTool.dingtalkCli && dingtalkFlow && (
                    <FeishuFlowCard flow={dingtalkFlow} steps={storeCopy.dingtalkSteps} name={storeCopy.toolNames.dingtalk} copy={detailCopy.flow} twoStep={false} onRetry={dingtalkRetry} onCancel={dingtalkResetFlow} />
                  )}
                  {externalAuthAvailable && selectedTool.tmeetCli && tmeetFlow && (
                    <FeishuFlowCard flow={tmeetFlow.phase === 'error' && !detailCopy.showRawErrors ? { ...tmeetFlow, err: detailCopy.actions.operationFailed } : tmeetFlow} steps={detailCopy.tmeetSteps} name={detailCopy.tools.tmeet.title} copy={detailCopy.flow} twoStep={false} browserAuth={!!tmeetFlow.browserAuth} onRetry={tmeetRetry} onCancel={tmeetResetFlow} />
                  )}
                  {selectedTool.feishuCli && feishuConnected && !feishuFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">{storeCopy.connectedBanner(storeCopy.toolNames.feishu)}</span>
                    </div>
                  )}
                  {selectedTool.wecomCli && wecomConnected && !wecomFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">{storeCopy.connectedBanner(storeCopy.toolNames.wecom)}</span>
                    </div>
                  )}
                  {selectedTool.dingtalkCli && dingtalkConnected && !dingtalkFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">{storeCopy.connectedBanner(storeCopy.toolNames.dingtalk)}</span>
                    </div>
                  )}
                  {selectedTool.tmeetCli && tmeetConnected && !tmeetFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">{storeCopy.connectedBanner(storeCopy.toolNames.tmeet)}</span>
                    </div>
                  )}
                  {selectedTool.imaOpenapi && imaConnected && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">{storeCopy.connectedBannerIma}</span>
                    </div>
                  )}

                  <div>
                    <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-4">{storeCopy.aboutTitle}</h3>
                    <div className="text-slate-600 dark:text-slate-300 leading-relaxed text-[15px] space-y-4 font-medium">
                      <p>{selectedTool.desc}</p>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          ), document.body)}
        </div>
      );
    };

    // ==========================================
    // Shared Components
    // ==========================================

export { FeishuStepIcon, FeishuBar, FeishuFlowCard, FeishuMini, feishuConn, ensureFeishuListeners, wecomConn, ensureWecomListeners, dingtalkConn, ensureDingtalkListeners, tmeetConn, ensureTmeetListeners, TsAlert, TsConfigDialog, TsEditDisplayDialog, TsObsidianGuide, ToolStoreView };
/* eslint-enable sonarjs/cognitive-complexity -- tool store main view;legacy view */
