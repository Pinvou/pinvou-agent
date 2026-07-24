import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Briefcase, ChevronLeft, ChevronRight, Cpu, Globe, IconGrid, IconList, Package, Search, Server, User, XIcon, Zap } from '../../components/icons.jsx';
import { resolveOAuthInstallOutcome } from './oauth-marketplace-logic.js';
import { notifyComposerToolsChanged } from './tool-events.js';
import { TsActionBtn, tsCategories, tsFeaturedCollections, tsSkillsData, tsToolsData } from './tool-common.jsx';
import { invokeTauri, isTauriAvailable, tauriEvents } from '../../platform/tauri/client.js';
import { can } from '../../shared/platform.js';

const OAUTH_UI_TIMEOUT_MS = 90_000;

const canStartExternalAuth = () => can('oauth') && can('externalAuth');

const isRestrictedExternalAuthTool = (tool) => !!tool && !!(
  tool.authRequired
  || tool.oauthMcp
  || tool.feishuCli
  || tool.wecomCli
  || tool.dingtalkCli
  || tool.eipCli
  || tool.zhidaoCli
);

const PlatformToolAction = (props) => {
  if (!can('toolStoreMutations')) {
    if (!props.tool?.installed) return null;
    const label = isRestrictedExternalAuthTool(props.tool) ? '已连接' : '已安装';
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
        已连接
      </span>
    );
  }
  return <TsActionBtn {...props} />;
};

const THIRD_PARTY_TOOL_LOGOS = {
  weather: 'assets/tool-icons/amap-user-v3.png',
  iwencai: 'assets/tool-icons/iwencai-user-v3.png',
  feishu: 'assets/tool-icons/wb-feishu.svg',
  wecom: 'assets/tool-icons/wecom-user.png',
  dingtalk: 'assets/tool-icons/dingtalk-user-v2.png',
  qcc: 'assets/tool-icons/qcc-user.png',
  'patsnap-search': 'assets/tool-icons/wb-patsnap-search.png',
  obsidian: 'assets/tool-icons/obsidian.ico',
  eip: 'assets/tool-icons/h3c-user-v2.png',
  zhidao: 'assets/tool-icons/h3c-user-v2.png',
  'yuandian-mcp': 'assets/tool-icons/wb-yuandian-mcp.svg',
  3: 'assets/tool-icons/wb-qq-mail.png',
  4: 'assets/tool-icons/wb-ima-mcp.png',
  5: 'assets/tool-icons/wb-lexiang.png',
  6: 'assets/tool-icons/wb-tencent-docs.png',
  8: 'assets/tool-icons/wecom-user.png',
  11: 'assets/tool-icons/wb-tapd.png',
  12: 'assets/tool-icons/wb-cnb-api.svg',
};

const FULL_TILE_LOGOS = new Set(['assets/tool-icons/amap-user-v3.png', 'assets/tool-icons/dingtalk-user-v2.png', 'assets/tool-icons/h3c-user-v2.png', 'assets/tool-icons/iwencai-user-v3.png', 'assets/tool-icons/qcc-user.png', 'assets/tool-icons/wb-yuandian-mcp.svg', 'assets/tool-icons/wecom-user.png']);
const CROPPED_TILE_LOGOS = new Set(['assets/tool-icons/wb-yuandian-mcp.svg']);

const TsToolIcon = ({ tool, className = '', imageClassName = 'h-8 w-8', fallbackSize = 30, fallbackStrokeWidth = 1.5, children }) => {
  const Icon = tool.icon;
  const isFullTileLogo = tool.logoSrc && FULL_TILE_LOGOS.has(tool.logoSrc);
  const cropTileLogo = tool.logoSrc && CROPPED_TILE_LOGOS.has(tool.logoSrc);
  return (
    <div className={`relative flex items-center justify-center overflow-hidden ${tool.logoSrc ? `${isFullTileLogo ? 'bg-transparent' : 'bg-white dark:bg-white'} text-slate-900` : `${tool.color} text-white`} ${className}`}>
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
  message: '未收到浏览器授权回调，请确认是否已完成授权，或稍后重新授权。',
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

const FEISHU_STEPS = [
      { key: 'runtime', label: '准备运行时', sub: '解压 Node 到 ~/.pinvou3' },
      { key: 'cli', label: '安装连接组件', sub: 'lark-cli · 首次约 40 秒' },
      { key: 'connect', label: '连接并授权', sub: '创建应用身份' },
      { key: 'qr', label: '扫码登录', sub: '飞书 App 扫一扫' },
    ];
    // 企业微信:纯扫码单段,无「创建应用身份」步骤(复用 runtime/cli/qr 的进度条逻辑)
    const WECOM_STEPS = [
      { key: 'runtime', label: '准备运行时', sub: '解压 Node 到 ~/.pinvou3' },
      { key: 'cli', label: '安装连接组件', sub: 'wecom-cli · 首次约 40 秒' },
      { key: 'qr', label: '扫码登录', sub: '企业微信 App 扫一扫' },
    ];
    const DINGTALK_STEPS = [
      { key: 'runtime', label: '准备运行时', sub: '解压 Node 到 ~/.pinvou3' },
      { key: 'cli', label: '安装连接组件', sub: 'dws · 首次约 40 秒' },
      { key: 'qr', label: '扫码登录', sub: '钉钉 App 扫一扫' },
    ];
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
    const FeishuFlowCard = ({ flow, onRetry, onCancel, name = '飞书', twoStep = true, browserAuth = false, steps = FEISHU_STEPS }) => {
      if (!flow) return null;
      const isErr = flow.phase === 'error';
      return (
        <div className="mb-8 rounded-2xl border border-slate-200 dark:border-white/10 bg-slate-50 dark:bg-white/5 overflow-hidden">
          <div className="flex items-center gap-3 px-5 pt-4 pb-2">
            <span className={`w-2 h-2 rounded-full ${isErr ? 'bg-rose-500' : 'bg-blue-500 animate-pulse'}`} />
            <span className="font-semibold text-[14px] text-slate-900 dark:text-slate-100">{isErr ? `${name}接入未完成` : (flow.phase === 'done' ? `已连接${name}` : `正在接入${name}`)}</span>
            <span className="flex-1" />
            {(flow.phase === 'running' || flow.phase === 'qr') && <button onClick={onCancel} className="text-[12px] text-slate-400 hover:text-slate-600 dark:hover:text-slate-200">取消</button>}
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
                    {active && s.key === 'runtime' && (<><FeishuBar pct={flow.pct} /><div className="text-[11px] text-slate-400 mt-1">解压中 {Math.round(flow.pct || 0)}%</div></>)}
                    {active && s.key === 'cli' && (<><FeishuBar pct={flow.pct} creep /><div className="flex items-center justify-between mt-1"><div className="text-[11px] text-slate-400 truncate max-w-[260px] font-mono">{flow.log || 'npm: starting…'}</div><div className="text-[11px] text-slate-400 tabular-nums">已 {flow.sec || 0}s</div></div></>)}
                    {!active && <div className="text-[11.5px] text-slate-400">{s.sub}</div>}
                  </div>
                </div>
              );
            })}
          </div>
          {flow.phase === 'qr' && (flow.qr || browserAuth) && (
            <div className="px-5 pb-5">
              <div className="flex items-center gap-5 p-4 rounded-xl bg-white dark:bg-black/30 border border-slate-200 dark:border-white/10">
                {browserAuth
                  ? <div className="w-36 h-36 rounded-xl bg-blue-50 dark:bg-blue-500/10 border border-blue-200 dark:border-blue-500/30 grid place-items-center text-blue-500 shrink-0 text-center text-[12px] px-3 leading-relaxed">已在浏览器<br/>打开登录页</div>
                  : <img src={flow.qr} alt={`${name}二维码`} className="w-36 h-36 rounded-xl border border-slate-200 bg-white shrink-0" />}
                <div>
                  {browserAuth ? (
                    <>
                      <div className="font-medium text-[14px] mb-1 text-slate-900 dark:text-slate-100">在浏览器里扫码登录{name}</div>
                      <div className="text-[12px] text-slate-500 dark:text-slate-400 mb-3">已自动打开{name}登录页 → 用手机{name} 扫<b>页面上的</b>二维码确认。没弹出就点下面重开。</div>
                    </>
                  ) : (
                    <>
                      <div className="font-medium text-[14px] mb-1 text-slate-900 dark:text-slate-100">{twoStep ? (flow.qrPhase === 'authorize' ? '第 2 步 / 共 2 步：扫码授权' : '第 1 步 / 共 2 步：扫码注册应用') : `扫码登录${name}`}</div>
                      <div className="text-[12px] text-slate-500 dark:text-slate-400 mb-3">用{name} App 扫一扫 → 确认</div>
                    </>
                  )}
                  {flow.userCode && (
                    <div className="mb-3 inline-flex flex-col gap-1 rounded-lg bg-slate-100 dark:bg-white/10 px-3 py-2">
                      <span className="text-[11px] text-slate-500 dark:text-slate-400">页面验证码</span>
                      <span className="font-mono text-[18px] font-bold tracking-wider text-slate-900 dark:text-white">{flow.userCode}</span>
                    </div>
                  )}
                  {flow.qrUrl && <button onClick={() => invokeTauri('open_external_url', { url: flow.qrUrl })} className="text-[13px] text-blue-600 dark:text-blue-400 hover:underline">{browserAuth ? '重新打开登录页 ↗' : '在浏览器打开 ↗'}</button>}
                </div>
              </div>
            </div>
          )}
          {isErr && (
            <div className="px-5 pb-5">
              <div className="rounded-xl border border-rose-200 dark:border-rose-500/30 bg-rose-50 dark:bg-rose-500/10 p-3">
                <div className="text-[13px] font-medium text-rose-700 dark:text-rose-300 mb-1.5">连接未完成</div>
                <pre className="text-[11.5px] leading-relaxed text-rose-800/80 dark:text-rose-200/70 whitespace-pre-wrap max-h-28 overflow-auto font-mono">{flow.err}</pre>
                <div className="flex gap-2 mt-3 justify-end">
                  <button onClick={onCancel} className="px-3 py-1.5 rounded-lg bg-slate-200 dark:bg-white/10 text-slate-700 dark:text-slate-100 text-[13px]">关闭</button>
                  <button onClick={onRetry} className="px-3 py-1.5 rounded-lg bg-blue-600 text-white text-[13px]">重试</button>
                </div>
              </div>
            </div>
          )}
        </div>
      );
    };
    // 商店列表行内的迷你进度（详情弹窗关掉后，后台仍在跑）
    const FeishuMini = ({ flow, onClick }) => {
      const label = flow.phase === 'qr' ? '待扫码'
        : (flow.active === 'cli' ? `装 ${Math.round(flow.pct || 0)}%`
        : (flow.active === 'runtime' ? `解压 ${Math.round(flow.pct || 0)}%` : '接入中'));
      return (
        <button onClick={(e) => { e.stopPropagation(); onClick(); }} title="点开查看进度" className="shrink-0 flex items-center gap-1.5 pl-1.5 pr-2.5 py-1.5 rounded-full bg-blue-50 dark:bg-blue-500/10 border border-blue-200 dark:border-blue-500/30 text-blue-600 dark:text-blue-300 text-[12px] font-medium">
          <span className="w-3 h-3 rounded-full border-2 border-blue-500 border-t-transparent animate-spin inline-block shrink-0" />
          <span className="tabular-nums whitespace-nowrap">{label}</span>
        </button>
      );
    };

    // ── 飞书连接流程 · 跨视图持久 store ──
    // ToolStoreView 随左栏切换会卸载；连接是长流程（装 CLI ~40s + 扫码），进度/监听/秒表
    // 若放组件 useState，一离开工具商店就全丢 → 回来按钮又变“连接”。故挂在模块级单例，
    // 活在组件生命周期之外；组件只订阅它做镜像渲染。
    const feishuConn = {
      flow: null,
      tick: null,
      listenersReady: false,
      subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch (_) {} }); },
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
    // 后端连接事件只注册一次（幂等，跨 ToolStoreView 多次挂载不重复注册）。
    function ensureFeishuListeners() {
      if (feishuConn.listenersReady) return;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      feishuConn.listenersReady = true;
      ev.listen('feishu:progress', (e) => {
        const p = e.payload || {};
        feishuConn.setFlow(f => {
          const nf = f ? { ...f, steps: { ...(f.steps || {}) } } : { phase: 'running', steps: {}, active: null, pct: 0, sec: 0, log: '' };
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
            ...(f || {}), phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), connect: 'done', qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase,
          };
        });
      });
      ev.listen('feishu:connected', () => {
        feishuConn.stopTick();
        feishuConn.setFlow(f => ({ ...(f || {}), phase: 'done', steps: { ...((f && f.steps) || {}), qr: 'done' } }));
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
          return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || '连接失败'), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } };
        });
      });
    }

    // ── 企业微信连接流程 · 跨视图持久 store(镜像 feishuConn;企微纯扫码单段）──
    const wecomConn = {
      flow: null, tick: null, listenersReady: false, subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch (_) {} }); },
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
    function ensureWecomListeners() {
      if (wecomConn.listenersReady) return;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      wecomConn.listenersReady = true;
      ev.listen('wecom:qr', (e) => {
        const p = e.payload || {};
        wecomConn.stopTick();
        wecomConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return { ...(f || {}), phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase };
        });
      });
      ev.listen('wecom:connected', () => {
        wecomConn.stopTick();
        wecomConn.setFlow(f => ({ ...(f || {}), phase: 'done', steps: { ...((f && f.steps) || {}), qr: 'done' } }));
        invokeTauri('wecom_apply_skills').catch(() => {});
        setTimeout(() => wecomConn.setFlow(null), 1800);
      });
      ev.listen('wecom:error', (e) => {
        const p = e.payload || {};
        wecomConn.stopTick();
        wecomConn.setFlow(f => { const step = (f && f.active) || 'cli'; return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || '连接失败'), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } }; });
      });
    }

    // ── 钉钉连接流程 · 跨视图持久 store(镜像企微;纯扫码单段）──
    const dingtalkConn = {
      flow: null, tick: null, listenersReady: false, subs: new Set(),
      subscribe(fn) { this.subs.add(fn); return () => { this.subs.delete(fn); }; },
      setFlow(u) { this.flow = (typeof u === 'function') ? u(this.flow) : u; this.subs.forEach(fn => { try { fn(this.flow); } catch (_) {} }); },
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
    function ensureDingtalkListeners() {
      if (dingtalkConn.listenersReady) return;
      const ev = isTauriAvailable() ? tauriEvents : null;
      if (!ev) return;
      dingtalkConn.listenersReady = true;
      ev.listen('dingtalk:qr', (e) => {
        const p = e.payload || {};
        dingtalkConn.stopTick();
        dingtalkConn.setFlow(f => {
          const prev = (f && f.steps) || {};
          return { ...(f || {}), phase: 'qr', active: 'qr',
            steps: { ...prev, runtime: prev.runtime || 'done', cli: prev.cli === 'active' ? 'done' : (prev.cli || 'done'), qr: 'active' },
            qr: p.qr_data_url, qrUrl: p.url, qrPhase: p.phase, userCode: p.user_code };
        });
      });
      ev.listen('dingtalk:connected', async () => {
        dingtalkConn.stopTick();
        try {
          await invokeTauri('dingtalk_apply_skills');
          dingtalkConn.setFlow(f => ({ ...(f || {}), phase: 'done', steps: { ...((f && f.steps) || {}), qr: 'done' } }));
          setTimeout(() => dingtalkConn.setFlow(null), 1800);
        } catch (e) {
          dingtalkConn.setFlow(f => ({ ...(f || {}), phase: 'error', err: `钉钉已授权，但技能启用失败：${String(e).slice(0, 220)}`, errStep: 'qr', steps: { ...((f && f.steps) || {}), qr: 'error' } }));
        }
      });
      ev.listen('dingtalk:error', (e) => {
        const p = e.payload || {};
        dingtalkConn.stopTick();
        dingtalkConn.setFlow(f => { const step = (f && f.active) || 'cli'; return { ...(f || { steps: {} }), phase: 'error', err: String(p.message || '连接失败'), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } }; });
      });
    }

    // iOS 风格弹窗（安装/卸载后提示需新建会话生效）
    const TsAlert = ({ alert, theme, onDismiss, onNewChat, onCancelLoading }) => {
      const isDark = theme === 'dark';
      if (!alert.visible && !alert.loading) return null;
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div
            className={`w-[280px] rounded-[20px] overflow-hidden shadow-2xl transition-transform duration-200 scale-100 ${
              isDark ? 'bg-[#2C2C2E]' : 'bg-white/95 backdrop-blur-xl'
            }`}
            style={{ animation: 'tsAlertIn .2s ease-out' }}
          >
            {alert.loading ? (
              <>
                <div className="px-6 py-8 text-center">
                  <div className="flex justify-center mb-4">
                    <div className={`w-6 h-6 rounded-full border-[2.5px] border-t-transparent ${isDark ? 'border-[#0A84FF]' : 'border-[#007AFF]'}`}
                      style={{ animation: 'tsSpinner .8s linear infinite' }} />
                  </div>
                  <div className={`text-[17px] font-semibold mb-1.5 ${isDark ? 'text-white' : 'text-slate-900'}`}>
                    {alert.title}
                  </div>
                  {alert.subtitle && (
                    <div className={`text-[13px] leading-relaxed ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>
                      {alert.subtitle}
                    </div>
                  )}
                </div>
                {alert.cancelable && (
                  <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
                    <button
                      onClick={() => onCancelLoading && onCancelLoading(alert)}
                      className={`w-full py-3 text-[17px] font-normal text-center transition-colors ${
                        isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100'
                      }`}
                    >
                      取消
                    </button>
                  </div>
                )}
              </>
            ) : (
              <>
                <div className="px-6 pt-6 pb-5 text-center">
                  <div className={`text-[17px] font-semibold mb-1.5 ${isDark ? 'text-white' : 'text-slate-900'}`}>
                    {alert.title}
                  </div>
                  {alert.subtitle ? (
                    <div className={`text-[13px] leading-relaxed ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>
                      {alert.subtitle}
                    </div>
                  ) : !alert.isError && (
                    <div className={`text-[13px] leading-relaxed ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>
                      {alert.isInstall ? '新工具需要在新会话中生效' : '已移除，新会话将不再加载该工具'}
                    </div>
                  )}
                </div>
                <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
                  <button
                    onClick={onDismiss}
                    className={`w-full py-3 text-[17px] font-normal text-center transition-colors ${
                      isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100'
                    }`}
                  >
                    知道了
                  </button>
                </div>
                {!alert.isError && (
                  <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
                    <button
                      onClick={onNewChat}
                      className={`w-full py-3 text-[17px] font-semibold text-center transition-colors ${
                        isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100'
                      }`}
                    >
                      新建会话
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
    const TsConfigDialog = ({ config, theme, onConfirm, onCancel }) => {
      const isDark = theme === 'dark';
      if (!config) return null;
      const [values, setValues] = useState({});
      const fields = config.fields || [];
      // required:false 的字段可留空；required:true 字段必须填写后才能连接。
      const canSubmit = fields.every(f => f.required === false || (values[f.key] || '').trim().length > 0);
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div
            className={`w-[300px] rounded-[20px] overflow-hidden shadow-2xl ${isDark ? 'bg-[#2C2C2E]' : 'bg-white/95 backdrop-blur-xl'}`}
            style={{ animation: 'tsAlertIn .2s ease-out' }}
          >
            <div className="px-6 pt-6 pb-4 text-center max-h-[70vh] overflow-y-auto">
              <div className={`text-[17px] font-semibold mb-3 ${isDark ? 'text-white' : 'text-slate-900'}`}>
                {config.configTitle || `配置「${config.name}」`}
              </div>
              {config.configDescription && (
                <div className={`text-[12px] leading-relaxed mb-3 ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>
                  {config.configDescription}
                </div>
              )}
              {config.configDocUrl && (
                <button
                  onClick={() => invokeTauri('open_external_url', { url: config.configDocUrl })}
                  className={`text-[13px] mb-4 inline-block ${isDark ? 'text-[#0A84FF]' : 'text-[#007AFF]'} hover:underline`}
                >
                  {config.configDocLabel || '查看配置说明'} →
                </button>
              )}
              {/* 引导链接放最上,不夹在输入框中间 */}
              {fields.find(f => f.helpUrl) && (
                <button
                  onClick={() => invokeTauri('open_external_url', { url: fields.find(f => f.helpUrl).helpUrl })}
                  className={`text-[13px] mb-4 inline-block ${isDark ? 'text-[#0A84FF]' : 'text-[#007AFF]'} hover:underline`}
                >
                  不会建应用？去飞书开放平台建一个 →
                </button>
              )}
              {/* 所有输入框紧挨着 */}
              {fields.map((field) => (
                <div key={field.key} className="text-left mb-3">
                  <label className={`text-[13px] font-medium mb-1.5 block ${isDark ? 'text-slate-300' : 'text-slate-600'}`}>
                    {field.label}
                  </label>
                  <input
                    type={field.secret ? 'password' : 'text'}
                    placeholder={field.placeholder || "sk-..."}
                    value={values[field.key] || ''}
                    onChange={e => setValues(v => ({ ...v, [field.key]: e.target.value }))}
                    className={`w-full px-3 py-2 rounded-lg text-[14px] outline-none transition-colors ${
                      isDark
                        ? 'bg-[#1C1C1E] border border-[#3A3A3C] text-white placeholder-slate-500 focus:border-[#0A84FF]'
                        : 'bg-slate-50 border border-slate-200 text-slate-900 placeholder-slate-400 focus:border-[#007AFF]'
                    }`}
                  />
                  {field.helpText && (
                    <div className={`text-[11px] mt-1 leading-snug ${isDark ? 'text-slate-500' : 'text-slate-400'}`}>
                      {field.helpText}
                    </div>
                  )}
                </div>
              ))}
            </div>
            <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
              <button
                onClick={onCancel}
                className={`w-full py-3 text-[17px] font-normal text-center transition-colors ${isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100'}`}
              >
                取消
              </button>
            </div>
            <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
              <button
                onClick={() => canSubmit && onConfirm(values)}
                disabled={!canSubmit}
                className={`w-full py-3 text-[17px] font-semibold text-center transition-colors ${
                  canSubmit
                    ? (isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100')
                    : (isDark ? 'text-slate-600' : 'text-slate-300')
                }`}
              >
                {config.backendId === 'feishu' || fields.length > 0 ? '连接' : '安装'}
              </button>
            </div>
          </div>
        </div>
      );
    };

    // Obsidian 连接前探测引导卡：未安装 → 引导下载；没库 / 库丢失 → 引导建库/重开
    const TsObsidianGuide = ({ guide, theme, onCancel, onDownload, onRetry, allowDownload = true }) => {
      const isDark = theme === 'dark';
      if (!guide) return null;
      const COPY = {
        not_installed: { title: '需要先安装 Obsidian', body: '「Obsidian 知识库」需配合 Obsidian 使用。检测到你尚未安装，安装并创建一个库后即可连接。', primary: '下载 Obsidian', retry: '我已安装，重新检测' },
        no_vault: { title: '还没有笔记库', body: '已检测到 Obsidian，但你还没创建过笔记库。请在 Obsidian 里新建一个库后再连接。', primary: null, retry: '我已新建，重新检测' },
        vault_missing: { title: '库文件夹不存在', body: '上次的笔记库文件夹找不到了。请在 Obsidian 重新打开，或新建一个库后再连接。', primary: null, retry: '重新检测' },
      };
      const c = COPY[guide.state] || COPY.not_installed;
      const btn = (label, on, cls) => (
        <div className={`border-t ${isDark ? 'border-white/10' : 'border-slate-200'}`}>
          <button onClick={on} className={`w-full py-3 text-center transition-colors ${cls}`}>{label}</button>
        </div>
      );
      return (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 backdrop-blur-sm">
          <div className={`w-[300px] rounded-[20px] overflow-hidden shadow-2xl ${isDark ? 'bg-[#2C2C2E]' : 'bg-white/95 backdrop-blur-xl'}`} style={{ animation: 'tsAlertIn .2s ease-out' }}>
            <div className="px-6 pt-6 pb-4 text-center">
              <div className="text-[34px] mb-2">📖</div>
              <div className={`text-[17px] font-semibold mb-2 ${isDark ? 'text-white' : 'text-slate-900'}`}>{c.title}</div>
              <div className={`text-[13px] leading-relaxed ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>{!allowDownload && guide.state === 'not_installed' ? '请先在桌面端安装 Obsidian 并创建笔记库，然后在这里重新检测。' : c.body}</div>
            </div>
            {allowDownload && c.primary && btn(c.primary, onDownload, `text-[17px] font-semibold ${isDark ? 'text-[#0A84FF] active:bg-white/5' : 'text-[#007AFF] active:bg-slate-100'}`)}
            {btn(c.retry, onRetry, `text-[15px] ${isDark ? 'text-slate-300 active:bg-white/5' : 'text-slate-600 active:bg-slate-100'}`)}
            {btn('取消', onCancel, `text-[15px] ${isDark ? 'text-slate-500 active:bg-white/5' : 'text-slate-400 active:bg-slate-100'}`)}
          </div>
        </div>
      );
    };

    const ToolStoreView = ({ theme, onNewChat }) => {
      const isDark = theme === 'dark';
      const externalAuthAvailable = canStartExternalAuth();
      const canMutateToolStore = can('toolStoreMutations');
      const [searchQuery, setSearchQuery] = useState('');
      const [activeCategory, setActiveCategory] = useState('all');
      const [selectedTool, setSelectedTool] = useState(null);
      const [showH3cModal, setShowH3cModal] = useState(false); // H3C 集团内部工具合集详情
      const featuredScrollRef = useRef(null);
      const [isFeaturedHovered, setIsFeaturedHovered] = useState(false);
      const [toolStates, setToolStates] = useState({});
      const [toolAuthStates, setToolAuthStates] = useState({});
      // 配套技能 id → 所属 MCP id(由 list_marketplace_tools 的 companion_skills 反建,manifest 单一真源)。
      // 有配套 MCP 的技能卡据此把状态/装卸联动到该 MCP,避免命名不一致(government-writing↔gongwen)时状态分叉。
      const [skillToMcp, setSkillToMcp] = useState({});
      const [busyId, setBusyId] = useState(null);
      const [alert, setAlert] = useState({ visible: false, loading: false, title: '', subtitle: '', isInstall: false, isError: false });
      const oauthRequestRef = useRef({});
      const [configDialog, setConfigDialog] = useState(null); // { backendId, name, fields }
      const [obsidianGuide, setObsidianGuide] = useState(null); // {backendId,name,state,vault_path} 未安装/没库引导
      const [viewMode, setViewMode] = useState('card'); // 'card'(卡片视图) | 'list'(列表视图)
      const [installedOnly, setInstalledOnly] = useState(false); // 头像入口:只看已安装
      const [skillBackend, setSkillBackend] = useState([]); // list_marketplace_skills 原始返回
      const isCard = viewMode === 'card';
      const isSkillTab = isCard; // 兼容:卡片视图 = 渲染本地技能 Today 卡
      const showFeaturedCollections = isCard && searchQuery === '' && activeCategory === 'all';
      // 连接器 tab 只显示"需连外部数据"的工具,排除本地生成类(PPT / 公文)
      const LOCAL_TOOLS = ['pptx', 'gongwen'];
      // 飞书(CLI 路线)连接态:不走 marketplace,由 lark-cli auth status 判定
      const [feishuConnected, setFeishuConnected] = useState(false);
      // 飞书连接流程状态机（取代旧阻塞式扫码浮层）：null=idle
      // { phase:'running'|'qr'|'error'|'done', steps:{runtime,cli,connect,qr}, active, pct, sec, log, err, qr, qrUrl, qrPhase }
      const [feishuFlow, setFeishuFlow] = useState(feishuConn.flow); // 从跨视图 store 水合：切走再回来不丢进度
      const refreshFeishu = async () => {
        try {
          const s = await invokeTauri('feishu_status');
          setFeishuConnected(!!(s && s.connected));
        } catch (e) { console.error('feishu_status failed:', e); }
      };
      useEffect(() => { refreshFeishu(); }, []);

      // 企业微信(CLI 路线)连接态:同飞书,由 wecom-cli auth show 判定
      const [wecomConnected, setWecomConnected] = useState(false);
      const [wecomQr, setWecomQr] = useState(null); // { qr: dataUrl, url } 扫码弹窗(单段)
      const [wecomFlow, setWecomFlow] = useState(wecomConn.flow); // 企微连接流程卡(跨视图水合)
      const refreshWecom = async () => {
        try {
          const s = await invokeTauri('wecom_status');
          setWecomConnected(!!(s && s.connected));
        } catch (e) { console.error('wecom_status failed:', e); }
      };
      useEffect(() => { refreshWecom(); }, []);

      // 钉钉(CLI 路线)连接态:由 dws auth status 判定
      const [dingtalkConnected, setDingtalkConnected] = useState(false);
      const [dingtalkFlow, setDingtalkFlow] = useState(dingtalkConn.flow);
      const refreshDingtalk = async () => {
        try {
          const s = await invokeTauri('dingtalk_status');
          setDingtalkConnected(!!(s && s.connected));
        } catch (e) { console.error('dingtalk_status failed:', e); }
      };
      useEffect(() => { refreshDingtalk(); }, []);

      useEffect(() => {
        const urls = [
          'assets/h3c-banner.jpg',
          ...tsFeaturedCollections.map((item) => item.img),
          ...tsSkillsData.map((item) => item.todayImg),
        ].filter(Boolean);
        urls.forEach((src) => {
          const img = new Image();
          img.decoding = 'async';
          img.src = src;
        });
      }, []);

      // 订阅跨视图 store：把 store 状态镜像进本组件渲染，并在完成/失败时做组件级收尾
      //（弹窗、刷新连接态）。真正的事件监听/秒表在模块级 feishuConn 里，切视图不丢。
      useEffect(() => {
        if (!externalAuthAvailable) return undefined;
        ensureFeishuListeners();
        let prevPhase = feishuConn.flow && feishuConn.flow.phase;
        const unsub = feishuConn.subscribe((flow) => {
          setFeishuFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setFeishuConnected(true); setBusyId(null);
              setAlert({ visible: true, loading: false, title: '已连接飞书', subtitle: '官方技能已启用，可新建对话直接用', isInstall: true, isError: false, toolId: 'feishu' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setFeishuFlow(feishuConn.flow); // (重)挂载即水合当前进度
        return unsub;
      }, [externalAuthAvailable]);

      // 订阅企业微信 store(镜像飞书):镜像进渲染 + 完成/失败收尾
      useEffect(() => {
        if (!externalAuthAvailable) return undefined;
        ensureWecomListeners();
        let prevPhase = wecomConn.flow && wecomConn.flow.phase;
        const unsub = wecomConn.subscribe((flow) => {
          setWecomFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setWecomConnected(true); setBusyId(null);
              setAlert({ visible: true, loading: false, title: '已连接企业微信', subtitle: '官方技能已启用，可新建对话直接用', isInstall: true, isError: false, toolId: 'wecom' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setWecomFlow(wecomConn.flow);
        return unsub;
      }, [externalAuthAvailable]);

      // 订阅钉钉 store(镜像企微):镜像进渲染 + 完成/失败收尾
      useEffect(() => {
        if (!externalAuthAvailable) return undefined;
        ensureDingtalkListeners();
        let prevPhase = dingtalkConn.flow && dingtalkConn.flow.phase;
        const unsub = dingtalkConn.subscribe((flow) => {
          setDingtalkFlow(flow);
          const ph = flow && flow.phase;
          if (ph !== prevPhase) {
            if (ph === 'done') {
              setDingtalkConnected(true); setBusyId(null);
              setAlert({ visible: true, loading: false, title: '已连接钉钉', subtitle: '官方技能已启用，可新建对话直接用', isInstall: true, isError: false, toolId: 'dingtalk' });
              notifyComposerToolsChanged();
            } else if (ph === 'error') {
              setBusyId(null);
            }
            prevPhase = ph;
          }
        });
        setDingtalkFlow(dingtalkConn.flow);
        return unsub;
      }, [externalAuthAvailable]);

      // EIP(CLI 路线)连接态:由 eip-cli auth status 判定(同飞书)。
      const [eipConnected, setEipConnected] = useState(false);
      const [eipSso, setEipSso] = useState(null); // { url, qr } SSO 登录引导弹窗
      const [zhidaoConnected, setZhidaoConnected] = useState(false);
      const [zhidaoSso, setZhidaoSso] = useState(null); // { url, qr } SSO 登录引导弹窗
      const refreshEip = async () => {
        try {
          const s = await invokeTauri('eip_status');
          setEipConnected(!!(s && s.connected));
        } catch (e) { console.error('eip_status failed:', e); }
      };
      const refreshZhidao = async () => {
        try {
          const s = await invokeTauri('zhidao_status');
          setZhidaoConnected(!!(s && s.connected));
        } catch (e) { console.error('zhidao_status failed:', e); }
      };
      useEffect(() => { refreshEip(); refreshZhidao(); }, []);

      // 企微/EIP/知道 连接编排事件:后端推进度,前端驱动 UI。
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return;
        const unlisten = [];
        ev.listen('wecom:qr', (e) => {
          const p = e.payload || {};
          // 二维码到了 → 清掉一直显示的"正在生成…"loading,再弹出二维码弹窗。
          setAlert(a => ({ ...a, visible: false, loading: false }));
          setWecomQr({ qr: p.qr_data_url, url: p.url, phase: p.phase });
        }).then(u => unlisten.push(u));
        ev.listen('wecom:connected', () => {
          setWecomQr(null); setWecomConnected(true); setBusyId(null);
          // 连上 → 按规则写技能(默认启用),企微技能即刻对模型可见。
          invokeTauri('wecom_apply_skills').catch(() => {});
          setAlert({ visible: true, loading: false, title: '已连接企业微信', subtitle: '', isInstall: true, isError: false, toolId: 'wecom' });
          notifyComposerToolsChanged();
        }).then(u => unlisten.push(u));
        ev.listen('wecom:error', (e) => {
          const p = e.payload || {};
          setWecomQr(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: '企业微信连接失败', subtitle: String(p.message || '').slice(0, 240), isError: true });
        }).then(u => unlisten.push(u));
        ev.listen('eip:sso', (e) => {
          const p = e.payload || {};
          setEipSso({ url: p.url, qr: p.qr_data_url });
        }).then(u => unlisten.push(u));
        ev.listen('eip:connected', () => {
          setEipSso(null); setEipConnected(true); setBusyId(null);
          setAlert({ visible: true, loading: false, title: '已连接员工门户（EIP）', subtitle: '', isInstall: true, isError: false, toolId: 'eip' });
          notifyComposerToolsChanged();
        }).then(u => unlisten.push(u));
        ev.listen('eip:error', (e) => {
          const p = e.payload || {};
          setEipSso(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: 'EIP 连接失败', subtitle: String(p.message || '').slice(0, 240), isError: true });
        }).then(u => unlisten.push(u));
        ev.listen('zhidao:sso', (e) => {
          const p = e.payload || {};
          setZhidaoSso({ url: p.url, qr: p.qr_data_url });
        }).then(u => unlisten.push(u));
        ev.listen('zhidao:connected', () => {
          setZhidaoSso(null); setZhidaoConnected(true); setBusyId(null);
          setAlert({ visible: true, loading: false, title: '已连接知道知识库', subtitle: '', isInstall: true, isError: false, toolId: 'zhidao' });
          notifyComposerToolsChanged();
        }).then(u => unlisten.push(u));
        ev.listen('zhidao:error', (e) => {
          const p = e.payload || {};
          setZhidaoSso(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: '知道连接失败', subtitle: String(p.message || '').slice(0, 240), isError: true });
        }).then(u => unlisten.push(u));
        return () => { unlisten.forEach(u => { try { u(); } catch (_) {} }); };
      }, [externalAuthAvailable]);

      // 合并后端安装状态到 mock 数据(飞书/企微/EIP/知道 的 installed = 已连接)
      // 新分类映射(按工具 id):沟通协作 / 文档知识 / 研发 / 金融数据 / 生活实用 / H3C 内部
      const CAT_BY_ID = { 1: 'life', 2: 'finance', 3: 'collab', 4: 'docs', 5: 'docs', 6: 'docs', 7: 'collab', 8: 'collab', 9: 'collab', 10: 'collab', 11: 'dev', 12: 'dev', 13: 'finance', 14: 'docs', 17: 'h3c', 18: 'h3c', 99: 'collab' };
      const tools = tsToolsData.map(t => {
        const authState = t.oauthMcp && t.backendId ? toolAuthStates[t.backendId] : null;
        return {
          ...t,
          category: CAT_BY_ID[t.id] || t.category,
          logoSrc: THIRD_PARTY_TOOL_LOGOS[t.backendId] || THIRD_PARTY_TOOL_LOGOS[t.id] || null,
          installed: t.feishuCli
            ? feishuConnected
            : t.wecomCli
            ? wecomConnected
            : t.dingtalkCli
            ? dingtalkConnected
            : t.eipCli
            ? eipConnected
            : t.zhidaoCli
            ? zhidaoConnected
            : t.oauthMcp
            ? authState?.status === 'connected'
            : (t.backendId ? (toolStates[t.backendId] || false) : false),
          authStatus: authState?.status || 'not_installed',
          authMessage: authState?.message || '',
          mcpConfigured: !!authState?.mcp_configured,
          oauthTokenPresent: !!authState?.oauth_token_present,
        };
      });
      const isToolVisibleOnPlatform = (tool) => (
        externalAuthAvailable
        || !isRestrictedExternalAuthTool(tool)
        || !!tool.installed
      );
      const visibleInternalTools = tools.filter(tool => tool.internal && isToolVisibleOnPlatform(tool));

      // 技能卡 = 预置(合并安装状态) + 用户上传(后端动态返回,默认图标)
      const presetSkills = tsSkillsData.map(s => {
        if (s.builtin) return { ...s, installed: true };
        // 有配套 MCP 的技能(公文=gongwen,manifest companion_skills 声明)→ 跟随该 MCP 工具态;
        // 同名工具的展示别名(PPT=pptx)同样跟工具态;都不命中才读独立 skill 后端(纯技能/上传)。
        const mcpId = skillToMcp[s.backendId]
          || (tsToolsData.some(t => t.backendId === s.backendId) ? s.backendId : null);
        if (mcpId) return { ...s, installed: !!toolStates[mcpId] };
        const be = skillBackend.find(x => x.id === s.backendId);
        return { ...s, installed: be ? be.installed : false };
      });
      const uploadedSkills = skillBackend.filter(x => x.user_uploaded).map(x => ({
        id: 'up-' + x.id, backendId: x.id, title: x.title, subtitle: x.subtitle || '用户上传的技能',
        category: 'skill', type: 'Skill', version: '—', latency: '本地', desc: x.description || '',
        icon: Package, color: 'bg-gradient-to-b from-slate-400 to-slate-600', installed: true, userUploaded: true,
      }));
      const skillCards = [...presetSkills, ...uploadedSkills];

      const connectorTools = tools.filter(t => !LOCAL_TOOLS.includes(t.backendId) && isToolVisibleOnPlatform(t));
      const listItems = [...connectorTools, ...skillCards]; // 列表视图:连接器 + 技能全放一起
      // 独家技能:公文写作 / PPT 生成 / 数据可视化 / 视觉设计(按此序;视觉无 backendId,PIN 后自然排末)
      const FEATURED_SKILL = s => ['government-writing', 'pptx', 'visualizer'].includes(s.backendId) || s.id === 's5';
      // 搜索全局:有搜索词时跨「连接器 + 全部技能」检索,不受卡片视图/分类限制(「我的工具」内搜索仍限已安装)
      const searching = searchQuery.trim() !== '';
      const sourceItems = (searching && !installedOnly) ? listItems : (isCard ? skillCards.filter(FEATURED_SKILL) : listItems);
      const isLaunchedTool = tool => !!tool.backendId || !!tool.builtin || !!tool.userUploaded;
      const visibleCategories = tsCategories.filter(cat => cat.id === 'all' || listItems.some(tool => (
        isLaunchedTool(tool)
        && (cat.id === 'h3c' ? (tool.category === 'h3c' || !!tool.internal) : tool.category === cat.id)
      )));
      const PIN = ['government-writing', 'pptx', 'visualizer'];
      const filteredTools = sourceItems.filter(tool => {
        if (!isLaunchedTool(tool)) return false;
        const q = searchQuery.toLowerCase();
        const matchesSearch = tool.title.toLowerCase().includes(q) || (tool.desc || '').toLowerCase().includes(q);
        if (installedOnly && !isCard) return matchesSearch && tool.installed;
        const matchesCategory = searching || isCard || activeCategory === 'all' || (activeCategory === 'h3c' ? (tool.category === 'h3c' || !!tool.internal) : tool.category === activeCategory);
        return matchesSearch && matchesCategory;
      }).sort((a, b) => {
        if (isCard && !searching) { const r = x => { const i = PIN.indexOf(x.backendId); return i === -1 ? 99 : i; }; if (r(a) !== r(b)) return r(a) - r(b); }
        // 已上线(有 backendId 或内置)排在未上线(即将上线)之前
        const onA = !!a.backendId || !!a.builtin, onB = !!b.backendId || !!b.builtin;
        if (onA !== onB) return onA ? -1 : 1;
        if (a.installed && !b.installed) return -1;
        if (!a.installed && b.installed) return 1;
        return 0;
      });
      useEffect(() => {
        if (!isCard && !installedOnly && !searching && activeCategory !== 'all' && !visibleCategories.some(cat => cat.id === activeCategory)) {
          setActiveCategory('all');
        }
      }, [activeCategory, installedOnly, isCard, searching, visibleCategories]);

      // 从后端加载已安装状态
      const loadBackendState = async () => {
        try {
          const list = await invokeTauri('list_marketplace_tools');
          const states = {};
          const s2m = {}; // 配套技能 → 所属 MCP(manifest companion_skills 反建,单一真源)
          list.forEach(t => {
            states[t.id] = t.installed;
            (t.companion_skills || []).forEach(sid => { s2m[sid] = t.id; });
          });
          setToolStates(states);
          setSkillToMcp(s2m);
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
        try {
          const skills = await invokeTauri('list_marketplace_skills');
          setSkillBackend(Array.isArray(skills) ? skills : []);
        } catch (e) {
          console.error('list_marketplace_skills failed:', e);
        }
      };

      useEffect(() => { loadBackendState(); }, []);

      const beginOAuthRequest = (backendId) => {
        const requestId = `${Date.now()}-${Math.random()}`;
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

      const markOAuthPending = (backendId, message = '已写入 MCP 配置，但尚未完成 OAuth 授权。') => {
        setToolAuthStates(prev => ({
          ...prev,
          [backendId]: {
            ...(prev[backendId] || {}),
            installed: true,
            mcp_configured: true,
            oauth_required: true,
            oauth_token_present: false,
            status: 'config_installed_auth_pending',
            message,
          },
        }));
        if (selectedTool && selectedTool.backendId === backendId) {
          setSelectedTool(prev => ({
            ...prev,
            installed: false,
            authStatus: 'config_installed_auth_pending',
            authMessage: message,
          }));
        }
      };

      const cancelOAuthLoading = async (activeAlert) => {
        const backendId = activeAlert?.toolId;
        const requestId = activeAlert?.requestId;
        if (!backendId || !isCurrentOAuthRequest(backendId, requestId)) return;
        setAlert(prev => ({
          ...prev,
          cancelable: false,
          subtitle: '正在停止浏览器授权等待…',
        }));
        try {
          await invokeTauri('cancel_marketplace_tool_oauth_login', {
            toolId: backendId,
            requestId,
          });
          if (isCurrentOAuthRequest(backendId, requestId)) {
            const t = tsToolsData.find(x => x.backendId === backendId);
            const name = t ? t.title : backendId;
            clearOAuthRequest(backendId, requestId);
            setBusyId(null);
            const outcome = resolveOAuthInstallOutcome(
              name,
              { status: 'cancelled', message: '已取消等待浏览器授权，可稍后重新授权。' },
              {
                installed: true,
                mcp_configured: true,
                oauth_required: true,
                oauth_token_present: false,
                status: 'config_installed_auth_pending',
              }
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
              subtitle: '取消失败，可重试；授权等待仍在继续。',
            }));
          }
        }
      };

      // 执行安装（已拿到 config 或无需 config）
      const doInstall = async (backendId, userConfig) => {
        if (!canMutateToolStore) return;
        const t = tsToolsData.find(x => x.backendId === backendId);
        if (!externalAuthAvailable && isRestrictedExternalAuthTool(t)) return;
        const name = t ? t.title : backendId;
        const hasConfig = Boolean(t?.configFields?.length);
        const hasPipDeps = !hasConfig; // 无 config 的本地工具可能有 pip deps
        const oauthServerName = t?.oauthMcp ? oauthServerNameForTool(t) : null;
        if (t?.oauthMcp && !oauthServerName) {
          setAlert({ visible: true, loading: false, title: 'OAuth 配置错误', subtitle: `「${name}」未声明 MCP server name，无法发起授权。`, isInstall: false, isError: true });
          return;
        }
        const oauthRequestId = t?.oauthMcp ? beginOAuthRequest(backendId) : null;
        setBusyId(backendId);
        if (t?.oauthMcp) {
          setAlert({ loading: true, visible: false, title: `正在连接「${name}」`, subtitle: '正在写入 MCP 配置…', isInstall: true, isError: false, cancelable: false, toolId: backendId, requestId: oauthRequestId });
        } else if (hasConfig) {
          setAlert({ loading: true, visible: false, title: `正在连接「${name}」`, subtitle: '正在校验 API Key 与远程工具…', isInstall: true, isError: false });
        } else if (hasPipDeps) {
          setAlert({ loading: true, visible: false, title: `正在安装「${name}」`, subtitle: '首次安装需下载依赖，请耐心等待…', isInstall: true, isError: false });
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
                message: '正在等待浏览器授权完成。',
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
              title: `正在连接「${name}」`,
              subtitle: '已打开浏览器，正在等待授权…',
              isInstall: true,
              isError: false,
              cancelable: true,
              toolId: backendId,
              requestId: oauthRequestId,
            });
            const loginResult = await withUiTimeout(
              loginPromise,
              OAUTH_UI_TIMEOUT_MS,
              oauthUiTimeoutResult(oauthServerName)
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

            const outcome = resolveOAuthInstallOutcome(name, loginResult, authStatus);
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
            title: hasConfig ? `已连接「${name}」` : `已安装「${name}」`,
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
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', subtitle: String(e && e.message ? e.message : e).slice(0, 240), isInstall: false, isError: true });
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
          setAlert({ visible: true, loading: false, title: `${isInstalled ? '已卸载' : '已安装'}「${name}」`, isInstall: !isInstalled, isError: false });
          if (selectedTool && selectedTool.backendId === backendId) {
            setSelectedTool(prev => ({ ...prev, installed: !isInstalled }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('skill action failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败：' + e, isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 上传 zip 技能包(Rust 端弹 dialog 选文件)
      const handleUploadSkill = async () => {
        if (!canMutateToolStore) return;
        setBusyId('__upload__');
        setAlert({ loading: true, visible: false, title: '正在导入技能包…', subtitle: '校验并解压中', isInstall: true, isError: false });
        try {
          const ok = await invokeTauri('import_skill_package');
          if (ok) {
            await loadBackendState();
            setAlert({ visible: true, loading: false, title: '技能包已导入', isInstall: true, isError: false });
          } else {
            setAlert({ visible: false, loading: false, title: '', isInstall: false, isError: false }); // 用户取消
          }
        } catch (e) {
          console.error('import skill failed:', e);
          setAlert({ visible: true, loading: false, title: '导入失败：' + e, isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接飞书(config init --new 自建 app,两段扫码):事件驱动。
      // 进度走后端事件 feishu:qr / feishu:phase / feishu:connected / feishu:error
      //(监听见下方 useEffect);这里只 ensure cli + 触发 begin。busyId 在事件里清。
      const connectFeishu = async () => {
        setBusyId('feishu');
        ensureFeishuListeners();
        // 开流程卡（无阻塞弹窗）：先起“准备运行时”步。写进跨视图 store，切走不丢。
        feishuConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        // 客户端秒表 + 爬行条：后端 feishu:progress 有真实 pct 时会覆盖；没有也不至于像卡死。
        feishuConn.startTick();
        try {
          // ① 确保 CLI（B 方案下可能联网安装 ~40s，会 emit feishu:progress step=cli）
          feishuConn.setFlow(f => ({ ...(f || {}), active: 'cli', pct: 0, log: 'npm: starting…', steps: { ...((f && f.steps) || {}), runtime: 'done', cli: 'active' } }));
          await invokeTauri('feishu_ensure_cli');
          feishuConn.setFlow(f => ({ ...(f || {}), active: 'connect', pct: 100, steps: { ...((f && f.steps) || {}), cli: 'done', connect: 'active' } }));
          // ② 连接编排（后端 emit feishu:qr / connected / error）
          await invokeTauri('feishu_connect_begin');
        } catch (e) {
          console.error('feishu connect failed:', e);
          feishuConn.stopTick();
          setBusyId(null);
          feishuConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } };
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
          // 断开 → 撤掉技能(should_show 变 false)+ 广播刷新。
          await invokeTauri('feishu_apply_skills').catch(() => {});
          setFeishuConnected(false);
          setAlert({ visible: true, loading: false, title: '已断开飞书', isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('feishu logout failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接企业微信(单段扫码):流程卡驱动(镜像飞书),进度走 wecom:* 事件。
      const connectWecom = async () => {
        setBusyId('wecom');
        ensureWecomListeners();
        // 开流程卡(无阻塞弹窗):先起"准备运行时"步,写进跨视图 store,切走不丢。
        wecomConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        wecomConn.startTick();
        try {
          // ① 确保 CLI(首次联网装 wecom-cli ~40s)
          wecomConn.setFlow(f => ({ ...(f || {}), active: 'cli', pct: 0, log: 'npm: starting…', steps: { ...((f && f.steps) || {}), runtime: 'done', cli: 'active' } }));
          await invokeTauri('wecom_ensure_cli');
          wecomConn.setFlow(f => ({ ...(f || {}), pct: 100, steps: { ...((f && f.steps) || {}), cli: 'done' } }));
          // ② 连接编排(后端 emit wecom:qr / connected / error)
          await invokeTauri('wecom_connect_begin');
        } catch (e) {
          console.error('wecom connect failed:', e);
          wecomConn.stopTick();
          setBusyId(null);
          wecomConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } };
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
          // 断开 → 撤掉技能(should_show 变 false)。
          await invokeTauri('wecom_apply_skills').catch(() => {});
          setWecomConnected(false);
          setAlert({ visible: true, loading: false, title: '已断开企业微信', isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('wecom logout failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接钉钉(单段扫码):流程卡驱动(镜像企微),进度走 dingtalk:* 事件。
      const connectDingtalk = async () => {
        setBusyId('dingtalk');
        ensureDingtalkListeners();
        dingtalkConn.setFlow({ phase: 'running', steps: { runtime: 'active' }, active: 'runtime', pct: 0, sec: 0, log: '' });
        dingtalkConn.startTick();
        try {
          dingtalkConn.setFlow(f => ({ ...(f || {}), active: 'cli', pct: 0, log: 'npm: starting…', steps: { ...((f && f.steps) || {}), runtime: 'done', cli: 'active' } }));
          await invokeTauri('dingtalk_ensure_cli');
          dingtalkConn.setFlow(f => ({ ...(f || {}), pct: 100, steps: { ...((f && f.steps) || {}), cli: 'done' } }));
          await invokeTauri('dingtalk_connect_begin');
        } catch (e) {
          console.error('dingtalk connect failed:', e);
          dingtalkConn.stopTick();
          setBusyId(null);
          dingtalkConn.setFlow(f => {
            const step = (f && f.active) || 'cli';
            return { ...(f || { steps: {} }), phase: 'error', err: String(e).slice(0, 300), errStep: step, steps: { ...((f && f.steps) || {}), [step]: 'error' } };
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
          setDingtalkConnected(false);
          setAlert({ visible: true, loading: false, title: '已断开钉钉', isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('dingtalk logout failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接 EIP(SSO 轮询自动收):事件驱动。二进制内置,无需 ensure_cli/扫码。
      const connectEip = async () => {
        setBusyId('eip');
        try {
          await invokeTauri('eip_connect_begin');
          // 后续:eip:sso 出登录地址 → 用户浏览器 SSO 登录 → eip:connected 收尾。
        } catch (e) {
          console.error('eip connect failed:', e);
          setEipSso(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: 'EIP 连接失败', subtitle: String(e).slice(0, 240), isError: true });
        }
      };
      const disconnectEip = async () => {
        setBusyId('eip');
        try {
          await invokeTauri('eip_logout');
          setEipConnected(false);
          setAlert({ visible: true, loading: false, title: '已断开员工门户（EIP）', isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('eip logout failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 连接知道知识库(SSO 轮询自动收):事件驱动。二进制内置,无需手工粘贴 token。
      const connectZhidao = async () => {
        setBusyId('zhidao');
        try {
          await invokeTauri('zhidao_connect_begin');
        } catch (e) {
          console.error('zhidao connect failed:', e);
          setZhidaoSso(null); setBusyId(null);
          setAlert({ visible: true, loading: false, title: '知道连接失败', subtitle: String(e).slice(0, 240), isError: true });
        }
      };
      const disconnectZhidao = async () => {
        setBusyId('zhidao');
        try {
          await invokeTauri('zhidao_logout');
          setZhidaoConnected(false);
          setAlert({ visible: true, loading: false, title: '已断开知道知识库', isInstall: false, isError: false });
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('zhidao logout failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isError: true });
        } finally {
          setBusyId(null);
        }
      };

      // 安装/卸载入口
      const handleAction = async (backendId, isInstalled) => {
        if (!canMutateToolStore) return;
        // 有配套 MCP 的技能(公文=gongwen)→ 改走该 MCP 装卸,skill 作为 companion 随 MCP 联动(两卡同步);
        // 纯技能(无配套 MCP、无同名工具:如上传技能)才走 handleSkillAction。PPT=pptx 有同名工具,落下方正常工具流。
        if (skillToMcp[backendId]) backendId = skillToMcp[backendId];
        else if (tsSkillsData.some(s => s.backendId === backendId) && !tsToolsData.some(t => t.backendId === backendId)) return handleSkillAction(backendId, isInstalled);
        const requestedTool = tools.find(x => x.backendId === backendId) || tsToolsData.find(x => x.backendId === backendId);
        if (!externalAuthAvailable && isRestrictedExternalAuthTool(requestedTool)) return;
        // 飞书走 CLI 连接流程,不走 marketplace install
        if (backendId === 'feishu') {
          if (isInstalled) return disconnectFeishu();
          // 未连接 → 弹详情弹窗（里面有进度卡）+ 触发 config init --new(浏览器自动建 app + 两段扫码,不收表单)
          const ft = tools.find(x => x.feishuCli) || tsToolsData.find(x => x.backendId === 'feishu');
          if (ft) setSelectedTool(ft);
          return connectFeishu();
        }
        // 企微同走 CLI 连接流程(单段扫码)
        if (backendId === 'wecom') {
          if (isInstalled) return disconnectWecom();
          // 打开详情弹窗(里面有流程卡)+ 触发连接
          const wt = tools.find(x => x.wecomCli) || tsToolsData.find(x => x.backendId === 'wecom');
          if (wt) setSelectedTool(wt);
          return connectWecom();
        }
        // 钉钉同走 CLI 连接流程(单段扫码)
        if (backendId === 'dingtalk') {
          if (isInstalled) return disconnectDingtalk();
          const dt = tools.find(x => x.dingtalkCli) || tsToolsData.find(x => x.backendId === 'dingtalk');
          if (dt) setSelectedTool(dt);
          return connectDingtalk();
        }
        // EIP 走 CLI 连接流程(浏览器 SSO 一键登录),不走 marketplace install
        if (backendId === 'eip') {
          if (isInstalled) return disconnectEip();
          return connectEip();
        }
        // 知道知识库走 CLI + SSO 自动轮询流程,不走 marketplace install
        if (backendId === 'zhidao') {
          if (isInstalled) return disconnectZhidao();
          return connectZhidao();
        }
        const t = tsToolsData.find(x => x.backendId === backendId);
        const name = t ? t.title : backendId;

        // 安装：有 configFields 的工具先弹配置弹窗
        if (!isInstalled) {
          // Obsidian：连接前先探测本机状态——没装/没库就引导，不默默装个用不了的连接器
          if (backendId === 'obsidian') {
            let st = null;
            try { st = await invokeTauri('detect_obsidian'); } catch (e) {}
            if (st && st.state && st.state !== 'ok') { setObsidianGuide({ backendId, name, ...st }); return; }
            return doInstall(backendId, {});
          }
          if (t?.configFields && t.configFields.length > 0) {
            setConfigDialog({
              backendId,
              name,
              fields: t.configFields,
              configTitle: t.configTitle,
              configDescription: t.configDescription,
              configDocUrl: t.configDocUrl,
              configDocLabel: t.configDocLabel,
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
          if (t?.oauthMcp) {
            setToolAuthStates(prev => ({
              ...prev,
              [backendId]: {
                installed: false,
                mcp_configured: false,
                oauth_required: true,
                oauth_token_present: false,
                status: 'not_installed',
                message: `尚未连接「${name}」。`,
              },
            }));
          }
          setAlert({ visible: true, loading: false, title: `已卸载「${name}」`, isInstall: false, isError: false });
          if (selectedTool && selectedTool.backendId === backendId) {
            setSelectedTool(prev => ({ ...prev, installed: false, authStatus: 'not_installed', authMessage: '' }));
          }
          notifyComposerToolsChanged();
        } catch (e) {
          console.error('uninstall failed:', e);
          setAlert({ visible: true, loading: false, title: '操作失败，请重试', isInstall: false, isError: true });
        } finally {
          setBusyId(null);
        }
      };

      useEffect(() => {
        if (selectedTool) document.body.style.overflow = 'hidden';
        else document.body.style.overflow = 'unset';
        return () => { document.body.style.overflow = 'unset'; };
      }, [selectedTool]);

      useEffect(() => {
        if (isFeaturedHovered || searchQuery !== '' || activeCategory !== 'all') return;
        const interval = setInterval(() => {
          if (featuredScrollRef.current) {
            const { scrollLeft, scrollWidth, clientWidth } = featuredScrollRef.current;
            if (scrollLeft + clientWidth >= scrollWidth - 50) {
              featuredScrollRef.current.scrollTo({ left: 0, behavior: 'smooth' });
            } else {
              featuredScrollRef.current.scrollBy({ left: 424, behavior: 'smooth' });
            }
          }
        }, 4000);
        return () => clearInterval(interval);
      }, [isFeaturedHovered, searchQuery, activeCategory]);

      const handleScrollFeatured = (direction) => {
        if (featuredScrollRef.current) {
          featuredScrollRef.current.scrollBy({
            left: direction === 'left' ? -424 : 424,
            behavior: 'smooth'
          });
        }
      };

      return (
        <div className={`${isDark ? 'dark' : ''} flex-1 flex flex-col w-full h-full relative z-10 overflow-hidden antialiased selection:bg-blue-200 dark:selection:bg-blue-900`}>
          {createPortal(<TsAlert alert={alert} theme={theme} onDismiss={() => setAlert(a => ({ ...a, visible: false }))} onCancelLoading={cancelOAuthLoading} onNewChat={() => { const tid = alert.toolId; setAlert(a => ({ ...a, visible: false })); if (onNewChat) onNewChat(tid); }} />, document.body)}
          {createPortal(<TsConfigDialog
            config={externalAuthAvailable ? configDialog : null}
            theme={theme}
            onCancel={() => setConfigDialog(null)}
            onConfirm={(values) => { const bid = configDialog.backendId; setConfigDialog(null); doInstall(bid, values); }}
          />, document.body)}
          {createPortal(<TsObsidianGuide
            guide={obsidianGuide}
            theme={theme}
            allowDownload={can('localModelSetup')}
            onCancel={() => setObsidianGuide(null)}
            onDownload={() => invokeTauri('open_external_url', { url: 'https://obsidian.md/' }).catch(() => {})}
            onRetry={async () => {
              let st = null;
              try { st = await invokeTauri('detect_obsidian'); } catch (e) {}
              if (st && st.state === 'ok') { const bid = obsidianGuide.backendId; setObsidianGuide(null); doInstall(bid, {}); }
              else setObsidianGuide(g => g ? { ...g, ...(st || {}) } : g);
            }}
          />, document.body)}
          {/* 飞书扫码二维码已内联进 FeishuFlowCard（详情弹窗内），不再单独浮层 */}
          {wecomQr && (() => {
            const cancel = () => { invokeTauri('wecom_cancel').catch(() => {}); setWecomQr(null); setBusyId(null); };
            return createPortal((
            <div className="fixed inset-0 z-[200] flex items-center justify-center p-4" style={{ backgroundColor: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(8px)' }} onClick={cancel}>
              <div className="bg-white dark:bg-[#1C1C1E] rounded-3xl p-7 w-full max-w-[440px] flex flex-col items-center text-center shadow-2xl" onClick={e => e.stopPropagation()}>
                <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-4">连接企业微信</h3>
                {/* 文案精简(方案A):扫码指引交给内嵌页自己说，这里不重复。直接内嵌企微登录页
                    （其 JS 动态渲染真正的登录码）——避免把 gen 网页地址编码成二维码导致的二次扫码。 */}
                {wecomQr.url
                  ? <iframe src={wecomQr.url} title="企业微信登录" className="w-full h-[440px] rounded-2xl border border-slate-200 dark:border-white/10 bg-white" scrolling="no" />
                  : <div className="w-52 h-52 rounded-2xl border border-dashed border-slate-300 dark:border-white/10 flex items-center justify-center text-[12px] text-slate-400 px-4">登录页加载失败，请用下方浏览器授权</div>}
                <div className="flex items-center gap-1.5 mt-4 text-[13px] text-slate-500 dark:text-slate-400">
                  <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse"></span> 等待授权中…
                </div>
                <button onClick={() => { if (wecomQr.url) invokeTauri('open_external_url', { url: wecomQr.url }); }} className="mt-4 text-[13px] text-blue-600 dark:text-blue-400 hover:underline">在浏览器打开</button>
                <button onClick={cancel} className="mt-3 px-6 py-2 rounded-full text-[14px] font-semibold bg-slate-100 dark:bg-[#2C2C2E] text-slate-600 dark:text-slate-300">取消</button>
              </div>
            </div>
            ), document.body);
          })()}
          {eipSso && (() => {
            const cancel = () => { invokeTauri('eip_cancel').catch(() => {}); setEipSso(null); setBusyId(null); };
            return createPortal((
            <div className="fixed inset-0 z-[200] flex items-center justify-center p-4" style={{ backgroundColor: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(8px)' }} onClick={cancel}>
              <div className="bg-white dark:bg-[#1C1C1E] rounded-3xl p-7 w-full max-w-[360px] flex flex-col items-center text-center shadow-2xl" onClick={e => e.stopPropagation()}>
                <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-1">登录员工门户（EIP）</h3>
                <p className="text-[13px] text-slate-500 dark:text-slate-400 mb-5">扫码或在浏览器完成 SSO 登录，登录后会自动连接（需公司内网）</p>
                {eipSso.qr
                  ? <img src={eipSso.qr} alt="EIP 登录二维码" className="w-52 h-52 rounded-2xl border border-slate-200 dark:border-white/10 bg-white" />
                  : <div className="w-52 h-52 rounded-2xl border border-dashed border-slate-300 dark:border-white/10 flex items-center justify-center text-[13px] text-slate-500 dark:text-slate-400 bg-slate-50 dark:bg-white/5">二维码生成中…</div>}
                <button onClick={() => { if (eipSso.url) invokeTauri('open_external_url', { url: eipSso.url }); }} className="mt-4 px-6 py-2.5 rounded-full text-[14px] font-semibold bg-blue-600 hover:bg-blue-700 text-white">在浏览器打开登录</button>
                <div className="flex items-center gap-1.5 mt-5 text-[13px] text-slate-500 dark:text-slate-400">
                  <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse"></span> 等待登录中…
                </div>
                <button onClick={cancel} className="mt-3 px-6 py-2 rounded-full text-[14px] font-semibold bg-slate-100 dark:bg-[#2C2C2E] text-slate-600 dark:text-slate-300">取消</button>
              </div>
            </div>
            ), document.body);
          })()}
          {zhidaoSso && (() => {
            const cancel = () => { invokeTauri('zhidao_cancel').catch(() => {}); setZhidaoSso(null); setBusyId(null); };
            return createPortal((
            <div className="fixed inset-0 z-[200] flex items-center justify-center p-4" style={{ backgroundColor: 'rgba(0,0,0,0.5)', backdropFilter: 'blur(8px)' }} onClick={cancel}>
              <div className="bg-white dark:bg-[#1C1C1E] rounded-3xl p-7 w-full max-w-[360px] flex flex-col items-center text-center shadow-2xl" onClick={e => e.stopPropagation()}>
                <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-1">登录知道知识库</h3>
                <p className="text-[13px] text-slate-500 dark:text-slate-400 mb-5">扫码或在浏览器完成 SSO 登录，登录后会自动连接（需公司内网）</p>
                {zhidaoSso.qr
                  ? <img src={zhidaoSso.qr} alt="知道登录二维码" className="w-52 h-52 rounded-2xl border border-slate-200 dark:border-white/10 bg-white" />
                  : <div className="w-52 h-52 rounded-2xl border border-dashed border-slate-300 dark:border-white/10 flex items-center justify-center text-[13px] text-slate-500 dark:text-slate-400 bg-slate-50 dark:bg-white/5">二维码生成中…</div>}
                <button onClick={() => { if (zhidaoSso.url) invokeTauri('open_external_url', { url: zhidaoSso.url }); }} className="mt-4 px-6 py-2.5 rounded-full text-[14px] font-semibold bg-blue-600 hover:bg-blue-700 text-white">在浏览器打开登录</button>
                <div className="flex items-center gap-1.5 mt-5 text-[13px] text-slate-500 dark:text-slate-400">
                  <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse"></span> 等待登录中…
                </div>
                <button onClick={cancel} className="mt-3 px-6 py-2 rounded-full text-[14px] font-semibold bg-slate-100 dark:bg-[#2C2C2E] text-slate-600 dark:text-slate-300">取消</button>
              </div>
            </div>
            ), document.body);
          })()}
          <div className="flex-1 flex flex-col bg-white dark:bg-[#131314] text-slate-900 dark:text-white transition-colors duration-300 font-sans overflow-y-auto custom-scrollbar p-4 sm:p-6 lg:p-10">

            {/* Header */}
            <header className="z-30 bg-white/80 dark:bg-[#131314]/80 backdrop-blur-2xl transition-colors">
              <div className="max-w-[1400px] mx-auto border-b border-slate-200/50 pb-6 dark:border-white/10">
                <div className="flex items-center justify-between gap-4">
                  <h1 className="shrink-0 text-[26px] font-normal tracking-tight">工具商店</h1>
                  <div className={`ml-8 flex min-w-0 flex-1 items-center justify-end gap-3 ${installedOnly ? 'hidden' : ''}`}>
                    <div className="relative group min-w-0 max-w-[520px] flex-1">
                      <Search className="absolute left-3.5 top-1/2 -translate-y-1/2 text-[#8E8E93] group-focus-within:text-blue-500 transition-colors" size={18} />
                      <input
                        data-testid="tool-store-search"
                        type="text"
                        placeholder="搜索连接器、skill、插件等"
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                        className="h-9 w-full rounded-[14px] border-none bg-slate-100 pl-10 pr-4 text-[13px] font-normal outline-none transition-all placeholder:text-[#8E8E93] focus:ring-0 dark:bg-[rgba(118,118,128,.24)] text-slate-900 dark:text-white"
                      />
                    </div>
                    <div className="flex shrink-0 items-center justify-end gap-3">
                      <div className="flex h-9 shrink-0 items-center rounded-full bg-slate-100 p-1 shadow-sm dark:bg-[#2C2C2E]">
                        {[{ key: 'card', label: '卡片', Icon: IconGrid }, { key: 'list', label: '列表', Icon: IconList }].map(seg => (
                          <button key={seg.key} onClick={() => { setViewMode(seg.key); setInstalledOnly(false); setSearchQuery(''); setActiveCategory('all'); }}
                            className={`inline-flex h-7 items-center rounded-full px-3 text-[13px] font-semibold transition-colors whitespace-nowrap ${
                              viewMode === seg.key
                                ? 'bg-white text-slate-900 shadow-sm dark:bg-[#3A3A3C] dark:text-white'
                                : 'text-slate-700 hover:bg-slate-200 dark:text-white dark:hover:bg-[#3A3A3C]'
                            }`}>
                            <seg.Icon size={14} className="mr-2 opacity-70" />
                            {seg.label}
                          </button>
                        ))}
                      </div>
                      <button onClick={() => { setViewMode('list'); setInstalledOnly(true); setSearchQuery(''); }} title="我的工具 · 已安装"
                        className="inline-flex h-9 items-center rounded-full bg-slate-100 px-4 text-[13px] font-semibold shadow-sm transition-colors hover:bg-slate-200 dark:bg-[#2C2C2E] dark:text-white dark:hover:bg-[#3A3A3C]">
                        <User size={14} className="mr-2 opacity-70" />
                        <span>我的工具</span>
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            </header>

            {/* Main scrollable area */}
            <main className="flex-1">
              <div className={`max-w-[1400px] mx-auto ${isCard ? 'py-8 space-y-12' : 'pt-5 pb-8 space-y-6'}`}>

                {/* Featured carousel */}
                <section
                  hidden={!showFeaturedCollections}
                  className={`relative group/featured ${showFeaturedCollections ? '' : 'hidden'}`}
                  aria-hidden={!showFeaturedCollections}
                  onMouseEnter={() => setIsFeaturedHovered(true)}
                  onMouseLeave={() => setIsFeaturedHovered(false)}
                >
                    <div className="flex items-end justify-between mb-5">
                      <h2 className="text-[13px] font-bold uppercase tracking-wider text-[#3C3C43]/60 dark:text-[#EBEBF5]/60">精选连接器</h2>
                    </div>

                    <button
                      onClick={(e) => { e.stopPropagation(); handleScrollFeatured('left'); }}
                      className="absolute -left-5 top-[55%] -translate-y-1/2 z-20 w-12 h-12 rounded-full bg-white/90 dark:bg-slate-800/90 backdrop-blur-md shadow-[0_4px_20px_rgba(0,0,0,0.15)] dark:shadow-[0_4px_20px_rgba(0,0,0,0.5)] flex items-center justify-center text-slate-800 dark:text-slate-200 opacity-0 group-hover/featured:opacity-100 hover:scale-110 transition-all border border-slate-200 dark:border-slate-700 hidden md:flex"
                    >
                      <ChevronLeft size={26} />
                    </button>
                    <button
                      onClick={(e) => { e.stopPropagation(); handleScrollFeatured('right'); }}
                      className="absolute -right-5 top-[55%] -translate-y-1/2 z-20 w-12 h-12 rounded-full bg-white/90 dark:bg-slate-800/90 backdrop-blur-md shadow-[0_4px_20px_rgba(0,0,0,0.15)] dark:shadow-[0_4px_20px_rgba(0,0,0,0.5)] flex items-center justify-center text-slate-800 dark:text-slate-200 opacity-0 group-hover/featured:opacity-100 hover:scale-110 transition-all border border-slate-200 dark:border-slate-700 hidden md:flex"
                    >
                      <ChevronRight size={26} />
                    </button>

                    <div
                      ref={featuredScrollRef}
                      className="flex gap-6 overflow-x-auto snap-x snap-mandatory pb-6 no-scrollbar relative"
                      style={{ scrollbarWidth: 'none', maskImage: 'linear-gradient(to right,#000 0,#000 92%,transparent 100%)', WebkitMaskImage: 'linear-gradient(to right,#000 0,#000 92%,transparent 100%)' }}
                    >
                      {/* H3C 集团内部工具合集 —— 精选第一张，点击展开详情 */}
                      {visibleInternalTools.length > 0 && (
                        <div
                          onClick={() => setShowH3cModal(true)}
                          className="relative min-w-[320px] md:min-w-[400px] h-[440px] max-sm:h-[380px] rounded-[32px] snap-start shrink-0 overflow-hidden cursor-pointer group shadow-sm hover:shadow-xl transition-all duration-500"
                        >
                          <img src="assets/h3c-banner.jpg" alt="" loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />
                          <div className="absolute inset-0 bg-gradient-to-b from-black/45 via-black/15 to-black/70" />
                          <div className="relative p-8 text-white">
                            <div className="flex items-center justify-between mb-4">
                              <span className="text-[11px] font-bold text-white/85 uppercase tracking-[0.15em] bg-white/15 backdrop-blur px-2.5 py-1 rounded-full">集团内部 · 员工专属</span>
                              <span className="text-[12px] font-black text-white border border-white/45 rounded-md px-1.5 py-0.5">H3C</span>
                            </div>
                            <h3 className="text-[30px] font-bold tracking-tight leading-tight drop-shadow-sm">H3C 办公生态<br/>一键接入</h3>
                            <p className="text-white/85 text-[15px] font-medium mt-3 max-w-[88%]">以你本人 SSO 身份直连集团内部系统，全程不填 key</p>
                          </div>
                          <div className="absolute bottom-6 left-6 right-6">
                            <div className="bg-white/20 dark:bg-black/40 backdrop-blur-3xl border border-white/20 dark:border-white/10 rounded-2xl p-4 flex items-center justify-between shadow-lg transition-transform group-hover:-translate-y-1">
                              <div className="flex items-center gap-3">
                                <div className="w-12 h-12 rounded-[13px] flex items-center justify-center text-white shadow-inner" style={{ background: 'linear-gradient(135deg,#ff2a43,#a30010)' }}>
                                  <Briefcase size={22} strokeWidth={1.6} />
                                </div>
                                <div>
                                  <h4 className="text-[14px] font-bold text-white drop-shadow-sm">H3C 集团内部工具</h4>
                                  <p className="text-[12px] text-white/70 mt-0.5">{visibleInternalTools.length} 个工具 · 需内网</p>
                                </div>
                              </div>
                              <span className="text-white font-bold text-[14px] drop-shadow-sm">查看</span>
                            </div>
                          </div>
                        </div>
                      )}
                      {tsFeaturedCollections.map((collection) => {
                        const featTool = tools.find(a => a.id === collection.featuredToolId);
                        if (featTool && !isToolVisibleOnPlatform(featTool)) return null;
                        return (
                          <div
                            key={collection.id}
                            onClick={() => setSelectedTool(featTool)}
                            className="relative min-w-[320px] md:min-w-[400px] h-[440px] max-sm:h-[380px] rounded-[32px] snap-start shrink-0 overflow-hidden cursor-pointer group shadow-sm hover:shadow-xl dark:shadow-none border border-slate-200/50 dark:border-white/10 transition-all duration-500"
                          >
                            {collection.img
                              ? <img src={collection.img} alt="" loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />
                              : <div className={`absolute inset-0 ${collection.bg} transition-transform duration-700 group-hover:scale-105`} />}
                            <div className="absolute inset-0 bg-gradient-to-b from-black/40 via-transparent to-black/60" />

                            <div className="absolute top-0 left-0 w-full p-8 text-white z-10">
                              <span className="text-xs font-bold text-white/70 uppercase tracking-widest mb-1.5 block">{collection.label}</span>
                              <h3 className="text-[28px] font-bold tracking-tight leading-tight mb-3 text-white drop-shadow-sm">{collection.title}</h3>
                              <p className="text-white/80 text-[15px] font-medium leading-relaxed max-w-[90%] drop-shadow-sm">{collection.subtitle}</p>
                            </div>

                            {featTool && (
                              <div className="absolute bottom-6 left-6 right-6 z-10">
                                <div className="bg-white/20 dark:bg-black/40 backdrop-blur-3xl border border-white/20 dark:border-white/10 rounded-2xl p-4 flex items-center gap-4 transition-transform group-hover:-translate-y-1">
                                  <TsToolIcon tool={featTool} className="h-14 w-14 flex-shrink-0 rounded-[14px] shadow-inner" imageClassName="h-9 w-9" fallbackSize={26} />
                                  <div className="flex-1 min-w-0">
                                    <h4 className="text-base font-bold text-white truncate drop-shadow-sm">{featTool.title}</h4>
                                    <p className="text-xs text-white/70 truncate flex items-center gap-1.5">
                                      <Cpu size={12} /> {featTool.type}
                                    </p>
                                  </div>
                                  <PlatformToolAction tool={featTool} busy={busyId === featTool.backendId} onAction={handleAction} />
                                </div>
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                </section>

                {/* Category filter + tool list */}
                <section>
                  <div className={`flex flex-col gap-4 mb-6 ${!isCard && !installedOnly && !searching ? '' : 'sm:flex-row sm:items-end justify-between'} ${isCard ? '' : 'pb-5'}`}>
                    {(isCard || installedOnly || searching) && (
                      <div className="flex items-center gap-3">
                        {installedOnly && (
                          <button onClick={() => { setInstalledOnly(false); setViewMode('card'); }} title="返回商店"
                            className="w-9 h-9 rounded-full bg-slate-100 dark:bg-white/10 hover:bg-slate-200 dark:hover:bg-white/20 flex items-center justify-center text-slate-600 dark:text-slate-300 transition-colors shrink-0">
                            <ChevronLeft size={20} />
                          </button>
                        )}
                        <h2 className="text-[13px] font-bold uppercase tracking-wider text-[#3C3C43]/60 dark:text-[#EBEBF5]/60">
                          {isCard ? (searchQuery ? '检索结果' : '独家技能') : (installedOnly ? '我的工具' : '检索结果')}
                        </h2>
                      </div>
                    )}
                    {!isCard && !installedOnly && (
                      <div className="flex gap-2 overflow-x-auto no-scrollbar scroll-smooth">
                        {visibleCategories.map((cat) => {
                          const isActive = activeCategory === cat.id;
                          return (
                            <button
                              key={cat.id}
                              onClick={() => { setActiveCategory(cat.id); setInstalledOnly(false); }}
                              className="h-9 whitespace-nowrap shrink-0 text-[13px] px-3.5 rounded-full font-semibold transition-colors"
                              style={isActive
                                ? { background: isDark ? '#fff' : '#3A3A3C', color: isDark ? '#000' : '#fff' }
                                : { background: isDark ? '#2C2C2E' : '#F2F2F7', color: isDark ? '#fff' : '#000' }}
                            >
                              {cat.label}
                            </button>
                          );
                        })}
                      </div>
                    )}
                  </div>

                  {filteredTools.length > 0 ? (
                    (isSkillTab && !searching) ? (
                    <div key="tool-store-card-grid" className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-6 pb-7">
                      {filteredTools.map((tool) => {
                        const v = tool.todayVariant || 'fallback';
                        const bar = (
                          <div className="absolute bottom-4 left-4 right-4 bg-white/92 dark:bg-[#1c1c1e]/92 backdrop-blur-xl rounded-2xl p-3 flex items-center gap-3 shadow-lg">
                            <div className={`w-12 h-12 rounded-[13px] ${tool.color} flex items-center justify-center text-white shadow-inner shrink-0`}><tool.icon size={22} strokeWidth={1.6} /></div>
                            <div className="flex-1 min-w-0"><h4 className="text-[15px] font-bold truncate text-slate-900 dark:text-white">{tool.title}</h4><p className="text-[12px] text-slate-500 dark:text-slate-400 truncate">{tool.subtitle}</p></div>
                            <PlatformToolAction tool={tool} busy={busyId === tool.backendId} onAction={handleAction} />
                          </div>
                        );
                        return (
                          <div key={`card-${tool.id}`} onClick={() => setSelectedTool(tool)} className="today-card group relative w-full h-[440px] max-sm:h-[400px] rounded-[28px] overflow-hidden cursor-pointer shadow-[0_14px_40px_-18px_rgba(15,23,42,0.35)] transition-all duration-500 hover:shadow-[0_28px_64px_-24px_rgba(15,23,42,0.45)] hover:-translate-y-1">
                            {v === 'light' ? (
                              <>
                                <div className="p-6"><p className="text-slate-500 dark:text-slate-400 text-[13px] font-bold uppercase tracking-[0.12em] mb-1.5">{tool.todayLabel}</p><h2 className="text-[30px] font-bold leading-[1.1] tracking-tight whitespace-pre-line text-slate-900 dark:text-white">{tool.todayTitle}</h2></div>
                                {tool.todayImg && <img src={tool.todayImg} loading="eager" decoding="async" className="absolute bottom-0 left-0 w-full h-[62%] object-cover" />}
                                {bar}
                              </>
                            ) : v === 'drama' ? (
                              <>
                                {tool.todayImg && <img src={tool.todayImg} loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />}
                                <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/25 to-black/40" />
                                <div className="absolute top-0 left-0 p-6"><p className="text-white/85 text-[13px] font-bold uppercase tracking-[0.12em]">{tool.todayLabel}</p></div>
                                <div className="absolute bottom-0 left-0 p-6"><h2 className="text-white text-[32px] font-bold leading-[1.05] tracking-tight drop-shadow mb-2 whitespace-pre-line">{tool.todayTitle}</h2><p className="text-white/85 text-[14px] font-medium">{tool.subtitle}</p></div>
                              </>
                            ) : v === 'appbar' ? (
                              <>
                                {tool.todayImg && <img src={tool.todayImg} loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />}
                                <div className="absolute inset-0 bg-gradient-to-b from-black/55 via-black/10 to-black/35" />
                                <div className="absolute top-0 left-0 p-6"><p className="text-white/85 text-[13px] font-bold uppercase tracking-[0.12em] mb-1.5">{tool.todayLabel}</p><h2 className="text-white text-[30px] font-bold leading-[1.1] tracking-tight drop-shadow whitespace-pre-line">{tool.todayTitle}</h2></div>
                                {bar}
                              </>
                            ) : v === 'appimg' ? (
                              <>
                                {tool.todayImg && <img src={tool.todayImg} loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />}
                                <div className="absolute inset-0 bg-gradient-to-b from-black/50 via-black/5 to-black/80" />
                                <div className="absolute top-0 left-0 p-6"><p className="text-white/85 text-[13px] font-bold uppercase tracking-[0.12em] mb-1.5">{tool.todayLabel}</p><h2 className="text-white text-[30px] font-bold leading-[1.1] tracking-tight drop-shadow whitespace-pre-line">{tool.todayTitle}</h2></div>
                                <div className="absolute bottom-0 left-0 right-0 p-6 flex items-center gap-3">
                                  <div className={`w-12 h-12 rounded-[13px] ${tool.color} flex items-center justify-center text-white text-xl shadow-lg shrink-0`}><tool.icon size={22} strokeWidth={1.6} /></div>
                                  <div className="flex-1 min-w-0"><h4 className="text-white text-[15px] font-bold truncate drop-shadow">{tool.title}</h4><p className="text-white/80 text-[12px] truncate drop-shadow">{tool.subtitle}</p></div>
                                  {tool.builtin
                                    ? <span className="text-white text-[12px] font-bold bg-white/20 backdrop-blur px-3 py-1 rounded-full shrink-0">内置</span>
                                    : <PlatformToolAction tool={tool} busy={busyId === tool.backendId} onAction={handleAction} />}
                                </div>
                              </>
                            ) : v === 'fallback' ? (
                              <>
                                <div className={`absolute inset-0 ${tool.color}`} />
                                <div className="absolute inset-0 bg-gradient-to-b from-black/25 via-transparent to-black/55" />
                                <div className="absolute top-6 left-6 right-6"><p className="text-white/85 text-[13px] font-bold uppercase tracking-[0.12em] mb-1.5">技能</p><h2 className="text-white text-[28px] font-bold leading-[1.1] tracking-tight drop-shadow">{tool.title}</h2></div>
                                <tool.icon size={120} strokeWidth={1} className="absolute -bottom-3 -right-1 text-white/15" />
                                {bar}
                              </>
                            ) : (
                              <>
                                {tool.todayImg && <img src={tool.todayImg} loading="eager" decoding="async" className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105" />}
                                <div className="absolute inset-0 bg-gradient-to-b from-black/55 via-transparent to-black/70" />
                                <div className="absolute top-0 left-0 p-6"><p className="text-white/85 text-[13px] font-bold uppercase tracking-[0.12em] mb-1.5">{tool.todayLabel}</p><h2 className="text-white text-[30px] font-bold leading-[1.1] tracking-tight drop-shadow whitespace-pre-line">{tool.todayTitle}</h2></div>
                                <p className="absolute bottom-5 left-6 right-6 text-white/90 text-[14px] font-medium leading-snug">{tool.subtitle}</p>
                              </>
                            )}
                          </div>
                        );
                      })}
                    </div>
                    ) : (
                    <div key="tool-store-list-grid" className="grid gap-x-10 gap-y-0" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))' }}>
                      {filteredTools.map((tool) => (
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
                                <span className="text-[10px] font-semibold text-sky-700 dark:text-sky-300 bg-sky-100 dark:bg-sky-500/15 px-1.5 py-0.5 rounded-full">内网直连</span>
                              ) : tool.authRequired && (
                                <span className="text-[10px] text-amber-500/80 dark:text-amber-400/80 flex items-center gap-0.5">
                                  <Zap size={10} /> 需密钥
                                </span>
                              )}
                            </div>
                          </div>
                          <div className="flex flex-col items-center justify-center gap-1 pl-2">
                            {(() => {
                              const cf = tool.feishuCli ? feishuFlow : tool.wecomCli ? wecomFlow : tool.dingtalkCli ? dingtalkFlow : null;
                              return (externalAuthAvailable && cf && (cf.phase === 'running' || cf.phase === 'qr'))
                                ? <FeishuMini flow={cf} onClick={() => setSelectedTool(tool)} />
                                : <PlatformToolAction tool={tool} busy={busyId === tool.backendId} onAction={handleAction} />;
                            })()}
                          </div>
                        </div>
                      ))}
                    </div>
                    )
                  ) : (
                    <div className="py-24 text-center flex flex-col items-center">
                      <div className="w-16 h-16 mb-4 rounded-full bg-slate-100 dark:bg-slate-800 flex items-center justify-center text-slate-400">
                        {isSkillTab ? <Package size={28} /> : <Server size={28} />}
                      </div>
                      <h3 className="text-xl font-semibold text-slate-800 dark:text-slate-200 mb-2">{searching ? '未找到匹配的工具' : (installedOnly ? '还没有已安装的工具' : (isSkillTab ? '没有技能' : '未检索到工具'))}</h3>
                      <p className="text-slate-500 dark:text-slate-400">{searching ? '换个关键词试试，或检查一下拼写。' : (installedOnly ? (canMutateToolStore ? '去商店安装连接器或技能后，会出现在这里。' : '桌面端尚未安装工具或技能。') : (isSkillTab ? (canMutateToolStore ? '点右上「上传技能包」导入 zip。' : '当前没有可浏览的技能。') : '请尝试修改搜索词或查阅 API 开发文档。'))}</p>
                    </div>
                  )}
                </section>

              </div>
            </main>
          </div>

          {/* H3C 合集详情 — 同 selectedTool,portal 到 body 避开 backdrop-blur 包含块 */}
          {showH3cModal && createPortal((
            <div
              className="fixed inset-0 z-[60] flex items-center justify-center p-4 sm:p-6 bg-slate-900/50 dark:bg-black/60 backdrop-blur-md"
              onClick={() => setShowH3cModal(false)}
            >
              <div
                className="ts-modal-in relative w-full max-w-[560px] max-h-[90vh] overflow-y-auto no-scrollbar bg-white dark:bg-[#1C1C1E] rounded-[32px] shadow-2xl border border-slate-200/50 dark:border-white/10"
                onClick={(e) => e.stopPropagation()}
              >
                <div className="relative h-[200px] shrink-0">
                  <img src="assets/h3c-banner.jpg" alt="" className="absolute inset-0 w-full h-full object-cover" />
                  <div className="absolute inset-0 bg-gradient-to-b from-black/40 via-black/15 to-black/60" />
                  <button onClick={() => setShowH3cModal(false)} className="absolute top-4 right-4 w-9 h-9 rounded-full bg-black/30 backdrop-blur text-white flex items-center justify-center z-10 text-[18px] leading-none">✕</button>
                  <div className="absolute bottom-0 p-7 text-white">
                    <span className="text-[11px] font-bold text-white/85 uppercase tracking-[0.15em] bg-white/15 backdrop-blur px-2.5 py-1 rounded-full">集团内部 · 员工专属</span>
                    <h2 className="text-[28px] font-bold tracking-tight drop-shadow mt-2.5">H3C 办公生态 一键接入</h2>
                  </div>
                </div>
                <div className="px-6 py-6">
                  <p className="text-[14px] text-slate-600 dark:text-slate-300 leading-relaxed mb-6">以你<b>本人 SSO 身份</b>直连 H3C 集团内部系统，浏览器 / 扫码登录、<b>全程不填 key</b>；数据经公司内网、不出集团。<span className="text-slate-400">需连接公司内网使用。</span></p>
                  <h3 className="text-[16px] font-bold mb-4 text-slate-900 dark:text-white">包含的工具 ({visibleInternalTools.length})</h3>
                  <div className="space-y-4">
                    {visibleInternalTools.map(t => (
                      <div key={t.id} className="flex items-start gap-4">
                        <TsToolIcon tool={t} className="h-14 w-14 flex-shrink-0 rounded-[14px] border border-black/5 shadow-sm dark:border-white/5" />
                        <div className="flex-1 min-w-0 border-b border-slate-100 dark:border-white/5 pb-4">
                          <div className="flex justify-between items-center gap-2 mb-1">
                            <h4 className="text-[15px] font-bold truncate text-slate-900 dark:text-white">{t.title}</h4>
                            <PlatformToolAction tool={t} busy={busyId === t.backendId} onAction={handleAction} />
                          </div>
                          <p className="text-[12px] text-slate-500 dark:text-slate-400 leading-relaxed">{t.subtitle}</p>
                        </div>
                      </div>
                    ))}
                  </div>
                  <div className="mt-6 pt-4 border-t border-slate-100 dark:border-white/10 text-center">
                    <p className="text-[11px] text-slate-400 dark:text-slate-500">需连接公司内网 · 首次点「连接」浏览器 / 扫码 SSO 登录，不填 key</p>
                  </div>
                </div>
              </div>
            </div>
          ), document.body)}
          {/* Detail modal — portal 到 body：否则被主内容区 backdrop-blur 祖先造的包含块困住，
              fixed inset-0 只盖住右侧内容区、盖不到左侧栏。portal 后蒙层铺满整个视口。 */}
          {selectedTool && createPortal((
            <div
              className="fixed inset-0 z-[90] flex items-center justify-center p-4 sm:p-6 bg-slate-900/40 dark:bg-black/60 backdrop-blur-md transition-all duration-300"
              onClick={() => setSelectedTool(null)}
            >
              <div
                className="ts-modal-in relative w-full max-w-2xl bg-white dark:bg-[#1C1C1E] rounded-[32px] shadow-2xl overflow-hidden flex flex-col max-h-[90vh] border border-slate-200/50 dark:border-white/10"
                onClick={(e) => e.stopPropagation()}
              >
                <div className="absolute top-0 right-0 w-full px-6 py-5 flex items-center justify-end z-20 pointer-events-none">
                  <button
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
                      <div className="flex items-center gap-4">
                        {(() => { const sf = selectedTool.feishuCli ? feishuFlow : selectedTool.wecomCli ? wecomFlow : selectedTool.dingtalkCli ? dingtalkFlow : null; return (externalAuthAvailable && sf && (sf.phase === 'running' || sf.phase === 'qr'))
                          ? <FeishuMini flow={sf} onClick={() => {}} />
                          : <PlatformToolAction tool={selectedTool} busy={busyId === selectedTool.backendId} onAction={handleAction} size="lg" />; })()}
                      </div>
                    </div>
                  </div>

                  <div className="flex items-center justify-between py-5 mb-8 border-y border-slate-100 dark:border-white/5 overflow-x-auto no-scrollbar gap-8">
                    <div className="flex flex-col flex-shrink-0">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">接口类型</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.type}</span>
                      <span className="text-[12px] text-slate-400 mt-1 flex items-center gap-1"><Server size={12}/> 官方支持</span>
                    </div>
                    <div className="w-px h-12 bg-slate-200 dark:bg-slate-800 flex-shrink-0" />
                    <div className="flex flex-col flex-shrink-0">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">当前版本</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.version}</span>
                      <span className="text-[12px] text-slate-400 mt-1">稳定版发布</span>
                    </div>
                    <div className="w-px h-12 bg-slate-200 dark:bg-slate-800 flex-shrink-0" />
                    <div className="flex flex-col flex-shrink-0 pr-4">
                      <span className="text-[11px] text-slate-500 font-semibold uppercase tracking-wider mb-1">平均延迟</span>
                      <span className="text-xl font-bold text-slate-800 dark:text-slate-200">{selectedTool.latency}</span>
                      <span className="text-[12px] text-slate-400 mt-1 flex items-center gap-1"><Globe size={12}/> 全球加速</span>
                    </div>
                  </div>

                  {externalAuthAvailable && selectedTool.feishuCli && feishuFlow && (
                    <FeishuFlowCard flow={feishuFlow} onRetry={feishuRetry} onCancel={feishuResetFlow} />
                  )}
                  {externalAuthAvailable && selectedTool.wecomCli && wecomFlow && (
                    <FeishuFlowCard flow={wecomFlow} steps={WECOM_STEPS} name="企业微信" twoStep={false} onRetry={wecomRetry} onCancel={wecomResetFlow} />
                  )}
                  {externalAuthAvailable && selectedTool.dingtalkCli && dingtalkFlow && (
                    <FeishuFlowCard flow={dingtalkFlow} steps={DINGTALK_STEPS} name="钉钉" twoStep={false} onRetry={dingtalkRetry} onCancel={dingtalkResetFlow} />
                  )}
                  {selectedTool.feishuCli && feishuConnected && !feishuFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">已连接飞书 · 官方技能已启用，可直接对它下指令</span>
                    </div>
                  )}
                  {selectedTool.wecomCli && wecomConnected && !wecomFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">已连接企业微信 · 官方技能已启用，可直接对它下指令</span>
                    </div>
                  )}
                  {selectedTool.dingtalkCli && dingtalkConnected && !dingtalkFlow && (
                    <div className="mb-8 flex items-center gap-3 p-4 rounded-2xl bg-emerald-50 dark:bg-emerald-500/10 border border-emerald-200 dark:border-emerald-500/30">
                      <span className="w-8 h-8 rounded-lg bg-emerald-500 grid place-items-center text-white flex-shrink-0">✓</span>
                      <span className="text-emerald-700 dark:text-emerald-300 font-semibold text-[15px]">已连接钉钉 · 官方技能已启用，可直接对它下指令</span>
                    </div>
                  )}

                  <div>
                    <h3 className="text-[19px] font-bold text-slate-900 dark:text-white mb-4">关于此能力</h3>
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

export { FEISHU_STEPS, WECOM_STEPS, DINGTALK_STEPS, FeishuStepIcon, FeishuBar, FeishuFlowCard, FeishuMini, feishuConn, ensureFeishuListeners, wecomConn, ensureWecomListeners, dingtalkConn, ensureDingtalkListeners, TsAlert, TsConfigDialog, TsObsidianGuide, ToolStoreView };
