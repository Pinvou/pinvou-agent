import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ArrowLeft, BookOpen, Brain, Check, ChevronDown, ChevronRight, ClipboardList, Copy, Edit2, Mic, Package, Paperclip, Send, Sparkles, StopCircle, Trash2, X, Zap } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ArtifactsPanel } from '../artifacts/ArtifactsPanel.jsx';
import { AppIcon, DEPT_ORDER, deptColor, deptLabelFor, personaText } from '../personas/Personas.jsx';
import { ComposerModeMenu, ComposerModelSelector, ComposerToolMenu } from '../settings/SettingsView.jsx';
import { ArtifactCard, tsToolsData } from '../tools/tool-common.jsx';
import { CarefulBlockedCard, PlanCard, PlanStuckCard, ToolCard, UserInputCard, cardBtnCls } from '../tools/tool-renderers.jsx';

const ToolWelcomeCard = ({ toolId, theme, onSend }) => {
      const isDark = theme === 'dark';
      const [hovered, setHovered] = useState(null);
      const tool = tsToolsData.find(t => t.backendId === toolId);
      if (!tool || !tool.welcomeQueries) return null;
      const ToolIcon = tool.icon || Sparkles;
      return (
        <div className="flex justify-start">
          <div className={`max-w-[800px] w-full rounded-[2rem] overflow-hidden border transition-all ${
            isDark ? 'bg-[#1E1F20] border-[#3A3A3C]/60' : 'bg-white border-slate-100 shadow-lg shadow-slate-200/30'
          }`}>
            <div className={`relative p-5 border-b flex items-center gap-3.5 ${
              isDark ? 'bg-[#1E1F20] border-[#3A3A3C]/60' : 'bg-gradient-to-b from-blue-50/80 to-white border-slate-100'
            }`}>
              <div className="bg-gradient-to-tr from-blue-600 to-indigo-500 p-2.5 rounded-xl shadow-lg shadow-blue-500/30">
                <ToolIcon size={22} className="text-white" />
              </div>
              <div>
                <div className={`text-[1.05rem] font-bold tracking-tight ${isDark ? 'text-slate-100' : 'text-slate-800'}`}>{tool.title}</div>
                <div className={`flex items-center text-xs mt-0.5 ${isDark ? 'text-slate-400' : 'text-slate-500'}`}>
                  <span className="relative flex h-2 w-2 mr-2">
                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"></span>
                    <span className="relative inline-flex rounded-full h-2 w-2 bg-emerald-500"></span>
                  </span>
                  系统已就绪
                </div>
              </div>
            </div>
            <div className="p-5">
              <p className={`leading-relaxed text-[15px] ${isDark ? 'text-slate-300' : 'text-slate-600'}`}>
                {tool.desc.split('。')[0]}。自然语言提问即可。
              </p>
              <div className="flex items-center my-5">
                <div className={`flex-grow h-px ${isDark ? 'bg-gradient-to-r from-transparent via-[#3A3A3C] to-transparent' : 'bg-gradient-to-r from-transparent via-slate-200 to-transparent'}`}></div>
                <span className={`px-4 text-[11px] uppercase tracking-wider font-semibold flex items-center gap-1.5 ${isDark ? 'text-slate-500' : 'text-slate-400'}`}>
                  <Sparkles size={13} />
                  <span>试试问我</span>
                </span>
                <div className={`flex-grow h-px ${isDark ? 'bg-gradient-to-r from-transparent via-[#3A3A3C] to-transparent' : 'bg-gradient-to-r from-transparent via-slate-200 to-transparent'}`}></div>
              </div>
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                {tool.welcomeQueries.map((q, i) => (
                  <button
                    key={i}
                    onMouseEnter={() => setHovered(i)}
                    onMouseLeave={() => setHovered(null)}
                    onClick={() => onSend && onSend(q)}
                    className={`group relative flex items-center justify-between p-3 rounded-2xl border text-left transition-all duration-200 ${
                      hovered === i
                        ? (isDark ? 'border-blue-500/30 bg-blue-500/10 shadow-sm' : 'border-blue-200/80 bg-blue-50/50 shadow-sm')
                        : (isDark ? 'border-[#3A3A3C]/50 bg-[#2A2B2D]/30 hover:border-[#555]' : 'border-slate-200/60 bg-slate-50/30 hover:border-blue-200')
                    }`}
                  >
                    <span className={`text-sm font-medium transition-colors ${
                      hovered === i ? (isDark ? 'text-blue-300' : 'text-blue-700') : (isDark ? 'text-slate-300' : 'text-slate-700')
                    }`}>{q}</span>
                    <ChevronRight size={15} className={`transition-all duration-200 ${
                      hovered === i ? (isDark ? 'text-blue-400 opacity-100' : 'text-blue-500 opacity-100') : 'opacity-0 -translate-x-2'
                    }`} />
                  </button>
                ))}
              </div>
            </div>
          </div>
        </div>
      );
    };

    // 输入框底栏:知识库挂载选择器(与 ComposerModelSelector/ComposerToolMenu 同款 pill,
    // class 暗色策略)。给当前对话挂一个知识集(会话级粘连),挂上后每条消息发送前后端自动
    // 检索注入相关片段(commands::chat)。草稿态选集会经 bridge.mountCollection 先物化 session。
    const ComposerKbSelector = ({ t, bs, compact }) => {
      const [open, setOpen] = useState(false);
      const [collections, setCollections] = useState(null); // null=未加载
      const [installed, setInstalled] = useState(null); // embedding 模型是否已装:null=未知(不闪 gate,mock/旧后端当已装)
      const mountedId = (bs && bs.mountedCollection != null) ? bs.mountedCollection : null;

      const loadList = async () => {
        if (!bridge.available || !bridge.listCollections) { setCollections([]); return; }
        try { setCollections((await bridge.listCollections()) || []); }
        catch (e) { setCollections([]); }
      };
      const refreshInstalled = async () => {
        if (!bridge.available || !bridge.kbModelStatus) { setInstalled(true); return; } // mock/旧后端不 gate
        try { const m = await bridge.kbModelStatus(); setInstalled(m ? !!m.installed : true); }
        catch (e) { setInstalled(true); }
      };
      useEffect(() => { refreshInstalled(); }, []);
      // 下载部署完成后 bs.kbModelSetup.status.installed 变 true → 立即开门,免重开菜单。
      const setupInstalled = !!(bs && bs.kbModelSetup && bs.kbModelSetup.status && bs.kbModelSetup.status.installed);
      useEffect(() => { if (setupInstalled) setInstalled(true); }, [setupInstalled]);
      // 已挂载但还没列表 → 拉一次用于显示名字。
      useEffect(() => {
        if (mountedId != null && collections === null) loadList();
      }, [mountedId]);

      const mounted = (collections || []).find(c => c.id === mountedId) || null;
      const mountedName = mounted ? mounted.name : (mountedId != null ? ('#' + mountedId) : null);
      const active = mountedId != null;
      const modelMissing = installed === false; // 仅"明确未装"才门控;未知/已装都放行

      function toggle() { const next = !open; setOpen(next); if (next) { refreshInstalled(); if (collections === null) loadList(); } }
      function pick(id) { if (modelMissing) return; setOpen(false); if (id !== mountedId && bridge.available) bridge.mountCollection(id); }
      function unmount() { setOpen(false); if (bridge.available) bridge.unmountCollection(); }

      return (
        <div className="relative">
          <button onClick={toggle} title={modelMissing ? t.kbMountNoModel : (active ? mountedName : t.kbMountTitle)}
            className={`relative shrink-0 flex items-center justify-center transition-colors border ${compact ? 'w-9 h-9 rounded-full' : 'gap-1.5 px-2.5 py-1.5 rounded-xl text-[13px] font-semibold'} ${active
              ? 'bg-[#E8F0FE] dark:bg-[#1A3A5C] text-[#1A73E8] dark:text-[#A8C7FA] border-[#1A73E8]/20 dark:border-[#A8C7FA]/25'
              : modelMissing
                ? 'bg-gray-100 dark:bg-white/5 text-gray-400 dark:text-gray-600 border-black/[0.04] dark:border-white/5 opacity-70'
                : 'bg-gray-100 dark:bg-white/5 hover:bg-gray-200 dark:hover:bg-white/10 text-gray-700 dark:text-gray-200 border-black/[0.04] dark:border-white/5'}`}>
            <BookOpen size={compact ? 18 : 14} className="opacity-70 shrink-0" />
            {!compact && <span className="max-w-[140px] truncate">{active ? mountedName : t.kbMount}</span>}
            {!compact && <ChevronDown size={14} className="opacity-50 shrink-0" />}
            {compact && active && <span className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[#1A73E8] dark:bg-[#A8C7FA] ring-2 ring-white dark:ring-[#161618]"></span>}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-64 max-h-[340px] overflow-y-auto bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {modelMissing ? (
                  <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">{t.kbMountNoModel}</div>
                ) : collections === null ? (
                  <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">…</div>
                ) : collections.length === 0 ? (
                  <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">{t.kbMountNone}</div>
                ) : collections.map(c => (
                  <button key={c.id} onClick={() => pick(c.id)}
                    className="w-full flex items-center justify-between px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                    <span className="flex items-center gap-2.5 min-w-0">
                      <BookOpen size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                      <span className="truncate">{c.name}</span>
                    </span>
                    {c.id === mountedId
                      ? <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />
                      : <span className="text-[11px] text-gray-400 group-hover:text-white/80 shrink-0">{c.docCount}</span>}
                  </button>
                ))}
                {active && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <button onClick={unmount}
                      className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                      <X size={15} className="text-gray-400 group-hover:text-white/90" />
                      {t.kbMountRemove}
                    </button>
                  </>
                )}
              </div>
            </>
          )}
        </div>
      );
    };

    // [plan/yolo] composer 模式 chip:默认 Yolo,下拉手切 Plan。进 Plan=只读调研
    // (底座 ReadOnly+只读工具集),调 update_plan 出方案卡决策。切换逻辑搬自旧 ModeHeader。
    const ComposerModeChip = ({ t, bs, compact }) => {
      const [open, setOpen] = useState(false);
      const ms = (bs && bs.modeState) || { mode: 'yolo' };
      const isPlan = ms.mode === 'plan';
      async function switchTo(target) {
        setOpen(false);
        if (!bridge.available) return;
        if (target === 'plan' && !isPlan) {
          await bridge.setPlanModeNext();
        } else if (target === 'yolo' && isPlan) {
          if (bs && bs.busy) await bridge.cancelGeneration();
          await bridge.exitPlanToYolo();
        }
      }
      const optCls = "w-full flex items-center justify-between px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group";
      return (
        <div className="relative">
          <button onClick={() => setOpen(!open)} title={t.modeSwitchTitle + ' · ' + (isPlan ? t.modePlan : t.modeYolo)}
            className={`flex items-center shrink-0 font-semibold transition-colors border ${compact ? 'justify-center w-9 h-9 rounded-full' : 'gap-1.5 px-2.5 py-1.5 rounded-xl text-[13px]'} ${isPlan
              ? 'bg-[#E8F0FE] dark:bg-[#1A3A5C] text-[#1A73E8] dark:text-[#A8C7FA] border-[#1A73E8]/20 dark:border-[#A8C7FA]/25'
              : 'bg-gray-100 dark:bg-white/5 hover:bg-gray-200 dark:hover:bg-white/10 text-gray-700 dark:text-gray-200 border-black/[0.04] dark:border-white/5'}`}>
            {isPlan
              ? <ClipboardList size={compact ? 18 : 14} className="opacity-70 shrink-0" />
              : <Zap size={compact ? 18 : 14} className="opacity-70 shrink-0" />}
            {!compact && <span>{isPlan ? t.modePlan : t.modeYolo}</span>}
            {!compact && <ChevronDown size={14} className="opacity-50 shrink-0" />}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-60 bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                <button onClick={() => switchTo('yolo')} className={optCls}>
                  <span className="flex flex-col items-start min-w-0">
                    <span className="font-semibold">{t.modeYolo}</span>
                    <span className="text-[11px] text-gray-400 group-hover:text-white/80">{t.modeYoloDesc}</span>
                  </span>
                  {!isPlan && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
                </button>
                <button onClick={() => switchTo('plan')} className={optCls}>
                  <span className="flex flex-col items-start min-w-0">
                    <span className="font-semibold">{t.modePlan}</span>
                    <span className="text-[11px] text-gray-400 group-hover:text-white/80">{t.modePlanDesc}</span>
                  </span>
                  {isPlan && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
                </button>
              </div>
            </>
          )}
        </div>
      );
    };

    const ChatView = ({ theme, t, bs, prefill, onPrefillConsumed, onOpenEditor, justInstalledTool, setJustInstalledTool, onGotoSettings, onGotoTools, onBackScheduledRun }) => {
      const isDark = theme === 'dark';
      const [inputText, setInputText] = useState('');
      const [artifactsOpen, setArtifactsOpen] = useState(false);
      // ── 产物分栏:宽屏(≥900)并排可拖、窄屏回退覆盖抽屉 ──
      const ART_MIN = 360, ART_MAX_RATIO = 0.65, ART_DEFAULT_RATIO = 0.45, ART_NARROW = 900;
      const rootRef = useRef(null);
      const artColRef = useRef(null);
      const [isWide, setIsWide] = useState(() => (typeof window !== 'undefined' ? window.innerWidth : 1200) >= ART_NARROW);
      const [artifactW, setArtifactW] = useState(() => {
        const s = parseInt(localStorage.getItem('pinvou_artifactW') || '', 10);
        const w = (typeof window !== 'undefined' ? window.innerWidth : 1200);
        return Number.isFinite(s) && s >= ART_MIN ? s : Math.round(w * ART_DEFAULT_RATIO);
      });
      useEffect(() => {
        const onResize = () => {
          setIsWide(window.innerWidth >= ART_NARROW);
          setArtifactW(w => Math.max(ART_MIN, Math.min(w, Math.round(window.innerWidth * ART_MAX_RATIO))));
        };
        onResize();                 // 挂载即测一次(maximized 启动时 init 可能读到小值,这里校正)
        const t = setTimeout(onResize, 300);  // 再补一发,防 webview 首帧尺寸未定
        window.addEventListener('resize', onResize);
        return () => { clearTimeout(t); window.removeEventListener('resize', onResize); };
      }, []);
      const startArtifactDrag = (e) => {
        e.preventDefault();
        const rect = rootRef.current ? rootRef.current.getBoundingClientRect() : { right: window.innerWidth, width: window.innerWidth };
        const max = Math.min(rect.width * ART_MAX_RATIO, rect.width - ART_MIN);
        const col = artColRef.current;
        let last = artifactW, raf = 0;
        if (col) col.style.pointerEvents = 'none';   // 拖动时让产物 iframe 不吃 mousemove(否则往右拖发涩)
        const onMove = (ev) => {
          last = Math.max(ART_MIN, Math.min(rect.right - ev.clientX, max));
          if (raf) return;                            // rAF 合帧:每帧最多改一次
          raf = requestAnimationFrame(() => {
            raf = 0;
            if (col) col.style.width = last + 'px';    // 直接改 DOM 宽度,拖动期间不触发 React 重渲染
          });
        };
        const onUp = () => {
          document.removeEventListener('mousemove', onMove);
          document.removeEventListener('mouseup', onUp);
          if (raf) cancelAnimationFrame(raf);
          if (col) col.style.pointerEvents = '';
          document.body.style.cursor = ''; document.body.style.userSelect = '';
          setArtifactW(last);                          // 仅松手时提交一次 state + 落盘
          localStorage.setItem('pinvou_artifactW', String(Math.round(last)));
        };
        document.addEventListener('mousemove', onMove);
        document.addEventListener('mouseup', onUp);
        document.body.style.cursor = 'col-resize'; document.body.style.userSelect = 'none';
      };
      const resetArtifactW = () => {
        const w = Math.round((rootRef.current ? rootRef.current.getBoundingClientRect().width : window.innerWidth) * ART_DEFAULT_RATIO);
        setArtifactW(w); localStorage.setItem('pinvou_artifactW', String(w));
      };
      const scrollRef = useRef(null);
      const autoScrollRef = useRef(true);
      const [showScrollBottom, setShowScrollBottom] = useState(false);
      const composerRef = useRef(null);
      // 输入框自动增高:随内容从最小(~2行)长到上限 160px,再内部滚动(iOS 手感)。
      // 清空(发送后)inputText 变 '' → 自动缩回最小高。
      useEffect(() => {
        const el = composerRef.current;
        if (!el) return;
        el.style.height = 'auto';
        el.style.height = Math.min(Math.max(el.scrollHeight, 48), 160) + 'px';
      }, [inputText]);
      // 输入框是浮动绝对定位,会随 auto-grow / 附件 / 排队 chips 变高 → 量它实际高度,
      // 动态给滚动区底部留白(= 输入框高 + 间距),保证最后几条消息永不被遮挡、也不浪费空间。
      const composerWrapRef = useRef(null);
      const [composerH, setComposerH] = useState(0);
      // 底栏响应式:输入框实际可用宽 < 阈值 → 控件收成纯图标;够宽 → 图标+文字(像 WorkBuddy)
      const [composerCompact, setComposerCompact] = useState(false);
      const COMPOSER_COMPACT_W = 660;
      useEffect(() => {
        const el = composerWrapRef.current;
        if (!el) return;
        const measure = () => { setComposerH(el.offsetHeight); setComposerCompact(el.clientWidth < COMPOSER_COMPACT_W); };
        measure();
        if (!window.ResizeObserver) return;
        const ro = new ResizeObserver(measure);
        ro.observe(el);
        return () => ro.disconnect();
      }, []);
      const chatItems = bs ? bs.chatItems : [];
      const busy = bs ? bs.busy : false;
      const hasMessages = chatItems.length > 0;
      const attachments = (bs && bs.attachments) || [];
      const queued = (bs && bs.queued) || []; // 排队待发消息（当前 session 生成中时积压）
      const ctxTokens = (bs && bs.tokens) || null; // {input, max}，chat:usage 每轮更新
      const ctxPct = ctxTokens && ctxTokens.max > 0 ? ctxTokens.input / ctxTokens.max : 0;
      const fmtCtxTok = (n) => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n);
      const artifactCount = (bs && bs.artifacts) ? bs.artifacts.length : 0;
      const hasSkill = !!(bs && bs.workflow && bs.workflow.activeSkillName);
      const isScheduledTaskCreationChat = !!(bs && bs.scheduledTaskCreationSessionId && bs.activeSessionId === bs.scheduledTaskCreationSessionId);
      const scheduledRunContext = bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === bs.activeSessionId
        ? bs.scheduledRunContext
        : null;
      let lastUserId = null;
      for (let i = chatItems.length - 1; i >= 0; i--) { if (chatItems[i].type === 'user') { lastUserId = chatItems[i].id; break; } }

      // 工作流启用时预填输入框
      useEffect(() => {
        if (prefill) {
          setInputText(prefill);
          setTimeout(() => {
            if (composerRef.current) {
              composerRef.current.focus();
              composerRef.current.setSelectionRange(prefill.length, prefill.length);
            }
          }, 80);
          if (onPrefillConsumed) onPrefillConsumed();
        }
      }, [prefill]);

      // Auto-scroll：直接滚内部容器到底（绝不动外层窗口，避免内容被顶到看不见/拉不动）
      const isNearChatBottom = (el) => (el.scrollHeight - el.scrollTop - el.clientHeight) < 96;

      useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const onScroll = () => {
          const near = isNearChatBottom(el);
          const shouldShow = !near && el.scrollHeight > el.clientHeight + 4;
          autoScrollRef.current = near;
          setShowScrollBottom(v => v === shouldShow ? v : shouldShow);
        };
        onScroll();
        el.addEventListener('scroll', onScroll, { passive: true });
        return () => el.removeEventListener('scroll', onScroll);
      }, []);

      function scrollChatToBottom() {
        const el = scrollRef.current;
        if (!el) return;
        autoScrollRef.current = true;
        setShowScrollBottom(false);
        el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
      }

      useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const lastItem = chatItems[chatItems.length - 1];
        if (!lastItem) { autoScrollRef.current = true; setShowScrollBottom(false); return; }
        if (autoScrollRef.current || lastItem.type === 'user') {
          el.scrollTop = el.scrollHeight;
          autoScrollRef.current = true;
          setShowScrollBottom(false);
        } else {
          const shouldShow = !isNearChatBottom(el) && el.scrollHeight > el.clientHeight + 4;
          setShowScrollBottom(v => v === shouldShow ? v : shouldShow);
        }
      }, [chatItems.length, chatItems[chatItems.length - 1]?.html, composerH]);

      // 切换/加载会话:无条件把新会话滚到最底部(最新消息)并复位 autoScrollRef。
      // 上面的流式 auto-scroll 复用了跨会话持久的 autoScrollRef + 不 remount 的滚动容器,
      // 若切走前在旧会话翻过历史(autoScrollRef=false),切来的新会话会命中 else 分支、停在
      // 残留 scrollTop 半空处。按 activeSessionId 单独滚底,且在流式 effect 之后声明→后跑覆盖它。
      useEffect(() => {
        const el = scrollRef.current;
        autoScrollRef.current = true;
        setShowScrollBottom(false);
        if (el) el.scrollTop = el.scrollHeight;
      }, [bs && bs.activeSessionId]);

      // 安装工具后新建会话 → 本地显示欢迎卡片（不发 LLM query，不浪费 token）。
      // welcomeToolId 是一次性引导态,必须跟随会话身份:只有"装完工具"(justInstalledTool 非
      // null)才显示;其余任何新建对话/切换会话(activeSessionId 变)都清掉,否则残留的工具卡会
      // 顶掉「你好」欢迎语(该 tool 无 welcomeQueries 时 ToolWelcomeCard 渲染 null → 整块空白)。
      // 设置与清空收进同一 effect,按 justInstalledTool 优先,避免多 effect 同帧竞态。
      const [welcomeToolId, setWelcomeToolId] = useState(null);
      const welcomeSessionKeyRef = useRef(null);
      const activeSessionId = bs ? bs.activeSessionId : null;
      const draftEpoch = bs ? bs.draftEpoch : 0;
      const voiceInput = (bs && bs.voiceInput) || { status: 'idle' };
      const voiceActive = voiceInput.status === 'requesting_permission' || voiceInput.status === 'recording' || voiceInput.status === 'transcribing';
      const voiceRecording = voiceInput.status === 'recording';
      const voiceNotice = voiceInput.status !== 'idle' && voiceInput.message;
      const hasDraftText = inputText.trim().length > 0;
      const hasReadyAttachment = attachments.some(a => a.status === 'ready');
      const canSend = hasDraftText || hasReadyAttachment;
      const canClearInput = hasDraftText && !voiceActive;
      const voiceAsrSetup = (bs && bs.voiceAsrSetup) || { open: false };
      useEffect(() => {
        const sessionKey = `${activeSessionId || 'draft'}:${draftEpoch}`;
        if (justInstalledTool) {
          setWelcomeToolId(justInstalledTool);
          welcomeSessionKeyRef.current = sessionKey;
          if (setJustInstalledTool) setJustInstalledTool(null);
        } else if (welcomeSessionKeyRef.current && welcomeSessionKeyRef.current !== sessionKey) {
          setWelcomeToolId(null);
          welcomeSessionKeyRef.current = null;
        }
        // justInstalledTool 故意不放进依赖:否则上面 setJustInstalledTool(null) 清掉它会二次触发
        // 本 effect → 这次走 else 把刚显示的欢迎卡又清空(表现为"装完工具欢迎卡一闪即消失")。
        // 依赖 activeSessionId(切会话)+ draftEpoch(每次点「新建对话」自增):后者保证即便已在草稿态
        // 再点「新建对话」(activeSessionId 不变 null→null)也能重新求值,否则残留工具卡顶掉「你好」。
      }, [justInstalledTool, activeSessionId, draftEpoch]);

      // chip 显示当前会话绑定的模型:切会话/草稿时刷新 currentSessionModelId
      useEffect(() => {
        if (bridge.available) bridge.loadSessionModel(activeSessionId);
      }, [activeSessionId]);

      function handleSend() {
        // 不再因 busy 拦截:bridge.sendMessage 在生成中会把这句排队(本轮跑完自动发)。
        if (!canSend) return;
        if (bridge.available) bridge.sendMessage(inputText.trim());
        setInputText('');
      }

      function handleKeyDown(e) {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          handleSend();
        }
      }

      function handleCancel() {
        if (bridge.available) bridge.cancelGeneration();
      }

      function handleVoiceClick() {
        if (!bridge.available) return;
        if (voiceInput.status === 'recording') {
          bridge.startVoiceInput(inputText, (text) => setInputText(prev => bridge.appendVoiceText(prev, text)));
          return;
        }
        bridge.startVoiceInput(inputText, (text) => setInputText(prev => bridge.appendVoiceText(prev, text)));
      }

      function handleClearInput() {
        if (!canClearInput) return;
        setInputText('');
      }

      function handleVoiceCancel() {
        if (bridge.available) bridge.cancelVoiceInput();
      }

      function handleVoiceClose() {
        if (bridge.available) bridge.clearVoiceInput();
      }

      function handlePaste(e) {
        const items = (e.clipboardData && e.clipboardData.items) || [];
        for (const it of items) {
          if (it.type && it.type.indexOf('image/') === 0) {
            const file = it.getAsFile();
            if (!file) continue;
            e.preventDefault();
            const reader = new FileReader();
            reader.onload = () => {
              const bytes = Array.from(new Uint8Array(reader.result));
              const ext = (file.type.split('/')[1] || 'png');
              if (bridge.available) bridge.addPasteImage(`paste-${Date.now()}.${ext}`, bytes);
            };
            reader.readAsArrayBuffer(file);
          }
        }
      }

      return (
        <div ref={rootRef} className="flex-1 flex flex-row w-full h-full min-h-0 relative z-10 animate-in fade-in duration-300">
          <div className="flex-1 flex flex-col min-w-0 relative h-full">

          {/* Top Header (浮动) */}
          <div className="absolute top-0 left-0 right-0 p-4 flex justify-between items-center z-20 pointer-events-none">
            <div className="flex items-center gap-2 min-w-0">
              {scheduledRunContext && (
                <button type="button" onClick={onBackScheduledRun}
                  data-testid="scheduled-run-back"
                  aria-label="返回定时任务运行历史"
                  title="返回定时任务运行历史"
                  className={`pointer-events-auto h-10 max-w-[520px] px-3 rounded-full flex items-center gap-2 border text-[14px] font-medium transition-colors ${isDark ? 'bg-[#1E1F20] border-[#333537] text-[#E3E3E3] hover:bg-[#2B2C2F]' : 'bg-white border-[#E3E5E8] text-[#1F1F1F] hover:bg-[#F5F5F6] shadow-sm'}`}>
                  <ArrowLeft size={16} className="shrink-0" />
                  <span className="truncate">{scheduledRunContext.taskName || '定时任务运行'}</span>
                  <span className={`shrink-0 text-[12px] ${isDark ? 'text-[#9AA0A6]' : 'text-[#85888D]'}`}>运行记录</span>
                </button>
              )}
            </div>
            <div className="flex items-center gap-2">
              <button
                onClick={() => setArtifactsOpen(true)}
                className={`pointer-events-auto px-4 py-2 rounded-full text-[14px] font-medium flex items-center gap-2 ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-white text-[#1F1F1F] hover:bg-[#F0F4F9] shadow-sm'}`}>
                <Package size={16} /> {t.artifacts}
                {artifactCount > 0 && <span className={`text-[11px] px-1.5 rounded-full ${isDark ? 'bg-[#A8C7FA] text-[#062E6F]' : 'bg-[#0B57D0] text-white'}`}>{artifactCount}</span>}
              </button>
            </div>
          </div>


          {/* Main Chat Area */}
          <div ref={scrollRef} style={{ paddingBottom: (composerH ? composerH + 48 : 160) + 'px' }} className={`flex-1 min-h-0 overflow-y-auto ${(artifactsOpen && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'} custom-scrollbar flex flex-col ${hasSkill ? 'pt-3' : 'pt-20'} ${hasMessages ? 'justify-start' : 'items-center justify-center'}`}>

            {!hasMessages && !welcomeToolId && (
              /* Gemini Style Centered Empty State */
              <div className="text-center mb-12 animate-in slide-in-from-bottom-4 duration-500">
                <h1 className={`text-[44px] font-normal mb-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
                  {t.chatGreeting}
                </h1>
              </div>
            )}

            {!hasMessages && welcomeToolId && (
              <div className="max-w-[800px] w-full mx-auto mt-8">
                <ToolWelcomeCard
                  toolId={welcomeToolId}
                  theme={theme}
                  onSend={(q) => {
                    setWelcomeToolId(null);
                    if (bridge.available) bridge.sendMessage(q);
                  }}
                />
              </div>
            )}

            {hasMessages && (
              <div className="max-w-[800px] w-full mx-auto space-y-4">
                {(() => {
                  // 每个产物 path 只在它最新的那张卡上挂品悟入口:不同文件各自最新卡都给入口
                  // (一轮多产物都能审);同文件的旧卡不挂,避免"点老卡却审到磁盘最新态"的错位
                  // (后端 focus 按 path 定向、审 workspace 当前文件)。
                  const latestArtIdByPath = {};
                  chatItems.forEach((it) => { if (it.type === 'artifact_card' && it.path) latestArtIdByPath[it.path] = it.id; });
                  const latestArtIds = new Set(Object.values(latestArtIdByPath));
                  return chatItems
                    .filter((item) => !(item.type === 'memory_candidate' && !item.resolved))
                    .map((item) => (
                    <ChatBubble key={item.id} item={item} theme={theme} t={t} onPrefill={(txt) => setInputText(txt)} onSend={(txt) => { if (bridge.available) bridge.sendMessage(txt); }} editable={!busy && item.id === lastUserId} onOpenEditor={onOpenEditor} isLatestArtifact={latestArtIds.has(item.id)} allowScheduledTaskDraft={isScheduledTaskCreationChat} />
                  ));
                })()}
                {busy && <ThinkingBubble thinking={bs && bs.thinking} theme={theme} t={t} />}
              </div>
            )}

          </div>

          {/* 底部渐变蒙层:内容滚到底时在输入框上方柔和淡出(pointer-events-none 不挡滑动/点击;高度跟随输入框 auto-grow)。 */}
          <div className={`pointer-events-none absolute bottom-0 inset-x-0 z-[15] bg-gradient-to-t to-transparent from-30% via-70% ${isDark ? 'from-[#131314] via-[#131314]/95' : 'from-white via-white/95'}`}
            style={{ height: (composerH ? composerH + 96 : 220) + 'px' }} />
          {hasMessages && showScrollBottom && (
            <div className="pointer-events-none absolute inset-x-0 z-[25] flex justify-center"
              style={{ bottom: (composerH ? composerH + 54 : 172) + 'px' }}>
              <button
                type="button"
                onClick={scrollChatToBottom}
                aria-label={t.backToBottom}
                title={t.backToBottom}
                className={`pointer-events-auto w-9 h-9 rounded-full flex items-center justify-center shadow-lg backdrop-blur transition-all hover:-translate-y-0.5 active:translate-y-0 ${
                  isDark ? 'bg-[#2B2C2F]/95 text-[#E3E3E3] border border-white/10 hover:bg-[#34363A]' : 'bg-white/95 text-[#1F1F1F] border border-black/10 hover:bg-[#F8FAFF]'
                }`}>
                <ChevronDown size={15} />
              </button>
            </div>
          )}
          {hasMessages && chatItems.some((item) => item.type === 'memory_candidate' && !item.resolved) && (
            <div className={`pointer-events-none absolute inset-x-0 z-[24] ${(artifactsOpen && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'}`}
              style={{ bottom: (composerH ? composerH + 28 : 148) + 'px' }}>
              <div className="max-w-[800px] w-full mx-auto flex flex-col items-end gap-3">
                {chatItems
                  .filter((item) => item.type === 'memory_candidate' && !item.resolved)
                  .slice(-2)
                  .map((item) => (
                    <div key={item.id} className="pointer-events-auto w-full flex justify-end">
                      <ChatBubble item={item} theme={theme} t={t} onPrefill={(txt) => setInputText(txt)} onSend={(txt) => { if (bridge.available) bridge.sendMessage(txt); }} editable={false} onOpenEditor={onOpenEditor} isLatestArtifact={false} />
                    </div>
                  ))}
              </div>
            </div>
          )}
          {/* Floating Input Area */}
          <div ref={composerWrapRef} className={`absolute bottom-8 inset-x-0 z-20 ${(artifactsOpen && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'}`}>
            <div className="max-w-[800px] w-full mx-auto">
            {/* 排队待发消息 chips（生成中继续输入会积压到这里，本轮跑完自动发） */}
            {queued.length > 0 && (
              <div className="flex flex-col gap-1 mb-2 px-2">
                {queued.map((q) => (
                  <div key={q.id} className={`flex items-center gap-1.5 pl-3 pr-1.5 py-1 rounded-full text-[12px] self-start max-w-full ${isDark ? 'bg-[#2A2B2D] text-[#C4C7C5]' : 'bg-[#EAEDF1] text-[#444746]'}`}>
                    <span className="opacity-60">{t.queuedTag}</span>
                    <span className="max-w-[480px] truncate">{q.displayText}</span>
                    <button onClick={() => bridge.removeQueued(q.id)} title={t.queuedCancel} className={`w-5 h-5 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'}`}>×</button>
                  </div>
                ))}
              </div>
            )}
            {/* 模型选择器/知识库挂载已挪进下方底栏(ComposerModelSelector/ComposerKbSelector) */}
            {/* 附件 chips */}
            {attachments.length > 0 && (
              <div className="flex flex-wrap gap-2 mb-2 px-2">
                {attachments.map((a) => (
                  <div key={a.id} className={`flex items-center gap-1.5 pl-3 pr-1.5 py-1 rounded-full text-[12px] ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F] shadow-sm'}`}>
                    <span>📎</span>
                    <span className="max-w-[160px] truncate">{a.basename}</span>
                    <span className={a.status === 'error' ? 'text-[#F28B82]' : a.status === 'parsing' ? 'opacity-60' : 'text-[#93D5A6]'}>
                      {a.status === 'parsing' ? t.attachParsing : a.status === 'error' ? t.attachFailed : '✓'}
                    </span>
                    <button onClick={() => bridge.removeAttachment(a.id)} className={`w-5 h-5 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'}`}>×</button>
                  </div>
                ))}
              </div>
            )}
            {voiceNotice && (
              <div className={`flex items-center justify-between gap-2 mb-2 px-3 py-2 rounded-2xl text-[12px] ${
                voiceInput.status === 'failed'
                  ? (isDark ? 'bg-[#3A1F1F] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]')
                  : (isDark ? 'bg-[#1E2B3A] text-[#A8C7FA]' : 'bg-[#E8F0FE] text-[#174EA6]')
              }`}>
                <span className="min-w-0 truncate">
                  {voiceInput.status === 'requesting_permission' ? t.voiceRequesting
                    : voiceInput.status === 'recording' ? t.voiceRecording
                    : voiceInput.status === 'transcribing' ? t.voiceTranscribing
                    : voiceInput.status === 'completed' ? t.voiceCompleted
                    : voiceInput.message}
                </span>
                <div className="flex items-center gap-1 shrink-0">
                  {voiceInput.status === 'failed' && voiceInput.category === 'recognition_failed' && onGotoSettings && (
                    <button onClick={onGotoSettings} className={`px-2 py-1 rounded-full font-medium ${isDark ? 'bg-white/10 hover:bg-white/20' : 'bg-black/5 hover:bg-black/10'}`}>{t.voiceGotoDeps}</button>
                  )}
                  {voiceInput.status === 'failed' && (
                    <button onClick={handleVoiceClick} className={`px-2 py-1 rounded-full ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceRetry}</button>
                  )}
                  {voiceActive && (
                    <button onClick={handleVoiceCancel} className={`px-2 py-1 rounded-full ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{t.voiceCancel}</button>
                  )}
                  {!voiceActive && (
                    <button onClick={handleVoiceClose} title={t.voiceClose} className={`w-6 h-6 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>×</button>
                  )}
                </div>
              </div>
            )}
            {voiceAsrSetup.open && (() => {
              const su = voiceAsrSetup;
              const prog = su.progress || {};
              const pct = (prog.stage === 'model' && prog.total) ? Math.floor(prog.downloaded / prog.total * 100) : null;
              const missing = (su.status && su.status.missing) || [];
              const needFfmpeg = missing.indexOf('ffmpeg') >= 0;
              return (
                <div className="fixed inset-0 z-[80] flex items-center justify-center p-4 bg-black/45"
                  onClick={() => { if (!su.installing) bridge.closeVoiceAsrSetup(); }}>
                  <div className={`w-full max-w-[440px] rounded-[20px] shadow-2xl p-6 ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}
                    onClick={e => e.stopPropagation()}>
                    <h3 className="text-[16px] font-semibold mb-2">启用本地语音识别</h3>
                    <p className="text-[13px] leading-relaxed opacity-80 mb-4">
                      首次使用需要安装语音识别组件（模型约 174MB{needFfmpeg ? ' + ffmpeg' : ''}），完全本地运行、语音不上传云端。
                    </p>
                    {su.installing && (
                      <div className="mb-4">
                        <div className="text-[12px] opacity-70 mb-1">
                          {prog.stage === 'ffmpeg' ? '正在安装 ffmpeg（可能弹系统授权框）…'
                            : prog.stage === 'model' ? ('正在下载模型 ' + (pct != null ? pct + '%' : '…'))
                            : prog.stage === 'done' ? '完成' : '准备中…'}
                        </div>
                        <div className={`h-2 rounded-full overflow-hidden ${isDark ? 'bg-white/10' : 'bg-black/10'}`}>
                          <div className="h-full bg-[#0B57D0] transition-all" style={{ width: (pct != null ? pct : 30) + '%' }} />
                        </div>
                      </div>
                    )}
                    {su.error && <div className="text-[13px] text-[#EA4335] mb-3">❌ {su.error}</div>}
                    <div className="flex items-center justify-end gap-2">
                      <button onClick={() => bridge.closeVoiceAsrSetup()} disabled={su.installing}
                        className={`text-[13px] px-4 py-2 rounded-full ${isDark ? 'bg-[#333537] hover:bg-[#444746]' : 'bg-[#E1E5EA] hover:bg-[#D3D9E0]'} ${su.installing ? 'opacity-50' : ''}`}>取消</button>
                      <button onClick={() => bridge.installVoiceAsr()} disabled={su.installing}
                        className={`text-[13px] font-medium px-4 py-2 rounded-full ${isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'} ${su.installing ? 'opacity-50' : ''}`}>
                        {su.installing ? '安装中…' : '安装'}</button>
                    </div>
                  </div>
                </div>
              );
            })()}
            <div className="bg-white/80 dark:bg-[#161618]/85 backdrop-blur-2xl border border-black/[0.06] dark:border-white/10 rounded-[28px] shadow-lg focus-within:border-blue-400/50 dark:focus-within:border-blue-500/50 transition-colors px-4 pt-3 pb-2.5">
              <textarea
                ref={composerRef}
                value={inputText}
                onChange={e => setInputText(e.target.value)}
                onKeyDown={handleKeyDown}
                onPaste={handlePaste}
                placeholder={t.placeholder}
                rows={1}
                className="w-full bg-transparent resize-none outline-none text-gray-800 dark:text-gray-100 text-[16px] leading-relaxed min-h-[48px] overflow-y-auto hide-scrollbar placeholder:text-gray-400 dark:placeholder:text-gray-500"
              />
              <TextareaContextMenu inputRef={composerRef} setValue={setInputText} theme={theme} t={t} />
              <div className="flex items-center justify-between mt-1.5 gap-2">
                <div className="flex items-center gap-1.5 min-w-0 flex-1">
                  <button onClick={() => bridge.available && bridge.pickAndAttach()} title={t.attachAdd}
                    className="w-9 h-9 shrink-0 rounded-full flex items-center justify-center text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 hover:bg-black/5 dark:hover:bg-white/10 transition-colors">
                    <Paperclip size={20} />
                  </button>
                  <button onClick={handleVoiceClick} title={voiceRecording ? t.voiceStop : t.voiceStart}
                    className={`w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                      voiceRecording
                        ? 'bg-[#C5221F] text-white hover:bg-[#A50E0E]'
                        : voiceActive
                          ? (isDark ? 'bg-[#1E2B3A] text-[#A8C7FA] hover:bg-[#24364C]' : 'bg-[#E8F0FE] text-[#174EA6] hover:bg-[#D2E3FC]')
                          : 'text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 hover:bg-black/5 dark:hover:bg-white/10'
                    }`}>
                    <Mic size={20} />
                  </button>
                  <ComposerModeChip t={t} bs={bs} compact={composerCompact} />
                  <ComposerModelSelector t={t} bs={bs} onGotoSettings={onGotoSettings} compact={composerCompact} />
                  <ComposerToolMenu t={t} onGotoTools={onGotoTools} sessionId={bs && bs.activeSessionId} compact={composerCompact} />
                  <ComposerModeMenu t={t} bs={bs} compact={composerCompact} />
                  <ComposerKbSelector t={t} bs={bs} compact={composerCompact} />
                </div>
                {hasDraftText && (
                  <button onClick={handleClearInput} disabled={!canClearInput} aria-label={t.clearInput} title={t.clearInput}
                    className={`w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                      canClearInput
                        ? (isDark ? 'text-[#C4C7C5] hover:bg-white/10' : 'text-[#5F6368] hover:bg-black/5')
                        : 'text-gray-400 cursor-not-allowed opacity-60'
                    }`}>
                    <Trash2 size={18} />
                  </button>
                )}
                {busy ? (
                  <button onClick={handleCancel}
                    className="w-9 h-9 shrink-0 rounded-full flex items-center justify-center bg-black/5 dark:bg-white/10 text-[#C5221F] dark:text-[#F28B82] hover:bg-black/10 dark:hover:bg-white/20 transition-colors">
                    <StopCircle size={20} />
                  </button>
                ) : (() => {
                  const ready = canSend;
                  return (
                    <button onClick={handleSend} disabled={!ready} aria-label={t.sendMsg} title={t.sendMsg}
                      className={`w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-all ${ready ? 'bg-gradient-to-b from-[#47A1FF] to-[#007AFF] text-white shadow-md hover:-translate-y-0.5 active:translate-y-0' : 'bg-black/5 dark:bg-white/10 text-gray-400 cursor-not-allowed'}`}>
                      <Send size={17} className="translate-x-[1px]" />
                    </button>
                  );
                })()}
              </div>
            </div>
            {ctxTokens && ctxTokens.input > 0 && (
              <div className={`mt-1.5 px-5 text-[11px] font-mono ${
                ctxPct >= 0.9 ? (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]')
                : ctxPct >= 0.75 ? (isDark ? 'text-[#F9AB00]' : 'text-[#B06000]')
                : (isDark ? 'text-[#5F6368]' : 'text-[#9AA0A6]')}`}>
                {t.ctxUsage} {fmtCtxTok(ctxTokens.input)} / {fmtCtxTok(ctxTokens.max)} · {Math.round(ctxPct * 100)}%
              </div>
            )}
            <div className="flex items-center justify-center mt-3">
               <p className={`text-[12px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{t.disclaimer}</p>
            </div>
            </div>
          </div>
          </div>{/* /对话列 */}

          {artifactsOpen && isWide && (
            <>
              <div onMouseDown={startArtifactDrag} onDoubleClick={resetArtifactW} role="separator" aria-orientation="vertical"
                className={`shrink-0 w-1.5 h-full cursor-col-resize transition-colors ${isDark ? 'bg-white/10 hover:bg-[#A8C7FA]/60' : 'bg-black/10 hover:bg-[#0B57D0]/50'}`} />
              <div ref={artColRef} className="shrink-0 h-full relative" style={{ width: artifactW + 'px' }}>
                <ArtifactsPanel bs={bs} theme={theme} t={t} onClose={() => setArtifactsOpen(false)} isWide={true} />
              </div>
            </>
          )}
          {artifactsOpen && !isWide && <ArtifactsPanel bs={bs} theme={theme} t={t} onClose={() => setArtifactsOpen(false)} isWide={false} />}
        </div>
      );
    };

    // ==========================================
    // Chat Bubble (message rendering)
    // ==========================================
    function fallbackCopyText(tx) {
      return new Promise(function (resolve) {
        var ta = null;
        try {
          ta = document.createElement('textarea');
          ta.value = String(tx || '');
          ta.setAttribute('readonly', '');
          ta.style.position = 'fixed';
          ta.style.left = '-9999px';
          ta.style.top = '-9999px';
          ta.style.opacity = '0';
          document.body.appendChild(ta);
          ta.focus();
          ta.select();
          ta.setSelectionRange(0, ta.value.length);
          resolve(!!document.execCommand('copy'));
        } catch (e) {
          resolve(false);
        } finally {
          if (ta && ta.parentNode) ta.parentNode.removeChild(ta);
        }
      });
    }

    function copyClipboardText(tx) {
      tx = String(tx || '');
      if (!tx) return Promise.resolve(false);
      if (navigator.clipboard && navigator.clipboard.writeText) {
        return navigator.clipboard.writeText(tx).then(function () { return true; }).catch(function () {
          return fallbackCopyText(tx);
        });
      }
      return fallbackCopyText(tx);
    }

    function readClipboardText() {
      if (navigator.clipboard && navigator.clipboard.readText) {
        return navigator.clipboard.readText().catch(function () { return ''; });
      }
      return Promise.resolve('');
    }

    const SelectionCopyButton = ({ hostRef, targetRef, theme, t }) => {
      const isDark = theme === 'dark';
      const [selCopy, setSelCopy] = useState({ visible: false, copied: false, text: '', x: 0, y: 0 });
      const hideTimerRef = useRef(null);

      const hideSelectionCopy = useCallback(() => {
        if (hideTimerRef.current) {
          clearTimeout(hideTimerRef.current);
          hideTimerRef.current = null;
        }
        setSelCopy(s => s.visible ? { ...s, visible: false, copied: false } : s);
      }, []);

      const openSelectionCopyMenu = useCallback((event) => {
        const target = targetRef.current;
        const host = hostRef.current;
        if (!target || !host || !window.getSelection) return false;
        const selection = window.getSelection();
        if (!selection || selection.rangeCount === 0) { hideSelectionCopy(); return false; }
        const text = selection.toString();
        if (!text || !text.trim()) { hideSelectionCopy(); return false; }
        if (!selection.anchorNode || !selection.focusNode) { hideSelectionCopy(); return false; }
        if (!target.contains(selection.anchorNode) || !target.contains(selection.focusNode)) {
          hideSelectionCopy();
          return false;
        }
        const hostRect = host.getBoundingClientRect();
        if (!hostRect) { hideSelectionCopy(); return false; }
        const minX = 4;
        const maxX = Math.max(minX, hostRect.width - 100);
        const x = Math.max(minX, Math.min(event.clientX - hostRect.left, maxX));
        const y = Math.max(4, event.clientY - hostRect.top + 8);
        setSelCopy({ visible: true, copied: false, text: text, x: x, y: y });
        return true;
      }, [hideSelectionCopy, hostRef, targetRef]);

      useEffect(() => {
        return () => {
          if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
        };
      }, []);

      useEffect(() => {
        const target = targetRef.current;
        if (!target) return;
        const onContextMenu = (e) => {
          if (openSelectionCopyMenu(e)) {
            e.preventDefault();
            e.stopPropagation();
          } else {
            hideSelectionCopy();
          }
        };
        target.addEventListener('contextmenu', onContextMenu);
        return () => {
          target.removeEventListener('contextmenu', onContextMenu);
        };
      }, [hideSelectionCopy, openSelectionCopyMenu, targetRef]);

      useEffect(() => {
        if (!selCopy.visible) return;
        const onDown = (e) => {
          if (e.target && e.target.closest && e.target.closest('[data-selection-copy-button]')) return;
          hideSelectionCopy();
        };
        const onKey = (e) => { if (e.key === 'Escape') hideSelectionCopy(); };
        document.addEventListener('mousedown', onDown, true);
        document.addEventListener('keydown', onKey, true);
        return () => {
          document.removeEventListener('mousedown', onDown, true);
          document.removeEventListener('keydown', onKey, true);
        };
      }, [hideSelectionCopy, selCopy.visible]);

      const onCopy = () => {
        copyClipboardText(selCopy.text).then(function (ok) {
          if (!ok) return;
          setSelCopy(s => ({ ...s, copied: true }));
          if (hideTimerRef.current) clearTimeout(hideTimerRef.current);
          hideTimerRef.current = setTimeout(function () {
            hideTimerRef.current = null;
            hideSelectionCopy();
          }, 900);
        });
      };

      if (!selCopy.visible) return null;
      return (
        <button
          type="button"
          data-selection-copy-button="true"
          title={selCopy.copied ? t.copied : t.copyMsg}
          onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
          onClick={(e) => { e.preventDefault(); e.stopPropagation(); onCopy(); }}
          className={`absolute z-30 h-9 min-w-[92px] px-3 rounded-[10px] flex items-center justify-start gap-2 text-[13px] font-medium shadow-lg backdrop-blur transition-colors ${
            isDark ? 'bg-[#2B2C2F] text-[#E3E3E3] hover:bg-[#34363A] border border-white/10' : 'bg-white text-[#1F1F1F] hover:bg-[#F8FAFF] border border-black/10'
          }`}
          style={{ left: selCopy.x + 'px', top: selCopy.y + 'px' }}
        >
          {selCopy.copied ? <Check size={13} className="text-[#34C759]" /> : <Copy size={13} />}
          <span>{selCopy.copied ? t.copied : t.copyMsg}</span>
        </button>
      );
    };

    const TextareaContextMenu = ({ inputRef, setValue, theme, t }) => {
      const isDark = theme === 'dark';
      const [menu, setMenu] = useState({ visible: false, x: 0, y: 0, canCopy: false });

      const closeMenu = useCallback(() => {
        setMenu(m => m.visible ? { ...m, visible: false } : m);
      }, []);

      const selectedText = useCallback(() => {
        const el = inputRef.current;
        if (!el) return '';
        const start = typeof el.selectionStart === 'number' ? el.selectionStart : 0;
        const end = typeof el.selectionEnd === 'number' ? el.selectionEnd : start;
        return start === end ? '' : String(el.value || '').slice(start, end);
      }, [inputRef]);

      const replaceSelection = useCallback((text) => {
        const el = inputRef.current;
        if (!el || !text) return;
        const raw = String(el.value || '');
        const start = typeof el.selectionStart === 'number' ? el.selectionStart : raw.length;
        const end = typeof el.selectionEnd === 'number' ? el.selectionEnd : start;
        const next = raw.slice(0, start) + text + raw.slice(end);
        const cursor = start + text.length;
        setValue(next);
        requestAnimationFrame(function () {
          el.focus();
          try { el.setSelectionRange(cursor, cursor); } catch (e) {}
        });
      }, [inputRef, setValue]);

      useEffect(() => {
        const el = inputRef.current;
        if (!el) return;
        const openMenu = (e) => {
          e.preventDefault();
          e.stopPropagation();
          const start = typeof el.selectionStart === 'number' ? el.selectionStart : 0;
          const end = typeof el.selectionEnd === 'number' ? el.selectionEnd : start;
          const menuW = 136;
          const menuH = 116;
          const x = Math.max(6, Math.min(e.clientX, window.innerWidth - menuW - 6));
          const y = Math.max(6, Math.min(e.clientY, window.innerHeight - menuH - 6));
          setMenu({ visible: true, x: x, y: y, canCopy: start !== end });
        };
        const onContextMenu = (e) => openMenu(e);
        const onMouseDown = (e) => { if (e.button === 2) openMenu(e); };
        el.addEventListener('contextmenu', onContextMenu, true);
        el.addEventListener('mousedown', onMouseDown, true);
        return () => {
          el.removeEventListener('contextmenu', onContextMenu, true);
          el.removeEventListener('mousedown', onMouseDown, true);
        };
      }, [inputRef]);

      useEffect(() => {
        if (!menu.visible) return;
        const onDown = (e) => {
          if (e.target && e.target.closest && e.target.closest('[data-textarea-context-menu]')) return;
          closeMenu();
        };
        const onKey = (e) => { if (e.key === 'Escape') closeMenu(); };
        const onScrollOrResize = () => closeMenu();
        document.addEventListener('mousedown', onDown, true);
        document.addEventListener('keydown', onKey, true);
        window.addEventListener('resize', onScrollOrResize);
        window.addEventListener('scroll', onScrollOrResize, true);
        return () => {
          document.removeEventListener('mousedown', onDown, true);
          document.removeEventListener('keydown', onKey, true);
          window.removeEventListener('resize', onScrollOrResize);
          window.removeEventListener('scroll', onScrollOrResize, true);
        };
      }, [closeMenu, menu.visible]);

      const menuItemCls = (disabled) => `w-full h-9 px-3 flex items-center gap-2 text-left text-[13px] transition-colors ${
        disabled
          ? (isDark ? 'text-white/30 cursor-not-allowed' : 'text-black/30 cursor-not-allowed')
          : (isDark ? 'text-[#E3E3E3] hover:bg-white/10' : 'text-[#1F1F1F] hover:bg-black/[0.06]')
      }`;

      const selectAll = () => {
        const el = inputRef.current;
        if (!el) return;
        el.focus();
        el.select();
        closeMenu();
      };

      const copySelected = () => {
        const tx = selectedText();
        if (!tx) return;
        copyClipboardText(tx).then(function () { closeMenu(); });
      };

      const pasteText = () => {
        readClipboardText().then(function (tx) {
          replaceSelection(tx);
          closeMenu();
        });
      };

      if (!menu.visible) return null;
      return createPortal((
        <div
          data-textarea-context-menu="true"
          className={`w-[136px] overflow-hidden rounded-[12px] py-1 shadow-xl backdrop-blur border ${
            isDark ? 'bg-[#2B2C2F] border-white/10' : 'bg-white border-black/10'
          }`}
          style={{ position: 'fixed', zIndex: 9999, left: menu.x + 'px', top: menu.y + 'px' }}
          onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
        >
          <button type="button" className={menuItemCls(false)} onClick={selectAll}>
            <span className="w-4 text-center text-[12px]">A</span><span>{t.selectAllMsg || 'Select all'}</span>
          </button>
          <button type="button" disabled={!menu.canCopy} className={menuItemCls(!menu.canCopy)} onClick={copySelected}>
            <Copy size={14} /><span>{t.copyMsg}</span>
          </button>
          <button type="button" className={menuItemCls(false)} onClick={pasteText}>
            <ClipboardList size={14} /><span>{t.pasteMsg || 'Paste'}</span>
          </button>
        </div>
      ), document.body);
    };

    const UserBubble = ({ item, theme, editable, t }) => {
      const isDark = theme === 'dark';
      const [editing, setEditing] = useState(false);
      const [val, setVal] = useState(item.text);
      const [copied, setCopied] = useState(false);
      function commit() { const tx = val.trim(); setEditing(false); if (tx && bridge.available) bridge.editLastTurn(tx); }
      function copyText() {
        const tx = item.text || '';
        copyClipboardText(tx).then(function (ok) {
          if (!ok) return;
          setCopied(true);
          setTimeout(function () { setCopied(false); }, 1200);
        });
      }
      if (editing) {
        return (
          <div className="flex justify-end">
            <div className="max-w-[85%] w-full">
              <textarea autoFocus value={val} onChange={e => setVal(e.target.value)}
                rows={Math.min(6, Math.max(1, val.split('\n').length))}
                onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); commit(); } else if (e.key === 'Escape') { setEditing(false); setVal(item.text); } }}
                className={`w-full rounded-[16px] px-4 py-2 text-[15px] outline-none ${isDark ? 'bg-[#004A77] text-[#E3E3E3]' : 'bg-[#D3E3FD] text-[#1F1F1F]'}`} />
              <div className="flex gap-2 justify-end mt-1">
                <button className={cardBtnCls(isDark)} onClick={() => { setEditing(false); setVal(item.text); }}>{t.cpCancel}</button>
                <button className={cardBtnCls(isDark, 'primary')} onClick={commit}>{t.resend}</button>
              </div>
            </div>
          </div>
        );
      }
      if (item.pinvouTransfer) {
        const isWu = item.pinvouTransfer === '悟';
        const tint = isWu ? (isDark ? '#8AB4F8' : '#1967D2') : (isDark ? '#D0BCFF' : '#7C3AED');
        const tintBg = isWu ? (isDark ? 'bg-[#1A73E8]/10' : 'bg-[#1A73E8]/[0.06]') : (isDark ? 'bg-[#D0BCFF]/10' : 'bg-[#7C3AED]/[0.07]');
        return (
          <div className="flex justify-end">
            <div className="max-w-[85%]">
              <div className="flex items-center justify-end gap-1 mb-1 text-[11px] font-medium" style={{ color: tint }}>
                <span>{isWu ? '✨' : '📋'}</span><span>{'Pinvou · ' + item.pinvouTransfer + ' · 转交修订'}</span>
              </div>
              <div className={`px-5 py-3 rounded-[20px] text-[15px] leading-relaxed whitespace-pre-wrap ${tintBg} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{item.text}</div>
            </div>
          </div>
        );
      }
      const actBtn = isDark
        ? 'text-[#8E8E8E] hover:text-[#E3E3E3] hover:bg-white/10'
        : 'text-[#9AA0A6] hover:text-[#444746] hover:bg-black/[0.06]';
      return (
        <div className="flex justify-end group">
          <div className="flex flex-col items-end max-w-[85%]">
            <div className={`px-5 py-3 rounded-[20px] text-[15px] leading-relaxed whitespace-pre-wrap ${isDark ? 'bg-[#004A77] text-[#E3E3E3]' : 'bg-[#D3E3FD] text-[#1F1F1F]'}`}>{item.text}</div>
            {/* iOS 风操作条：hover 气泡时下方浮现。复制=所有 query；编辑重发=仅最新(editable)。 */}
            <div className="flex items-center gap-0.5 mt-1 pr-1 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
              <button title={copied ? t.copied : t.copyMsg} onClick={copyText}
                className={`w-7 h-7 rounded-lg flex items-center justify-center transition-colors ${actBtn}`}>
                {copied ? <Check size={14} className="text-[#34C759]" /> : <Copy size={14} />}
              </button>
              {editable && (
                <button title={t.editResend} onClick={() => { setVal(item.text); setEditing(true); }}
                  className={`w-7 h-7 rounded-lg flex items-center justify-center transition-colors ${actBtn}`}>
                  <Edit2 size={14} />
                </button>
              )}
            </div>
          </div>
        </div>
      );
    };

    // 思考指示器：Braille 转圈 + 思考中/调用工具 + 计时（每阶段切换重置）
    const BRAILLE = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    const ThinkingBubble = ({ thinking, theme, t }) => {
      const isDark = theme === 'dark';
      const [frame, setFrame] = useState(0);
      const [elapsed, setElapsed] = useState(0);
      const phase = thinking ? thinking.phase : 'thinking';
      const toolName = thinking ? thinking.toolName : '';
      const startedAt = (thinking && thinking.startedAt) || Date.now();
      useEffect(() => {
        setFrame(0); setElapsed(0);
        const id = setInterval(() => {
          setFrame(f => (f + 1) % BRAILLE.length);
          setElapsed(Math.floor((Date.now() - startedAt) / 1000));
        }, 100);
        return () => clearInterval(id);
      }, [startedAt, phase, toolName]);
      let text;
      if (phase === 'tool' && toolName) {
        text = t.thinkingCall(toolName, elapsed);
      } else {
        const suffix = elapsed >= 120 ? ` · ${t.hintSlow120}` : elapsed >= 30 ? ` · ${t.hintSlow30}` : '';
        text = `${t.thinkingLabel}... ${elapsed}s${suffix}`;
      }
      return (
        <div className="flex justify-start">
          <div className={`text-[13px] font-mono px-3 py-1.5 rounded-full ${isDark ? 'bg-[#1E1F20] text-[#A8C7FA]' : 'bg-[#F0F4F9] text-[#0B57D0]'}`}>
            {BRAILLE[frame]} {text}
          </div>
        </div>
      );
    };

    // ③ 卡牌制造专家: 从助手消息渲染后的 html 里抠出 ```persona-card 草稿块 → 解析成卡。
    function htmlUnescape(s) {
      return String(s).replace(/&lt;/g,'<').replace(/&gt;/g,'>').replace(/&quot;/g,'"').replace(/&#39;/g,"'").replace(/&amp;/g,'&');
    }
    function asDraft(d) {
      if (!d || typeof d !== 'object' || !d.name || !d.body) return null;
      var dept = (d.dept && DEPT_ORDER.indexOf(d.dept) >= 0) ? d.dept : 'specialized';
      return { name: d.name, dept: dept, emoji: d.emoji || '🃏', color: d.color || '', description: d.description || '', body: d.body };
    }
    function asScheduledTaskDraft(d) {
      if (!d || typeof d !== 'object' || !d.name || !d.prompt || !d.rrule) return null;
      return {
        name: String(d.name),
        prompt: String(d.prompt),
        rrule: String(d.rrule),
        cwds: Array.isArray(d.cwds) ? d.cwds.filter((item) => typeof item === 'string') : [],
        mode: d.mode ? String(d.mode) : 'yolo',
        allowShell: !!d.allowShell,
        trustMode: !!d.trustMode,
        autoApprove: !!d.autoApprove,
        paused: !!d.paused,
      };
    }
    // 模型偶尔给 JSON 末尾多带一个逗号(trailing comma)等小瑕疵 → 严格 JSON.parse 会整段拒掉,
    // 导致草稿卡解析失败、不出「存入卡牌池」按钮。宽松解析:先严格,失败再去掉对象/数组结尾多余
    // 逗号重试(去尾逗号对合法 JSON 无副作用)。失败返回 null(不抛)。
    // 从第一个 { 起按括号配平截取一个完整 JSON 对象(尊重字符串/转义),容忍对象后面多余的 }
    // 或杂物(模型偶发多打一个右括号、或 ``` 块后还粘了别的字符)。截断(没闭合)则返回 null。
    function extractBalancedJson(s) {
      var i = s.indexOf('{');
      if (i < 0) return null;
      var depth = 0, inStr = false, esc = false;
      for (var j = i; j < s.length; j++) {
        var c = s.charAt(j);
        if (inStr) {
          if (esc) esc = false;
          else if (c === '\\') esc = true;
          else if (c === '"') inStr = false;
        } else if (c === '"') inStr = true;
        else if (c === '{') depth++;
        else if (c === '}') { depth--; if (depth === 0) return s.slice(i, j + 1); }
      }
      return null;
    }
    // 一条完整解析链: 严格 → 去尾逗号 → 从首个 { 括号配平截取(+去尾逗号)。失败返回 null。
    // (名字避开本文件里另一个 tryParseJson —— 那是工具结果视图用的简单版)
    function parseJsonChain(s) {
      try { return JSON.parse(s); } catch (e) {}
      try { return JSON.parse(s.replace(/,(\s*[}\]])/g, '$1')); } catch (e) {} // 去尾逗号
      var bal = extractBalancedJson(s);                                        // 容忍多余右括号/尾部杂物
      if (bal) {
        try { return JSON.parse(bal); } catch (e) {}
        try { return JSON.parse(bal.replace(/,(\s*[}\]])/g, '$1')); } catch (e) {}
      }
      return null;
    }
    function parseLooseJson(raw) {
      var v = parseJsonChain(raw);
      if (v) return v;
      // 最后手段: 模型偶尔把本该是普通 " 的地方(数组/对象边界处)打成转义 \",令整段 JSON 非法,
      // 上面的链全兜不住(实例: card-question 的 options 后两项被写成 \"...\")。去掉多余转义引号
      // 再走一遍完整链。仅对**已解析失败**的输入生效 —— 合法用了 \" 的 JSON 在 parseJsonChain(raw)
      // 第一步就成功、走不到这里,故不受影响。
      var deesc = raw.replace(/\\"/g, '"');
      return deesc !== raw ? parseJsonChain(deesc) : null;
    }
    // 扫所有 ```代码块,任何能解析成「含 name+body 的 JSON」的就当卡牌草稿。
    // 不强求 ```persona-card 标签 —— 小模型常打 ```json 或不打标签,放宽识别更鲁棒。
    // 形状校验(name+body)避免把别的 JSON 误判成草稿。明确 persona-card 标签的优先。
    // 返回 { draft, html }:html 是把那段原始 JSON 块抹掉后的版本(用户只看友好草稿卡,不看机器载荷)。
    function parsePersonaDraft(html) {
      if (!html || html.indexOf('{') < 0) return { draft: null, html: html };
      var re = /<pre[^>]*>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m, chosen = null, chosenDraft = null;
      while ((m = re.exec(html))) {
        var raw = htmlUnescape(m[2]).trim();
        if (raw.charAt(0) !== '{') continue;
        try {
          var draft = asDraft(parseLooseJson(raw));
          if (!draft) continue;
          if (/persona-card/.test(m[1])) { chosen = m[0]; chosenDraft = draft; break; } // 明确标签优先
          if (!chosenDraft) { chosen = m[0]; chosenDraft = draft; }
        } catch (e) { /* 非 JSON 块,跳过 */ }
      }
      if (!chosenDraft) return { draft: null, html: html };
      return { draft: chosenDraft, html: html.replace(chosen, '') };
    }
    function parseScheduledTaskDraft(html) {
      if (!html || html.indexOf('{') < 0) return { draft: null, html: html };
      var re = /<pre[^>]*>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m, chosen = null, chosenDraft = null;
      while ((m = re.exec(html))) {
        var raw = htmlUnescape(m[2]).trim();
        if (raw.charAt(0) !== '{') continue;
        var draft = asScheduledTaskDraft(parseLooseJson(raw));
        if (!draft) continue;
        if (/scheduled-task-draft/.test(m[1])) { chosen = m[0]; chosenDraft = draft; break; }
        if (!chosenDraft) { chosen = m[0]; chosenDraft = draft; }
      }
      if (!chosenDraft) return { draft: null, html: html };
      return { draft: chosenDraft, html: html.replace(chosen, '') };
    }
    // 卡牌制造专家追问时,若问题有可选项,会输出一个 ```card-question 块 {question, options[]}。
    // 抠出来 → 渲染成可点击的 iOS 选项卡;点选项即把它作为回答发送。返回 { q, html(抹掉块) }。
    function parseCardQuestion(html) {
      if (!html || html.indexOf('card-question') < 0) return { q: null, html: html };
      var re = /<pre[^>]*>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m;
      while ((m = re.exec(html))) {
        if (!/card-question/.test(m[1])) continue;
        var raw = htmlUnescape(m[2]).trim();
        if (raw.charAt(0) !== '{') continue;
        var d = parseLooseJson(raw);
        if (d && d.question && Array.isArray(d.options)) {
          var opts = d.options.filter(function (o) { return typeof o === 'string' && o.trim(); });
          if (opts.length) return { q: { question: String(d.question), options: opts }, html: html.replace(m[0], '') };
        }
      }
      return { q: null, html: html };
    }
    // 点选项时实际发送的回答:取"短标签 —— 说明"里的短标签;没分隔符就发整句。
    function optionAnswer(opt) {
      var s = String(opt).split(/\s*(?:——|—|::|:|：|\(|（)/)[0].trim();
      return s || String(opt).trim();
    }
    // 流式中: JSON 还没闭合无法解析,把正在生成的卡牌/选项代码块折叠成占位,避免原始 JSON 一直刷屏。
    function hideStreamingDraft(html, label) {
      if (!html) return html;
      var m = /<pre[^>]*>\s*<code[^>]*(?:persona-card|card-question|scheduled-task-draft)[\s\S]*$/i.exec(html); // persona-card / card-question / scheduled-task-draft 标签块(到末尾)
      if (!m) m = /<pre[^>]*>\s*<code[^>]*>\s*\{[\s\S]*?(?:name|&quot;name|rrule|&quot;rrule)[\s\S]*$/i.exec(html); // 兜底: 以 { 开头且含 name / rrule 的块
      if (!m) return html;
      return html.slice(0, m.index) + '<div style="margin-top:.5em;opacity:.7;font-size:13px">' + (label || '🃏 正在设计卡牌…') + '</div>';
    }

    const ChatBubble = ({ item, theme, onPrefill, onSend, editable, onOpenEditor, t, isLatestArtifact, allowScheduledTaskDraft }) => {
      const isDark = theme === 'dark';
      const assistantSelectionHostRef = useRef(null);
      const assistantSelectionTargetRef = useRef(null);

      if (item.type === 'artifact_card') return <ArtifactCard item={item} theme={theme} t={t} isLatest={isLatestArtifact} />;
      if (item.type === 'plan_card') return <PlanCard item={item} theme={theme} t={t} onPrefill={onPrefill} />;
      if (item.type === 'plan_stuck') return <PlanStuckCard item={item} theme={theme} t={t} />;
      if (item.type === 'careful_blocked') return <CarefulBlockedCard item={item} theme={theme} t={t} />;
      if (item.type === 'user_input') return <UserInputCard item={item} theme={theme} t={t} />;
      if (item.type === 'user') return <UserBubble item={item} theme={theme} editable={editable} t={t} />;

      if (item.type === 'card_creator_intro') {
        return (
          <div className="flex justify-start" style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
            <div className="rounded-[14px] px-4 py-3 max-w-[440px] text-[15px] font-medium" style={{ background: isDark ? '#1C1C1E' : '#F2F2F7', color: isDark ? '#fff' : '#000' }}>{t.cpIntroTitle}</div>
          </div>
        );
      }

      if (item.type === 'assistant') {
        if (item.streaming && !item.html) return null; // 空流式气泡交给 ThinkingBubble 表示
        const html = item.html || '';
        const streamingDraftLabel = /scheduled-task-draft/.test(html) ? '⏰ 正在整理定时任务草稿…' : (t && t.cpDesigning);
        const pd = item.streaming ? { draft: null, html: hideStreamingDraft(html, streamingDraftLabel) } : parsePersonaDraft(html);
        const sd = (item.streaming || !allowScheduledTaskDraft) ? { draft: null, html: pd.html } : parseScheduledTaskDraft(pd.html);
        const cq = item.streaming ? { q: null, html: sd.html } : parseCardQuestion(sd.html);
        // 草稿是否已存入(按名字在已加载的卡池里找同名自制卡 → 派生"已存入",免单独持久化)
        const draftSaved = pd.draft && bridge.available && bridge.getPersonas
          && bridge.getPersonas().some(function(c){ return c && c.source === 'user' && c.name === pd.draft.name; });
        return (
          <div className="flex justify-start">
            <div ref={assistantSelectionHostRef} className={`relative ${cq.q ? 'w-full' : 'max-w-[95%]'} ${isDark ? 'dark-code' : 'light-code'}`}>
              <div
                ref={assistantSelectionTargetRef}
                className={`msg-md text-[15px] leading-relaxed ${item.streaming ? 'streaming-cursor' : ''} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}
                onClick={(e) => {
                  // 聊天里的链接(如飞书授权 URL)点击 → 走系统浏览器,别导航主窗口/不可点。
                  const a = e.target && e.target.closest && e.target.closest('a[href]');
                  if (!a) return;
                  const href = a.getAttribute('href') || '';
                  if (/^https?:\/\//i.test(href)) {
                    e.preventDefault();
                    window.__TAURI__.core.invoke('open_external_url', { url: href }).catch(() => {});
                  }
                }}
                dangerouslySetInnerHTML={{ __html: cq.html || '' }}
              />
              <SelectionCopyButton hostRef={assistantSelectionHostRef} targetRef={assistantSelectionTargetRef} theme={theme} t={t} />
              {cq.q ? (
                <div className="mt-2 w-full" style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  <div className="text-[14px] font-medium mb-2" style={{ color: isDark ? '#fff' : '#000' }}>{cq.q.question}</div>
                  <div className="rounded-[14px] overflow-hidden" style={{ background: isDark ? '#1C1C1E' : '#fff', border: isDark ? 'none' : '0.5px solid rgba(60,60,67,.12)' }}>
                    {cq.q.options.map((opt, i) => (
                      <button key={i} onClick={()=> onSend && onSend(optionAnswer(opt))}
                        className="w-full flex items-center gap-3 px-4 py-3 text-left transition-opacity active:opacity-60 hover:opacity-90"
                        style={i ? { borderTop: '0.5px solid ' + (isDark ? 'rgba(84,84,88,.45)' : 'rgba(60,60,67,.12)') } : undefined}>
                        <span className="text-[15px] shrink-0 text-right" style={{ color: '#8E8E93', width: 15, fontVariantNumeric: 'tabular-nums' }}>{i + 1}</span>
                        <span className="text-[15px] flex-1 min-w-0" style={{ color: isDark ? '#fff' : '#000' }}>{opt}</span>
                        <ChevronRight size={16} className="shrink-0" style={{ color: '#C7C7CC' }} />
                      </button>
                    ))}
                  </div>
                </div>
              ) : null}
              {pd.draft ? (
                <div className="mt-2 rounded-[14px] p-3 flex items-center gap-3 max-w-[460px]" style={{ background: isDark ? '#1C1C1E' : '#F2F2F7' }}>
                  <AppIcon card={pd.draft} isDark={isDark} cls="w-11 h-11 rounded-[12px]" fb={22} />
                  <div className="min-w-0 flex-1">
                    <div className="text-[15px] font-semibold leading-snug truncate" style={{ color: isDark ? '#fff' : '#000' }}>{pd.draft.name}</div>
                    <div className="text-[13px] truncate" style={{ color: isDark ? 'rgba(235,235,245,.6)' : 'rgba(60,60,67,.6)' }}>{pd.draft.description || deptLabelFor(t, pd.draft.dept)}</div>
                  </div>
                  {draftSaved
                    ? <span className="shrink-0 inline-flex items-center gap-1 h-8 px-1 text-[13px] font-medium" style={{ color:'#8E8E93' }} title={t.cpDraftSavedTitle}><Check size={15} strokeWidth={2.5} style={{ color:'#34C759' }} />{t.cpDraftSaved}</span>
                    : <button onClick={()=> onOpenEditor && onOpenEditor(pd.draft)} className="shrink-0 px-4 h-8 rounded-full text-[13px] font-semibold text-white" style={{ background: isDark ? '#0A84FF' : '#007AFF' }} title={t.cpDraftViewTitle}>{t.cpDraftView}</button>}
                </div>
              ) : null}
              {item.time && !item.streaming && (
                <div className={`text-[11px] mt-1 ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{item.time}</div>
              )}
            </div>
          </div>
        );
      }

      if (item.type === 'tool') {
        return <ToolCard item={item} theme={theme} t={t} />;
      }

      if (item.type === 'persona_equip') {
        const c = item.card || {};
        const tc = (typeof deptColor !== 'undefined' ? deptColor(c.dept) : '#ffae1e');
        const deptLabel = deptLabelFor(t, c.dept);
        const cd = personaText(c, t);
        return (
          <div className="flex flex-col gap-1.5" style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
            <div className="text-[12px] font-medium" style={{ color: '#8E8E93' }}>{t.cpEquipBubbleSys}</div>
            <div className="rounded-[14px] p-4 max-w-[560px]" style={{ background: isDark ? '#1C1C1E' : '#F2F2F7' }}>
              <div className="flex items-center gap-3 mb-3">
                <AppIcon card={c} isDark={isDark} cls="w-11 h-11 rounded-[12px]" fb={22} />
                <div className="text-[15px] font-semibold leading-snug" style={{ color: isDark ? '#fff' : '#000' }}>{t.cpEquipBubbleTitle(cd.name)}</div>
              </div>
              <div className="text-[13px] space-y-1" style={{ color: isDark ? '#C7C7CC' : '#3C3C43' }}>
                <div>{t.cpDept}: <span style={{ color: isDark ? '#0A84FF' : '#007AFF', fontWeight: 600 }}>{deptLabel}</span></div>
                <div>{t.cpDescLabel}: {cd.description}</div>
              </div>
              <div className="text-[12px] mt-2.5" style={{ color: '#8E8E93' }}>{t.cpEquipBubbleNote}</div>
            </div>
          </div>
        );
      }
      if (item.type === 'system') {
        return (
          <div className="flex justify-center">
            <div className={`text-[13px] px-4 py-1.5 rounded-full ${isDark ? 'bg-[#1E1F20] text-[#8E8E8E]' : 'bg-[#F0F4F9] text-[#757575]'}`}>
              {item.text}
            </div>
          </div>
        );
      }
      if (item.type === 'memory_notice') {
        const text = item.text || '';
        const quietNotice = item.kind === 'recent_activity' || item.kind === 'recent_work';
        const meta = item.kind === 'current_focus'
          ? { label: '当前关注', hint: '后续对话会参考这个近期事项。' }
          : item.kind === 'recent_activity' || item.kind === 'recent_work'
            ? { label: '近期动态', hint: '后续对话会参考这次完成的事情。' }
          : item.kind === 'work_context'
            ? { label: '工作背景', hint: '后续对话会参考这条长期背景。' }
          : item.kind === 'profile'
            ? { label: '称呼', hint: '后续对话会按这个称呼交流。' }
            : { label: '长期偏好', hint: '后续对话会参考这条偏好。' };
        if (quietNotice) {
          return (
            <div className="flex justify-center">
              <div
                className="inline-flex items-center gap-1.5 max-w-[360px] px-3 py-1.5 rounded-full text-[12px] text-[#AEB4BC]"
                title={text}
                style={{
                  background: 'rgba(32, 34, 38, 0.54)',
                  border: '1px solid rgba(255,255,255,0.06)',
                }}
              >
                <Check size={12} className="shrink-0 text-[#30D158]" />
                <span className="font-medium text-[#D5D9DE]">已记录近期动态</span>
                <span className="truncate">可在记忆中心查看</span>
              </div>
            </div>
          );
        }
        return (
          <div className="flex justify-end">
            <div
              className="max-w-[420px] w-full rounded-[16px] px-4 py-3 text-[#F2F3F5]"
              style={{
                background: 'rgba(32, 34, 38, 0.86)',
                border: '1px solid rgba(255,255,255,0.08)',
                boxShadow: '0 14px 36px rgba(0,0,0,0.34)',
                backdropFilter: 'blur(16px)',
                WebkitBackdropFilter: 'blur(16px)',
              }}
            >
              <div className="flex items-center gap-2 min-w-0">
                <span className="w-7 h-7 rounded-full flex items-center justify-center shrink-0 bg-[#34C759]/[0.15] text-[#30D158]">
                  <Check size={15} />
                </span>
                <div className="min-w-0">
                  <div className="flex items-center gap-2 min-w-0">
                    <span className="text-[13px] font-semibold leading-tight">{item.statusLabel || '记忆已更新'}</span>
                    <span className="text-[11px] px-2 py-0.5 rounded-full bg-white/[0.07] text-[#AEB4BC]">{meta.label}</span>
                  </div>
                  <div className="mt-1 text-[12px] leading-relaxed text-[#AEB4BC]">{meta.hint}</div>
                </div>
              </div>
              {text && (
                <div className="mt-3 ml-9 border-l-2 border-[#0A84FF]/70 pl-3 py-1 text-[13px] leading-relaxed break-words text-[#E8EAED]">
                  “{text}”
                </div>
              )}
            </div>
          </div>
        );
      }
      if (item.type === 'memory_candidate') {
        const resolved = !!item.resolved;
        if (resolved && (item.statusLabel === '已忽略' || item.statusLabel === '不再提示')) return null;
        const text = item.text || '';
        const preferenceDetail = /回答|回复|简洁|详细|风格|语气|口吻/.test(text) ? '回答风格'
          : /代码|测试|开发|实现|文档/.test(text) ? '工作方式'
          : '';
        const meta = item.kind === 'current_focus'
          ? { label: '当前关注', prompt: '我可以记住这个当前关注', hint: '以后我会用它理解你最近正在推进的工作。' }
          : item.kind === 'recent_activity' || item.kind === 'recent_work'
            ? { label: '近期动态', prompt: '我可以记住这个近期动态', hint: '以后我会用它理解你刚完成的工作。' }
          : item.kind === 'work_context'
            ? { label: '工作背景', prompt: '我可以记住这条工作背景', hint: '以后我会用它理解你的长期工作上下文。' }
          : item.kind === 'profile'
            ? { label: '称呼', prompt: '我可以记住这个称呼', hint: '以后我会按这个称呼和你交流。' }
            : { label: '偏好' + (preferenceDetail ? ' · ' + preferenceDetail : ''), prompt: '我可以记住这条偏好', hint: '以后我会按这个偏好调整回复方式。' };
        return (
          <div className="flex justify-end">
            <div
              className={`max-w-[480px] w-full rounded-[18px] px-4 py-3.5 ${isDark ? 'text-[#F2F3F5]' : 'text-[#F8FAFC]'}`}
              style={{
                background: 'rgba(32, 34, 38, 0.92)',
                border: '1px solid rgba(255,255,255,0.08)',
                boxShadow: '0 18px 50px rgba(0,0,0,0.45)',
                backdropFilter: 'blur(18px)',
                WebkitBackdropFilter: 'blur(18px)',
              }}
            >
              <div className="flex items-start justify-between gap-3">
                <div className="flex items-center gap-2 min-w-0">
                  <span className={`w-7 h-7 rounded-full flex items-center justify-center shrink-0 ${resolved ? 'bg-[#34C759]/[0.15] text-[#30D158]' : 'bg-[#0A84FF]/[0.16] text-[#7DBDFF]'}`}>
                    {resolved ? <Check size={15} /> : <Brain size={15} />}
                  </span>
                  <div className="min-w-0">
                    <div className="text-[13px] font-semibold leading-tight">{resolved ? (item.statusLabel || '已处理') : '记忆候选'}</div>
                    {!resolved && <div className="text-[12px] leading-tight mt-1 text-[#AEB4BC]">{meta.prompt}</div>}
                  </div>
                </div>
                <div className="shrink-0 flex items-center gap-2">
                  <span className="text-[11px] px-2 py-1 rounded-full bg-white/[0.07] text-[#AEB4BC]">{meta.label}</span>
                  {!resolved && (
                    <button
                      className="w-6 h-6 rounded-full flex items-center justify-center text-[#8E8E93] hover:text-[#F2F3F5] hover:bg-white/[0.08] transition-colors"
                      title="这次忽略"
                      onClick={() => window.TauriBridge && window.TauriBridge.ignoreMemoryCandidate(item.memoryId, item.id)}
                    >
                      <X size={13} />
                    </button>
                  )}
                </div>
              </div>
              <div className="mt-3 ml-9 border-l-2 border-[#0A84FF]/70 pl-3 py-1 text-[14px] leading-relaxed break-words text-[#F2F3F5]">
                “{text}”
              </div>
              {!resolved && <div className="mt-2 ml-9 text-[12px] leading-relaxed text-[#AEB4BC]">{meta.hint}</div>}
              {!resolved && (
                <div className="mt-3 ml-9 flex flex-wrap items-center gap-2">
                  <button className="inline-flex items-center gap-1.5 text-[13px] font-medium px-3.5 py-1.5 rounded-full bg-[#0A84FF] text-white hover:bg-[#1677D2] transition-colors" onClick={() => window.TauriBridge && window.TauriBridge.confirmMemoryCandidate(item.memoryId, item.id)}><Check size={14} />记住</button>
                  <button className="inline-flex items-center gap-1.5 text-[13px] px-3.5 py-1.5 rounded-full bg-white/[0.08] text-[#E8EAED] hover:bg-white/[0.12] transition-colors" onClick={() => window.TauriBridge && window.TauriBridge.ignoreMemoryCandidate(item.memoryId, item.id)}><X size={14} />这次忽略</button>
                  <button className="text-[13px] px-2 py-1.5 rounded-full text-[#AEB4BC] hover:text-[#F2F3F5] hover:bg-white/[0.08] transition-colors" onClick={() => window.TauriBridge && window.TauriBridge.neverMemoryCandidate(item.memoryId, item.id)}>不再提示</button>
                </div>
              )}
            </div>
          </div>
        );
      }

      return null;
    };

    // ==========================================
    // Artifact Card — present_artifact 成品卡（点击打开预览）
    // ==========================================
    // 产物类型 → { 角标/标签文字, tile 配色, lucide 内联 SVG 路径 }（零下载；仅无封面紧凑态显图标）。
    // 配色/字形照搬 产物卡图标预览.html（唯一权威）。

export { ToolWelcomeCard, ComposerKbSelector, ComposerModeChip, ChatView, fallbackCopyText, copyClipboardText, readClipboardText, SelectionCopyButton, TextareaContextMenu, UserBubble, BRAILLE, ThinkingBubble, htmlUnescape, asDraft, asScheduledTaskDraft, extractBalancedJson, parseJsonChain, parseLooseJson, parsePersonaDraft, parseScheduledTaskDraft, parseCardQuestion, optionAnswer, hideStreamingDraft, ChatBubble };
