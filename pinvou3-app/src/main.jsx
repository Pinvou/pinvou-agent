import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import './styles/base.css';
import { I, Plus, Edit2, Trash2, ClipboardList, BarChart2, Settings, Monitor, Smartphone, Brain, BrainCircuit, Clock, Sun, Moon, Zap, Package, RefreshCw, RotateCcw, Search, Upload, Lightbulb, Paperclip, Mic, Send, Store, Terminal, ChevronDown, IconGrid, IconList, ChevronRight, Copy, CheckCircle2, AlertTriangle, Menu, MoreHorizontal, Check, Filter, Database, Download, FolderPlus, Award, Feather, AppWindow, Radio, Palette, Briefcase, Sparkles, StopCircle, XCircle, Wrench, User, Layers, MessageSquare, X, ArrowLeft, FolderOpen, ExternalLink, BookOpen, Code, FileText, Hexagon, Layout, Presentation, Mail, MessageCircle, Navigation, Video, Puzzle, LineChart, Building2, Cpu, Server, Globe, ChevronLeft, XIcon, CloudSun, TrendingUp, TrendingDown, GridIcon, TableIcon, PresentationIcon, ImageIcon, Archive, PinIcon, PinOffIcon } from './components/icons.jsx';
import { ArchiveConfirmDialog, ArchiveToast, ArchivedDeleteConfirmDialog, NavItem, RecentItem } from './components/layout/NavigationComponents.jsx';
import { VllmSetupProgress } from './components/VllmSetupProgress.jsx';
import { bridge, useBridge } from './hooks/useBridge.js';
import { dict, LANG_TO_TAG, SEARCH_KEY_PROVIDERS, TAG_TO_LANG } from './shared/i18n.js';
import { formatSessionDate } from './shared/date-utils.js';
import { KnowledgeView } from './features/knowledge/KnowledgeView.jsx';
import { MonitorView } from './features/monitor/MonitorView.jsx';
import { RemoteControlModal, SettingsView } from './features/settings/SettingsView.jsx';
import { ChatView } from './features/chat/ChatView.jsx';
import { ScheduledTasksView } from './features/scheduled/ScheduledTasksView.jsx';

// 临时止血：定时任务创建流程修复前，不向用户暴露入口或自动跳转。
// 保留后端、数据与页面实现，修复完成后只需恢复此开关。
const SCHEDULED_TASKS_ENTRY_ENABLED = true;
// Static regression anchor: SCHEDULED_TASKS_ENTRY_ENABLED && (<NavItem icon={<Clock size={18} />} label={t.scheduledPlans} unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))} />)
const PREVIEW_SCHEDULED_RUN_SHORTCUTS = [
  { id: 'preview-run-1', automationId: 'preview-daily-brief', taskName: '每日早报', sessionId: 'preview-session-1', status: 'completed', scheduledFor: '2026-07-14T08:00:00+08:00', unread: true },
  { id: 'preview-run-4', automationId: 'preview-follow-up', taskName: '事项督办', sessionId: 'preview-session-4', status: 'running', scheduledFor: '2026-07-14T09:00:00+08:00', unread: false },
  { id: 'preview-run-6', automationId: 'preview-weekly-report', taskName: '销售线索周报', sessionId: 'preview-session-6', status: 'completed', scheduledFor: '2026-07-10T16:00:00+08:00', unread: false },
];
import { ToolStoreView } from './features/tools/ToolStoreView.jsx';
import { PinvouSummonCard } from './features/tools/tool-renderers.jsx';
import { CardPoolView, Lanyard, PersonaEditorModal } from './features/personas/Personas.jsx';
import { WorkflowView } from './features/workflow/WorkflowView.jsx';


/* ==========================================
       Lucide icon replacements (inline SVG)
       ========================================== */
    const appWindow = (window.__TAURI__ && window.__TAURI__.window && window.__TAURI__.window.getCurrentWindow)
      ? window.__TAURI__.window.getCurrentWindow()
      : null;
    const TitleBar = ({ theme, t }) => {
      const isDark = theme === 'dark';
      const hoverBg = isDark ? 'hover:bg-white/10' : 'hover:bg-black/10';
      const chromeBg = isDark ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]';
      return (
        <div data-tauri-drag-region
          className={`h-9 shrink-0 flex items-center justify-between select-none ${chromeBg} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
          <div data-tauri-drag-region className="flex items-center gap-2 px-3 text-[13px] font-medium pointer-events-none">
            <img src="brand-blue.png" width={18} height={18} alt="" className="select-none" />
            {t.appTitle}
          </div>
          <div className="flex items-center h-full">
            <button onClick={() => appWindow && appWindow.minimize()} title={t.winMin}
              className={`h-full w-12 flex items-center justify-center transition-colors ${hoverBg}`}>
              <svg width="11" height="11" viewBox="0 0 11 11"><rect x="1" y="5" width="9" height="1" fill="currentColor"/></svg>
            </button>
            <button onClick={() => appWindow && appWindow.toggleMaximize()} title={t.winMax}
              className={`h-full w-12 flex items-center justify-center transition-colors ${hoverBg}`}>
              <svg width="11" height="11" viewBox="0 0 11 11"><rect x="1.5" y="1.5" width="8" height="8" fill="none" stroke="currentColor" strokeWidth="1"/></svg>
            </button>
            <button onClick={() => appWindow && appWindow.close()} title={t.winClose}
              className="h-full w-12 flex items-center justify-center transition-colors hover:bg-[#E81123] hover:text-white">
              <svg width="11" height="11" viewBox="0 0 11 11"><path d="M1 1 L10 10 M10 1 L1 10" stroke="currentColor" strokeWidth="1.1"/></svg>
            </button>
          </div>
        </div>
      );
    };

    // 桌宠快捷开关的爪印图标(lucide paw-print 风格,icons.jsx 没有现成的)
    const PetPawIcon = () => (
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="11" cy="4" r="2" /><circle cx="18" cy="8" r="2" /><circle cx="4" cy="8" r="2" />
        <path d="M14.35 13.5a4 4 0 0 0-6.7 0c-.63 1.4-1.6 2.4-2.25 3.3-.72 1 .16 3.2 1.9 3.2 1.4 0 2.3-1 4.35-1s2.95 1 4.35 1c1.74 0 2.62-2.2 1.9-3.2-.65-.9-1.62-1.9-2.25-3.3z" />
      </svg>
    );

    /* ==========================================
       MegaCube 本地大模型引导:进行中步骤指示 + 自跑计时
       (后端 vllm-setup:phase 事件给 phase/attempt;秒数本组件自跑,pkexec 阻塞期也在涨)
       ========================================== */
    class SettingsErrorBoundary extends React.Component {
      constructor(props) {
        super(props);
        this.state = { error: null };
      }
      static getDerivedStateFromError(error) {
        return { error };
      }
      render() {
        if (this.state.error) {
          const isDark = this.props.theme === 'dark';
          return (
            <div className="flex-1 flex flex-col w-full h-full relative z-10 px-16 py-12">
              <div className={`max-w-[800px] rounded-2xl border p-5 ${isDark ? 'bg-[#1F2023] border-[#333537] text-[#E8EAED]' : 'bg-white border-[#DDE3EA] text-[#1F1F1F]'}`}>
                <div className="text-[18px] font-semibold mb-2">设置页加载失败</div>
                <div className={`text-[13px] leading-relaxed ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>{String((this.state.error && this.state.error.message) || this.state.error)}</div>
              </div>
            </div>
          );
        }
        return this.props.children;
      }
    }

    const App = () => {
      const bs = useBridge();
      const [activeChat, setActiveChat] = useState(null);
      const [currentView, setCurrentView] = useState('chat');
      const [activeTheme, setActiveTheme] = useState('dark');
      // 供全局事件监听器读取最新视图状态（监听器只注册一次，不能闭包旧值）。
      const activeChatRef = useRef(activeChat);
      activeChatRef.current = activeChat;
      const currentViewRef = useRef(currentView);
      currentViewRef.current = currentView;
      useEffect(() => {
        const liveBridge = window.TauriBridge || bridge;
        if (!liveBridge || typeof liveBridge.startMonitorPolling !== 'function') return;
        if (currentView === 'monitor') {
          liveBridge.startMonitorPolling();
          return () => { if (typeof liveBridge.stopMonitorPolling === 'function') liveBridge.stopMonitorPolling(); };
        }
      }, [currentView]);
      // 工具商店/卡片用 Tailwind dark: 变体(darkMode:'class'),全局挂 <html>.dark 让其随 app 主题切换
      useEffect(() => { document.documentElement.classList.toggle('dark', activeTheme === 'dark'); }, [activeTheme]);
      // MegaCube(GB10) 首屏检测:仅启动一次,检测「预装但未启用」本地大模型环境(后端短路保证普通机零开销)。
      useEffect(() => { if (bridge.available) bridge.detectLocalVllmSetup(); }, []);
      const [vllmDeclineConfirm, setVllmDeclineConfirm] = useState(false); // 引导框「不再提醒」二次确认子态
      const [language, setLanguage] = useState('zh');
      const [superPerm, setSuperPerm] = useState(false);
      const defaultTaskCompletedNotif = !/linux/i.test(`${navigator.platform || ""} ${navigator.userAgent || ""}`);
      const [taskCompletedNotif, setTaskCompletedNotif] = useState(defaultTaskCompletedNotif);
      // search 后端配置:provider 默认 bing(对齐 bridge prefs::SearchProvider::default());
      // bs.settings 加载后 useEffect 同步进来。
      const [searchProvider, setSearchProvider] = useState('bing');
      const [searchApiKey, setSearchApiKey] = useState('');
      const [searchKeyDrafts, setSearchKeyDrafts] = useState({});
      const [searchKeyActions, setSearchKeyActions] = useState({});
      // 模型配置（动态适配）——草稿模式，确认后才保存
      const [modelPreset, setModelPreset] = useState('local_vllm');
      const [customModelName, setCustomModelName] = useState('');
      const [customBaseUrl, setCustomBaseUrl] = useState('');
      const [customApiKey, setCustomApiKey] = useState('');
      const [modelProfiles, setModelProfiles] = useState({});
      const modelConfigInitRef = useRef(false);
      const searchConfigInitRef = useRef(false);
      const uiPrefsInitRef = useRef(false);
      // engine 启动时生效的语言(= 进程启动时的 settings.language)。语言只写盘不重启
      // engine,LLM 的 locale_tag 要重启 app 才更新 —— 草稿偏离此基线就提示「需重启」。
      const bootedLanguageRef = useRef(null);
      // dirty 基线:已保存的模型配置(默认值填充后) / 已保存的搜索源配置。
      // 草稿偏离基线才显示「保存并重启」操作条。
      const savedModelConfigRef = useRef(null);
      const savedSearchConfigRef = useRef(null);

      // 各厂商默认配置（前端自动填充用，与 bridge/mod.rs 对齐）
      const PRESET_DEFAULTS = {
        local_vllm:  { baseUrl: 'http://127.0.0.1:8000/v1',                model: 'qwen36_35b_256k' },
        deepseek:    { baseUrl: 'https://api.deepseek.com',                model: 'deepseek-v4-pro' },
        kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k2.6' },
        openai_compatible: { baseUrl: 'https://api.openai.com/v1',        model: 'gpt-4o' },
        qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen-max' },
        doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-pro-256k' },
        minimax:     { baseUrl: 'https://api.minimax.chat/v1',            model: 'abab6.5s-chat' },
        glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-4-plus' },
        mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2-flash' },
      };
      function normalizedModelProfile(name, baseUrl, apiKey) {
        const modelName = (name || '').trim();
        const endpoint = (baseUrl || '').trim();
        const key = (apiKey || '').trim();
        return {
          model_name: modelName || null,
          base_url: endpoint || null,
          api_key: key || null,
        };
      }
      function modelDraftForPreset(preset, profiles, fallback) {
        const defs = PRESET_DEFAULTS[preset] || PRESET_DEFAULTS.local_vllm;
        const profile = (profiles && profiles[preset]) || {};
        return {
          preset,
          name: profile.model_name || (fallback && fallback.name) || defs.model,
          baseUrl: profile.base_url || (fallback && fallback.baseUrl) || defs.baseUrl,
          apiKey: profile.api_key || (fallback && fallback.apiKey) || '',
        };
      }
      function mergeModelDraft(profiles, preset, name, baseUrl, apiKey) {
        return {
          ...(profiles || {}),
          [preset]: normalizedModelProfile(name, baseUrl, apiKey),
        };
      }
      const [isSidebarOpen, setIsSidebarOpen] = useState(false);
      const [chatPrefill, setChatPrefill] = useState('');
      const composerPrefillSeenRef = useRef(0);
      const scheduledTaskAutoOpenSeenRef = useRef(null);
      const [personaEditor, setPersonaEditor] = useState(null); // 聊天里"存入卡牌池"草稿 → App 级编辑器
      const [savedConfirm, setSavedConfirm] = useState(null); // 存入成功 → iOS 确认窗 {name}
      const [poolMyOnly, setPoolMyOnly] = useState(false); // 跳卡池时是否直接落「我的卡牌」筛选(从确认窗"去查看"进来=true)
      const [remoteOpen, setRemoteOpen] = useState(false);
      const [settingsUpdateFocusTick, setSettingsUpdateFocusTick] = useState(0);
      const [petFocusComposerTick, setPetFocusComposerTick] = useState(0);
      const petSnapshotRef = useRef([]);
      const petSnapshotSequenceRef = useRef(0);

      // ── 多窗口(撕离/tear-off):长按标签 → 浮起跟手 → 拖到目标屏 → 松手 → 该屏最大化打开 ──
      // dragAvatar = 被拎起的标签副本(跟随光标的 DOM 元素);null=没在拖。原生只判落点,视觉全在这。
      const [dragAvatar, setDragAvatar] = useState(null); // {key,label,dx,dy,w,h,x,y}
      const dragOffsetRef = useRef({ dx: 0, dy: 0 });
      const beginTearOff = (kind, id, label, info) => {
        const inv = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke;
        if (!inv || !info) return;
        inv('begin_detach_drag', { kind, id: id != null ? id : null });
        dragOffsetRef.current = { dx: info.dx, dy: info.dy };
        setDragAvatar({
          key: kind + ':' + (id != null ? id : ''), label: label || kind,
          w: info.w, h: info.h, x: info.startX - info.dx, y: info.startY - info.dy,
        });
        if (window.getSelection) { const s = window.getSelection(); if (s && s.removeAllRanges) s.removeAllRanges(); }
      };
      // 拖拽中:光标移动 → 更新 avatar 位置(光标 - 抓取偏移,相对位置锁定);禁选 + 抓手光标。
      useEffect(() => {
        if (!dragAvatar) return;
        const prevUS = document.body.style.userSelect, prevCur = document.body.style.cursor;
        document.body.style.userSelect = 'none';
        document.body.style.cursor = 'grabbing';
        const onMove = (e) => {
          const o = dragOffsetRef.current;
          setDragAvatar(a => a ? { ...a, x: e.clientX - o.dx, y: e.clientY - o.dy } : a);
        };
        window.addEventListener('pointermove', onMove);
        return () => {
          window.removeEventListener('pointermove', onMove);
          document.body.style.userSelect = prevUS;
          document.body.style.cursor = prevCur;
        };
      }, [!!dragAvatar]);
      // 原生拖拽结束(松手/取消)→ 收起 avatar。
      useEffect(() => {
        if (!window.__TAURI__ || !window.__TAURI__.event) return;
        let un;
        window.__TAURI__.event.listen('detach:drag-ended', () => setDragAvatar(null)).then(f => { un = f; });
        return () => { if (un) un(); };
      }, []);

      const t = dict[language];
      // 有可用新版 → 侧边栏设置图标亮红点（不弹窗不打断）
      const hasUpdate = !!(bs && bs.updateInfo && bs.updateInfo.available);

      function handleOpenRemoteControl() {
        setRemoteOpen(true);
      }

      function handleActivateSkill(name) {
        setChatPrefill(t.skillPrefill(name));
        setCurrentView('chat');
      }

      // Sync from bridge state
      useEffect(() => {
        if (!bs) return;
        // activeChat 始终跟随 bridge(含 null:草稿态清掉近期列表高亮)。仅在物化成
        // 真实 session(非 null)时才强制切回 chat 视图——草稿态/删会话不该把用户从
        // monitor/settings 拽走。
        if (bs.activeSessionId !== activeChat) {
          setActiveChat(bs.activeSessionId);
          if (bs.activeSessionId && currentView !== 'monitor' && currentView !== 'settings' && currentView !== 'search' && currentView !== 'scheduled') {
            setCurrentView('chat');
          }
        }
        if (bs.superPermEnabled !== superPerm) setSuperPerm(bs.superPermEnabled);
        if (bs.composerPrefill && bs.composerPrefill.id && bs.composerPrefill.id !== composerPrefillSeenRef.current) {
          composerPrefillSeenRef.current = bs.composerPrefill.id;
          setChatPrefill(bs.composerPrefill.text || '');
          setCurrentView('chat');
        }
        if (SCHEDULED_TASKS_ENTRY_ENABLED && bs.scheduledTaskAutoOpenId && bs.scheduledTaskAutoOpenId !== scheduledTaskAutoOpenSeenRef.current) {
          scheduledTaskAutoOpenSeenRef.current = bs.scheduledTaskAutoOpenId;
          setCurrentView('scheduled');
        }
        // UI 语言/主题:启动时从落盘 settings 恢复一次(此前只写不读,重启即回中文+深色)
        if (!uiPrefsInitRef.current && bs.settings) {
          const lang = TAG_TO_LANG[bs.settings.language];
          if (lang && lang !== language) setLanguage(lang);
          // engine 已用此语言启动,作为「需重启」基线(切语言不重启 engine,见 commands.rs)
          bootedLanguageRef.current = lang || 'zh';
          // 后端 Theme 枚举(prefs.rs)只认 genesis/liquid-light/liquid-dark;深色=genesis,浅色=liquid-light
          const th = bs.settings.theme === 'liquid-light' ? 'light' : 'dark';
          if (th !== activeTheme) setActiveTheme(th);
          const notifications = bs.settings.notifications || {};
          setTaskCompletedNotif(notifications.task_completed !== false && notifications.enabled !== false);
          uiPrefsInitRef.current = true;
        }
        // 搜索配置：只在第一次从后端加载初始值，后续走草稿模式（确认后才保存并重启）。
        if (!searchConfigInitRef.current && bs.settings) {
          const search = bs.settings.search || {};
          const credentials = search.credentials || {};
          const saved = {
            provider: search.provider || 'bing',
            apiKey: search.api_key || '',
            credentials: credentials,
          };
          const drafts = {};
          const actions = {};
          SEARCH_KEY_PROVIDERS.forEach(p => {
            drafts[p] = '';
            actions[p] = 'keep_existing';
          });
          if (saved.apiKey && saved.provider !== 'bing') {
            drafts[saved.provider] = saved.apiKey;
            actions[saved.provider] = 'replace';
          }
          setSearchProvider(saved.provider);
          setSearchApiKey(drafts[saved.provider] || '');
          setSearchKeyDrafts(drafts);
          setSearchKeyActions(actions);
          savedSearchConfigRef.current = saved;
          searchConfigInitRef.current = true;
        }
        // 模型配置：只在第一次从后端加载初始值，后续走草稿模式（确认后才保存），
        // 避免 useEffect 把未保存的本地修改覆盖回 disk 旧值。
        // custom_* 为 null 时用 PRESET_DEFAULTS 填成真实值——输入框显示当前生效配置，
        // 而不是灰色 placeholder 冒充。
        if (!modelConfigInitRef.current && bs.settings) {
          const adv = bs.settings.advanced || {};
          const effective = bs.effectiveModelConfig || {};
          const preset = effective.preset || adv.model_preset || 'local_vllm';
          const profiles = { ...(adv.model_profiles || {}) };
          const fallback = {
            name: effective.model || adv.custom_model_name || '',
            baseUrl: effective.base_url || adv.custom_base_url || '',
            apiKey: effective.api_key || adv.custom_api_key || '',
          };
          const saved = modelDraftForPreset(preset, profiles, fallback);
          profiles[preset] = normalizedModelProfile(saved.name, saved.baseUrl, saved.apiKey);
          setModelProfiles(profiles);
          setModelPreset(saved.preset);
          setCustomModelName(saved.name);
          setCustomBaseUrl(saved.baseUrl);
          setCustomApiKey(saved.apiKey);
          savedModelConfigRef.current = saved;
          modelConfigInitRef.current = true;
        }
      }, [bs]);

      // HMR/旧前端状态可能仍停在 scheduled；入口关闭时立即回到普通聊天页。
      useEffect(() => {
        if (!SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled') {
          setCurrentView('chat');
        }
      }, [currentView]);

      // 草稿 vs 已保存基线 → 模型卡是否显示「保存并重启」操作条
      const savedModel = savedModelConfigRef.current;
      const modelConfigDirty = !!savedModel && (
        modelPreset !== savedModel.preset ||
        customModelName !== savedModel.name ||
        customBaseUrl !== savedModel.baseUrl ||
        customApiKey !== savedModel.apiKey
      );
      function normalizedSearchApiKeyValue(value) {
        const trimmed = (value || '').trim();
        return trimmed ? trimmed : null;
      }
      function searchCredentialForProvider(provider) {
        const saved = savedSearchConfigRef.current;
        return (saved && saved.credentials && saved.credentials[provider]) || {};
      }
      function searchHasSavedKey(provider) {
        const credential = searchCredentialForProvider(provider);
        const state = credential.credential_state || (credential.has_secret ? 'configured' : 'missing');
        return !!credential.has_secret || state === 'configured' || state === 'env_override';
      }
      function searchProviderKeyAction(provider) {
        return searchKeyActions[provider] || 'keep_existing';
      }
      function searchProviderCredentialDirty(provider) {
        const action = searchProviderKeyAction(provider);
        const draft = searchKeyDrafts[provider] || '';
        return action === 'delete' || (action === 'replace' && !!draft.trim());
      }
      function buildSearchSettingsPayload() {
        const baseSearch = (bs && bs.settings && bs.settings.search) || {};
        const credentials = { ...(baseSearch.credentials || {}) };
        SEARCH_KEY_PROVIDERS.forEach(provider => {
          const action = searchProviderKeyAction(provider);
          const draft = searchKeyDrafts[provider] || '';
          if (action === 'delete' || (action === 'replace' && draft.trim())) {
            credentials[provider] = {
              ...(credentials[provider] || {}),
              api_key: action === 'replace' ? draft.trim() : '',
              credential_action: action,
            };
          }
        });
        return {
          ...baseSearch,
          provider: searchProvider,
          api_key: null,
          credentials,
        };
      }
      // 搜索配置也影响 EngineConfig,需保存后重启进程才生效。
      const savedSearch = savedSearchConfigRef.current;
      const searchCredentialDirty = SEARCH_KEY_PROVIDERS.some(searchProviderCredentialDirty);
      const searchNeedsRestart = !!savedSearch && (
        searchProvider !== savedSearch.provider ||
        searchCredentialDirty
      );
      // 语言已即时写盘+切 UI,但 LLM 的 locale_tag 要重启 engine 才生效 → 偏离启动语言就提示。
      const languageNeedsRestart = !!bootedLanguageRef.current && language !== bootedLanguageRef.current;

      // Build chat history from sessions
      const skillBindings = (bs && bs.workflow && bs.workflow.bindings) || {};
      const sessionBusy = (bs && bs.sessionBusy) || {};
      const chatHistory = bs && bs.sessions ? bs.sessions.map(s => ({
        id: s.id,
        // 后端默认标题是字面 "新对话"/"New chat"(bridge 以此判断是否自动改名)——显示层映射成当前语言
        title: (!s.title || s.title === '新对话' || s.title === 'New chat') ? t.newChat : s.title,
        date: formatSessionDate(s.updated_at || s.created_at, language),
        updatedAt: s.updated_at || s.created_at || '',
        pinned: !!s.pinned,
        pinnedAt: s.pinned_at || '',
        skill: skillBindings[s.id] || null,
        working: !!sessionBusy[s.id], // 多 session 并发:该 session 是否正在后台生成
      })) : [];
      const pinnedChatHistory = chatHistory
        .filter(chat => chat.pinned)
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt)));
      const scheduledRunShortcuts = (bs && bs.scheduledTaskRecentRuns && bs.scheduledTaskRecentRuns.length)
        ? bs.scheduledTaskRecentRuns
        : (!bridge.available ? PREVIEW_SCHEDULED_RUN_SHORTCUTS : []);
      const scheduledRunSessionIds = new Set(
        scheduledRunShortcuts
          .map(run => run && run.sessionId)
          .filter(Boolean)
      );
      const scheduledRunBySessionId = Object.create(null);
      scheduledRunShortcuts.forEach(run => {
        if (run && run.sessionId) scheduledRunBySessionId[run.sessionId] = run;
      });
      const regularHistory = chatHistory
        .filter(chat => !chat.pinned && !scheduledRunSessionIds.has(chat.id))
        .sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)));
      const scheduledRunItems = scheduledRunShortcuts
        .filter(run => run && run.sessionId)
        .map(run => {
          // 定时运行会话不进 bs.sessions(list_sessions 隔离 sched-*),标题/置顶
          // 状态由后端 run DTO 直接携带。
          const rawTitle = run.sessionTitle || '';
          const title = (!rawTitle || rawTitle === '新对话' || rawTitle === 'New chat')
            ? (run.taskName || '定时任务')
            : rawTitle;
          return {
            id: run.sessionId,
            title,
            subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
            date: '',
            updatedAt: run.createdAt || run.scheduledFor || '',
            pinned: !!run.pinned,
            pinnedAt: run.pinnedAt || '',
            working: run.status === 'running' || run.status === 'queued',
            leadingIcon: (
              <span className="relative inline-flex h-5 w-5 items-center justify-center">
                <Clock size={16} />
                {run.unread && (
                  <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full border-2"
                    style={{ background: '#0B57D0', borderColor: activeTheme === 'dark' ? '#1E1F20' : '#F0F4F9' }} />
                )}
              </span>
            ),
            testId: 'scheduled-run-sidebar-item',
            menuTestId: 'scheduled-run-sidebar-menu',
            scheduledRun: run,
          };
        });
      const scheduledRunHistory = scheduledRunItems.filter(chat => !chat.pinned);
      const pinnedHistory = pinnedChatHistory
        .concat(scheduledRunItems.filter(chat => chat.pinned))
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt)));

      function decorateScheduledRunChat(chat, run) {
        if (!run) return chat;
        const title = (!chat.title || chat.title === t.newChat || chat.title === '新对话' || chat.title === 'New chat')
          ? (run.taskName || '定时任务')
          : chat.title;
        return Object.assign({}, chat, {
          title,
          subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
          leadingIcon: (
            <span className="relative inline-flex h-5 w-5 items-center justify-center">
              <Clock size={16} />
              {run.unread && (
                <span className="absolute -right-1 -top-1 h-2.5 w-2.5 rounded-full border-2"
                  style={{ background: '#0B57D0', borderColor: activeTheme === 'dark' ? '#1E1F20' : '#F0F4F9' }} />
              )}
            </span>
          ),
          testId: 'scheduled-run-sidebar-item',
          menuTestId: 'scheduled-run-sidebar-menu',
          scheduledRun: run,
        });
      }

      const [justInstalledTool, setJustInstalledTool] = useState(null);
      const [historyOpen, setHistoryOpen] = useState({ pinned: true, scheduledRuns: true, regular: true });
      const [archiveConfirm, setArchiveConfirm] = useState(null);
      const [archiveToast, setArchiveToast] = useState(false);

      petSnapshotRef.current = chatHistory.map(chat => ({
        id: chat.id,
        title: chat.title,
        working: chat.working,
      }));
      useEffect(() => {
        const ev = window.__TAURI__ && window.__TAURI__.event;
        if (!ev) return undefined;
        let disposed = false;
        let unlisten = null;
        const broadcast = () => ev.emit('pet:activity_snapshot', {
          sequence: ++petSnapshotSequenceRef.current,
          sessions: petSnapshotRef.current,
        }).catch(() => {});
        broadcast();
        ev.listen('pet:request_snapshot', broadcast).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        return () => {
          disposed = true;
          if (unlisten) unlisten();
        };
      }, [bs && bs.sessions, bs && bs.sessionBusy, language]);

      async function navigateFromScheduledRun(nextView, beforeNavigate) {
        if (bs && bs.scheduledRunContext && bridge.available && bridge.exitScheduledRunChat) {
          const exited = await bridge.exitScheduledRunChat();
          if (!exited) return false;
        }
        if (beforeNavigate) beforeNavigate();
        setCurrentView(nextView);
        return true;
      }

      function scheduledRunLabel(value) {
        return ({
          queued: '等待中',
          running: '运行中',
          completed: '已完成',
          failed: '失败',
          canceled: '已取消',
        }[value] || value || '未知');
      }

      async function handleOpenScheduledRunShortcut(run) {
        if (!run || !run.sessionId) return;
        if (!bridge.available || !bridge.openScheduledRunChat) {
          setCurrentView('scheduled');
          return;
        }
        const task = {
          id: run.automationId,
          name: run.taskName || t.scheduledPlans,
          model: run.taskModel || null,
        };
        const opened = await bridge.openScheduledRunChat(run, task);
        if (opened) setCurrentView('scheduled');
      }

      function handleNewChat(installedToolId) {
        // 类型守卫:installedToolId 必须是字符串 toolId。侧边栏按钮 onClick={() => handleNewChat()}
        // 本不传参,但若哪天有调用点写成 onClick={handleNewChat},React 会把事件对象当首参塞进来——
        // 那是 truthy 的 SyntheticEvent,会被当成 toolId 置进 welcomeToolId → ToolWelcomeCard 查不到
        // 工具渲染 null → 欢迎语整块空白。守卫挡住这条暗坑。
        if (typeof installedToolId === 'string' && installedToolId) {
          setJustInstalledTool(installedToolId);
        }
        if (bridge.available) bridge.createNewSession();
        setCurrentView('chat');
      }

      // AI 造卡:新对话 + 加持「卡牌制造专家」+ 一条 iOS 引导卡 → 用户在空输入框描述需求,复用 persona-card 草稿流程入库
      async function startAICard() {
        handleNewChat();
        if (!bridge.available) return;
        await bridge.equipPersona('pinvou-card-creator');           // 先加持(落新 session + 加持气泡)
        bridge.postCardCreatorIntro();                              // 再排在加持气泡之后(持久化,切会话/重启不丢)
      }

      async function handleSwitchSession(id) {
        if (!bridge.available) return;
        const switched = await bridge.switchToSession(id);
        if (!switched) return;
        setActiveChat(id);
        setCurrentView('chat');
      }

      // 用户在主窗口里亲眼看着完成的会话，公仔的活动卡属于冗余提醒——
      // 完成瞬间若该会话正处于前台聊天视图且窗口有焦点，直接标记已读，
      // 卡片自动消失，不需要用户再去点。
      useEffect(() => {
        const tauri = window.__TAURI__;
        const ev = tauri && tauri.event;
        if (!ev) return undefined;
        let disposed = false;
        const unlisteners = [];
        const emitToPet = (name, payload) => (
          typeof ev.emitTo === 'function'
            ? ev.emitTo('pet', name, payload)
            : ev.emit(name, payload)
        );
        ev.listen('chat:done', (event) => {
          if (disposed) return;
          const payload = event.payload || {};
          const sid = payload.session_id || payload.sessionId;
          if (!sid) return;
          if (typeof document.hasFocus === 'function' && !document.hasFocus()) return;
          if (currentViewRef.current !== 'chat') return;
          if (String(activeChatRef.current) !== String(sid)) return;
          emitToPet('pet:session_viewed', {
            session_id: sid,
            completed: true,
          }).catch(() => {});
        }).then((unlisten) => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        return () => {
          disposed = true;
          unlisteners.forEach((fn) => { try { fn(); } catch (_) {} });
        };
      }, []);

      // 用户从侧栏切进一个已经完成的会话时，也立即收掉对应完成气泡。
      // 运行中的卡不会被 markSessionViewed 删除；等它完成时，上面的
      // chat:done 监听会再次确认当前画面并完成收尾。
      useEffect(() => {
        const ev = window.__TAURI__ && window.__TAURI__.event;
        if (!ev || currentView !== 'chat' || !activeChat) return;
        if (typeof document.hasFocus === 'function' && !document.hasFocus()) return;
        const emit = typeof ev.emitTo === 'function'
          ? ev.emitTo('pet', 'pet:session_viewed', { session_id: activeChat })
          : ev.emit('pet:session_viewed', { session_id: activeChat });
        emit.catch(() => {});
      }, [currentView, activeChat]);

      useEffect(() => {
        const tauri = window.__TAURI__;
        const ev = tauri && tauri.event;
        const core = tauri && tauri.core;
        if (!ev || !core) return undefined;
        const emitToPet = (name, payload) => (
          typeof ev.emitTo === 'function'
            ? ev.emitTo('pet', name, payload)
            : ev.emit(name, payload)
        );
        let disposed = false;
        let consuming = false;
        const unlisteners = [];
        const consumePetNavigation = async () => {
          if (disposed || consuming) return;
          consuming = true;
          try {
            const request = await core.invoke('take_pet_navigation');
            if (!request || disposed) return;
            const scheduledRun = request.scheduled_run || request.scheduledRun;
            if (scheduledRun) {
              const automationId = scheduledRun.automationId || scheduledRun.automation_id;
              const runId = scheduledRun.runId || scheduledRun.run_id;
              const sessionId = scheduledRun.sessionId || scheduledRun.session_id;
              const taskName = scheduledRun.taskName || scheduledRun.task_name;
              const endedAt = scheduledRun.endedAt || scheduledRun.ended_at;
              if (!bridge.available || !bridge.openScheduledRunChat) {
                emitToPet('pet:scheduled_notice_open_failed', { run_id: runId }).catch(() => {});
                return;
              }
              let opened = false;
              try {
                opened = await bridge.openScheduledRunChat({
                  id: runId,
                  automationId,
                  sessionId,
                  status: 'completed',
                  endedAt,
                  unread: true,
                }, {
                  id: automationId,
                  name: taskName,
                });
              } catch (error) {
                console.error('[pet scheduled navigation] open failed', error);
              }
              if (!opened) {
                emitToPet('pet:scheduled_notice_open_failed', { run_id: runId }).catch(() => {});
                return;
              }
              setActiveChat(sessionId);
              setCurrentView('scheduled');
              emitToPet('pet:scheduled_notice_opened', { run_id: runId }).catch(() => {});
              return;
            }
            const sid = request.session_id || request.sessionId;
            if (!sid) {
              setCurrentView('chat');
              setPetFocusComposerTick(value => value + 1);
              return;
            }
            if (!bridge.available) return;
            const sessionExists = petSnapshotRef.current.some((session) => String(session.id) === String(sid));
            if (!sessionExists) {
              setCurrentView('chat');
              setPetFocusComposerTick(value => value + 1);
              emitToPet('pet:session_unavailable', { session_id: sid }).catch(() => {});
              return;
            }
            const switched = await bridge.switchToSession(sid);
            if (!switched) {
              emitToPet('pet:session_unavailable', { session_id: sid }).catch(() => {});
              return;
            }
            setActiveChat(sid);
            setCurrentView('chat');
            setPetFocusComposerTick(value => value + 1);
            emitToPet('pet:session_viewed', { session_id: sid }).catch(() => {});
          } catch (error) {
            console.error('[pet navigation] consume failed', error);
          } finally {
            consuming = false;
          }
        };
        const subscriptions = [ev.listen('pet:navigation_pending', consumePetNavigation)];
        window.addEventListener('focus', consumePetNavigation);
        void consumePetNavigation();
        Promise.all(subscriptions).then((items) => {
          if (disposed) items.forEach(fn => fn());
          else unlisteners.push(...items);
        }).catch(() => {});
        return () => {
          disposed = true;
          window.removeEventListener('focus', consumePetNavigation);
          unlisteners.forEach(fn => { try { fn(); } catch (_) {} });
        };
      }, []);

      useEffect(() => {
        const tauri = window.__TAURI__;
        const ev = tauri && tauri.event;
        const core = tauri && tauri.core;
        if (!ev || !core || !bridge.available || !bridge.sendMessageToSession) return undefined;
        let disposed = false;
        let consuming = false;
        let rerun = false;
        let unlisten = null;
        const emitToPet = (name, payload) => (
          typeof ev.emitTo === 'function' ? ev.emitTo('pet', name, payload) : ev.emit(name, payload)
        );
        const consume = async () => {
          if (disposed) return;
          if (consuming) {
            rerun = true;
            return;
          }
          consuming = true;
          try {
            if (typeof bridge.init === 'function') await bridge.init();
            while (!disposed) {
              const request = await core.invoke('take_pet_reply');
              if (!request) break;
              const requestId = request.request_id || request.requestId;
              const sid = request.session_id || request.sessionId;
              const text = String(request.text || '').trim();
              const liveSessions = typeof bridge.getState === 'function'
                ? (bridge.getState().sessions || [])
                : [];
              const sessionExists = petSnapshotRef.current.some(
                session => String(session.id) === String(sid),
              ) || liveSessions.some(session => String(session.id) === String(sid));
              if (!sessionExists) {
                await emitToPet('pet:reply_failed', {
                  request_id: requestId,
                  session_id: sid,
                  error: '目标会话不存在',
                  unavailable: true,
                }).catch(() => {});
                continue;
              }
              try {
                const result = await bridge.sendMessageToSession(sid, text);
                await emitToPet('pet:reply_accepted', {
                  request_id: requestId,
                  session_id: sid,
                }).catch(() => {});
                if (result?.completion) {
                  result.completion.then((outcome) => {
                    if (outcome?.ok) return;
                    return emitToPet('pet:reply_failed', {
                      request_id: requestId,
                      session_id: sid,
                      error: String(outcome?.error?.message || outcome?.error || '任务未能启动'),
                    }).catch(() => {});
                  });
                }
              } catch (error) {
                await emitToPet('pet:reply_failed', {
                  request_id: requestId,
                  session_id: sid,
                  error: String(error && error.message ? error.message : error),
                }).catch(() => {});
              }
            }
          } catch (error) {
            console.error('[pet reply] consume failed', error);
          } finally {
            consuming = false;
            if (rerun && !disposed) {
              rerun = false;
              void consume();
            }
          }
        };
        ev.listen('pet:reply_pending', consume).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        void consume();
        return () => {
          disposed = true;
          if (unlisten) unlisten();
        };
      }, []);

      function handleDeleteSession(id) {
        if (bridge.available) bridge.deleteSession(id);
      }

      function handleRenameSession(id, title) {
        if (bridge.available) bridge.renameSession(id, title);
      }

      function handleToggleSessionPinned(id, pinned) {
        if (bridge.available) bridge.toggleSessionPinned(id, pinned);
      }

      function handleArchiveSession(id) {
        const chat = chatHistory.find(c => c.id === id) || scheduledRunItems.find(c => c.id === id);
        setArchiveConfirm(chat || { id, title: t.newChat });
      }

      function confirmArchiveSession() {
        const id = archiveConfirm && archiveConfirm.id;
        setArchiveConfirm(null);
        if (id && bridge.available) {
          bridge.archiveSession(id);
          setArchiveToast(true);
        }
      }

      function handleRestoreArchivedSession(id) {
        if (bridge.available) bridge.restoreArchivedSession(id);
      }

      useEffect(() => {
        if (!archiveToast) return;
        const timer = setTimeout(() => setArchiveToast(false), 3500);
        return () => clearTimeout(timer);
      }, [archiveToast]);

      function handleToggleSuperPerm() {
        if (bridge.available) bridge.toggleSuperPerm();
        else setSuperPerm(!superPerm);
      }

      // 构造完整 UserPrefs 对象写盘。spread bs.settings 保留 search/advanced 等
      // 其他字段——之前漏 spread 会让后端 serde default 把它们重置(bug fix)。
      function buildFullSettings(overrides) {
        const base = (bs && bs.settings) ? bs.settings : {};
        const advancedOverrides = (overrides && overrides.advanced) ? overrides.advanced : {};
        const searchOverrides = (overrides && overrides.search) ? overrides.search : null;
        const topOverrides = { ...(overrides || {}) };
        delete topOverrides.advanced;
        delete topOverrides.search;
        delete topOverrides.notifications;
        const baseSearch = base.search || { provider: 'bing', api_key: null, credentials: {} };
        const nextLanguage = topOverrides.language !== undefined ? topOverrides.language : (LANG_TO_TAG[language] || 'zh-Hans');
        const memoryAvailable = nextLanguage === 'zh-Hans';
        const nextMemoryEnabled = memoryAvailable
          ? (topOverrides.memory_enabled !== undefined ? !!topOverrides.memory_enabled : !!base.memory_enabled)
          : false;
        const baseNotifications = base.notifications || { enabled: defaultTaskCompletedNotif, task_completed: defaultTaskCompletedNotif };
        const notificationOverrides = (overrides && overrides.notifications) ? overrides.notifications : null;
        return {
          ...base,
          ...topOverrides,
          theme: topOverrides.theme !== undefined ? topOverrides.theme : (activeTheme === 'dark' ? 'genesis' : 'liquid-light'),
          language: nextLanguage,
          memory_enabled: nextMemoryEnabled,
          search: searchOverrides ? { ...baseSearch, ...searchOverrides } : baseSearch,
          notifications: notificationOverrides ? { ...baseNotifications, ...notificationOverrides } : baseNotifications,
          advanced: buildAdvancedOverrides(advancedOverrides),
        };
      }

      function handleSetTheme(th) {
        setActiveTheme(th);
        if (bridge.available) {
          bridge.saveSettings(buildFullSettings({ theme: th === 'dark' ? 'genesis' : 'liquid-light' }));
        }
      }

      function handleSetSearchProvider(p) {
        if (p === searchProvider) return;
        setSearchProvider(p);
        setSearchApiKey(searchKeyDrafts[p] || '');
      }

      function handleSetSearchApiKey(k) {
        setSearchApiKey(k);
        setSearchKeyDrafts(prev => ({ ...prev, [searchProvider]: k }));
        setSearchKeyActions(prev => ({ ...prev, [searchProvider]: k.trim() ? 'replace' : 'keep_existing' }));
      }

      function handleKeepSearchApiKey() {
        setSearchApiKey('');
        setSearchKeyDrafts(prev => ({ ...prev, [searchProvider]: '' }));
        setSearchKeyActions(prev => ({ ...prev, [searchProvider]: 'keep_existing' }));
      }

      function handleReplaceSearchApiKey() {
        setSearchKeyActions(prev => ({ ...prev, [searchProvider]: 'replace' }));
      }

      function handleDeleteSearchApiKey() {
        setSearchApiKey('');
        setSearchKeyDrafts(prev => ({ ...prev, [searchProvider]: '' }));
        setSearchKeyActions(prev => ({ ...prev, [searchProvider]: 'delete' }));
      }

      function handleConfirmSearchConfig() {
        if (bridge.available) {
          bridge.saveSettingsAndRestart(buildFullSettings({
            search: buildSearchSettingsPayload(),
          }));
        }
      }

      function handleSetLanguage(lang) {
        setLanguage(lang);
        if (bridge.available) {
          const nextLanguage = LANG_TO_TAG[lang] || 'zh-Hans';
          bridge.saveSettings(buildFullSettings({ language: nextLanguage }));
        }
      }

      function handleSetMemoryEnabled(enabled) {
        if (bridge.available) {
          const memoryAvailable = (LANG_TO_TAG[language] || 'zh-Hans') === 'zh-Hans';
          bridge.saveSettings(buildFullSettings({ memory_enabled: memoryAvailable && !!enabled }));
        }
      }

      function handleSetPetEnabled(enabled) {
        if (!bridge.available) return;
        // 单一路径:set_pet_enabled 负责持久化 + 窗口显隐 + 广播
        // pet:enabled_changed(bridge 听到后刷新 settings 副本,防旧值回写)。
        window.__TAURI__.core.invoke('set_pet_enabled', { enabled: !!enabled }).catch(() => {});
      }

      async function handleSetTaskCompletedNotif(enabled) {
        const nextEnabled = !!enabled;
        const previousEnabled = taskCompletedNotif;
        setTaskCompletedNotif(nextEnabled);
        if (bridge.available) {
          const saved = await bridge.saveSettings(buildFullSettings({
            notifications: { enabled: nextEnabled, task_completed: nextEnabled },
          }));
          if (saved === false) {
            setTaskCompletedNotif(previousEnabled);
          }
        }
      }

      function buildAdvancedOverrides(overrides) {
        const baseAdvanced = (bs && bs.settings && bs.settings.advanced) ? bs.settings.advanced : {};
        const nextPreset = overrides.model_preset !== undefined ? overrides.model_preset : modelPreset;
        const nextModelName = overrides.custom_model_name !== undefined ? overrides.custom_model_name : customModelName;
        const nextBaseUrl = overrides.custom_base_url !== undefined ? overrides.custom_base_url : customBaseUrl;
        const nextApiKey = overrides.custom_api_key !== undefined ? overrides.custom_api_key : customApiKey;
        const nextProfiles = {
          ...(baseAdvanced.model_profiles || {}),
          ...(modelProfiles || {}),
          [nextPreset]: normalizedModelProfile(nextModelName, nextBaseUrl, nextApiKey),
        };
        return {
          ...baseAdvanced,
          ...overrides,
          model_preset: nextPreset,
          custom_model_name: nextModelName || null,
          custom_base_url: nextBaseUrl || null,
          custom_api_key: nextApiKey || null,
          model_profiles: nextProfiles,
        };
      }

      // 模型配置改为草稿模式：只更新 state，点击确认后统一保存并重启
      function handleChangeModelPreset(p) {
        const nextProfiles = mergeModelDraft(modelProfiles, modelPreset, customModelName, customBaseUrl, customApiKey);
        const saved = savedModelConfigRef.current;
        if (saved && p === saved.preset) {
          // 切回已保存的来源 → 还原已保存值（而非厂商默认），dirty 自然归零
          setModelProfiles(nextProfiles);
          setModelPreset(saved.preset);
          setCustomModelName(saved.name);
          setCustomBaseUrl(saved.baseUrl);
          setCustomApiKey(saved.apiKey);
          return;
        }
        const draft = modelDraftForPreset(p, nextProfiles);
        setModelProfiles(nextProfiles);
        setModelPreset(p);
        setCustomBaseUrl(draft.baseUrl);
        setCustomModelName(draft.name);
        setCustomApiKey(draft.apiKey);
      }
      function handleSetCustomModelName(v) {
        setCustomModelName(v);
      }
      function handleSetCustomBaseUrl(v) {
        setCustomBaseUrl(v);
      }
      function handleSetCustomApiKey(v) {
        setCustomApiKey(v);
      }
      function handleConfirmModelConfig() {
        if (bridge.available) {
          bridge.saveSettingsAndRestart(buildFullSettings({
            advanced: {
              model_preset: modelPreset,
              custom_model_name: customModelName || null,
              custom_base_url: customBaseUrl || null,
              custom_api_key: customApiKey || null,
            },
          }));
        }
      }

      return (
        <div data-testid="app-root" data-current-view={currentView} className={`flex flex-col h-screen font-sans overflow-hidden antialiased transition-colors duration-300 ${activeTheme === 'dark' ? 'bg-[#131314] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>

          {/* 撕离拖拽 avatar:被拎起的标签,跟随光标(DOM 实现,丝滑跟手、不选中文字) */}
          {dragAvatar && (
            <div style={{ position:'fixed', left: dragAvatar.x, top: dragAvatar.y, width: dragAvatar.w, height: dragAvatar.h,
              pointerEvents:'none', zIndex:9999, borderRadius:14, overflow:'hidden', whiteSpace:'nowrap',
              display:'flex', alignItems:'center', padding:'0 16px', fontWeight:600, fontSize:15,
              background: activeTheme === 'dark' ? '#A8C7FA' : '#0B57D0', color: activeTheme === 'dark' ? '#041E49' : '#ffffff',
              boxShadow:'0 14px 34px rgba(0,0,0,.5)', transform:'scale(1.03)', opacity:0.96 }}>
              {dragAvatar.label}
            </div>
          )}

          {archiveConfirm && createPortal(
            <ArchiveConfirmDialog
              theme={activeTheme}
              t={t}
              onCancel={() => setArchiveConfirm(null)}
              onConfirm={confirmArchiveSession}
            />,
            document.body
          )}

          {archiveToast && createPortal(
            <ArchiveToast
              theme={activeTheme}
              t={t}
              onClose={() => setArchiveToast(false)}
              onView={() => {
                setArchiveToast(false);
                setCurrentView('settings');
              }}
            />,
            document.body
          )}

          <TitleBar theme={activeTheme} t={t} />

          <div className={`flex flex-1 min-h-0 ${activeTheme === 'dark' ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]'}`}>

          {/* ================= Sidebar (Gemini Style) ================= */}
          <div className={`${isSidebarOpen ? 'w-[280px]' : 'w-[68px]'} shrink-0 flex flex-col z-40 transition-all duration-300 ${activeTheme === 'dark' ? 'bg-[#1E1F20]' : 'bg-[#F0F4F9]'}`}>

            {/* Header / Logo */}
            <div className={`px-4 py-4 flex items-center ${isSidebarOpen ? 'gap-3' : 'justify-center'} overflow-hidden`}>
              <button
                data-sidebar-toggle
                onClick={() => setIsSidebarOpen(!isSidebarOpen)}
                title={isSidebarOpen ? t.sidebarCollapse : t.sidebarExpand}
                className={`w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
              >
                <Menu size={20} className={activeTheme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#444746]'} />
              </button>
              <span className={`text-[18px] font-medium tracking-wide flex items-center gap-2 whitespace-nowrap transition-opacity duration-200 ${isSidebarOpen ? 'opacity-100' : 'opacity-0 w-0'}`}>
                PINVOU
              </span>
            </div>

            {/* Navigation — shrink-0 固定不滚动,list 再多也不挤压 nav */}
            <div className={`shrink-0 flex flex-col gap-1 mt-3 ${isSidebarOpen ? 'px-3' : 'px-2 items-center'}`}>
              <NavItem
                icon={<Edit2 size={18} />} label={t.newChat}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => handleNewChat()}
              />
              <NavItem
                icon={<Search size={18} />} label={t.searchChats}
                active={currentView === 'search'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('search')}
              />
              {SCHEDULED_TASKS_ENTRY_ENABLED && (
                <NavItem
                  icon={<Clock size={18} />} label={t.scheduledPlans}
                  active={currentView === 'scheduled'}
                  unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => navigateFromScheduledRun('scheduled')}
                />
              )}
              <NavItem
                icon={<BarChart2 size={18} />} label={t.monitor}
                active={currentView === 'monitor'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => {
                  navigateFromScheduledRun('monitor', () => {
                    const liveBridge = window.TauriBridge || bridge;
                    if (liveBridge && typeof liveBridge.startMonitorPolling === 'function') liveBridge.startMonitorPolling();
                  });
                }}
                dragKind="monitor" dragging={!!dragAvatar && dragAvatar.key === 'monitor:'} onPickUp={(geom) => beginTearOff('monitor', undefined, t.monitor, geom)}
              />
              <NavItem
                icon={<Layers size={18} />} label={t.cardPool}
                active={currentView === 'cardpool'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))}
                dragKind="cardpool" dragging={!!dragAvatar && dragAvatar.key === 'cardpool:'} onPickUp={(geom) => beginTearOff('cardpool', undefined, t.cardPool, geom)}
              />
              <NavItem
                icon={<ClipboardList size={18} />} label={t.workflow}
                active={currentView === 'workflow'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('workflow')}
                dragKind="workflow" dragging={!!dragAvatar && dragAvatar.key === 'workflow:'} onPickUp={(geom) => beginTearOff('workflow', undefined, t.workflow, geom)}
              />
              <NavItem
                icon={<Puzzle size={18} />} label={t.toolStore}
                active={currentView === 'toolStore'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('toolStore')}
                dragKind="toolstore" dragging={!!dragAvatar && dragAvatar.key === 'toolstore:'} onPickUp={(geom) => beginTearOff('toolstore', undefined, t.toolStore, geom)}
              />
              <NavItem
                icon={<BookOpen size={18} />} label={t.knowledge}
                active={currentView === 'knowledge'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('knowledge')}
                dragKind="knowledge" dragging={!!dragAvatar && dragAvatar.key === 'knowledge:'} onPickUp={(geom) => beginTearOff('knowledge', undefined, t.knowledge, geom)}
              />
              {/* 收起态专属:展开态近期列表的高亮项就是回会话入口,不重复渲染 */}
              {!isSidebarOpen && (
                <NavItem
                  icon={<MessageSquare size={18} />} label={t.currentChat}
                  active={currentView === 'chat'}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => navigateFromScheduledRun('chat')}
                />
              )}
            </div>

            {/* Recents — 独立 flex-1 + overflow-y-auto,只在展开态显示。
                min-h-0 关键:flex 子项默认 min-height: auto 会阻止 overflow,
                显式压成 0 才允许内容溢出触发滚动条。
                nav / list 分隔:「近期」label sticky top-0 + 实色背景,滚动时常驻顶端
                遮住下滑的列表项,避免首项与上方 nav 贴死("重合")。 */}
            {isSidebarOpen && (
              <div className="flex-1 min-h-0 overflow-y-auto custom-scrollbar px-3 flex flex-col">
                <div className="pt-5 pb-2 space-y-4">
                  {pinnedHistory.length > 0 && (
                    <div>
                      <button
                        type="button"
                        onClick={() => setHistoryOpen(prev => ({ ...prev, pinned: !prev.pinned }))}
                        className={`w-full h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                      >
                        <span className="truncate">{t.pinnedTasks} ({pinnedHistory.length})</span>
                        <ChevronDown size={16} className={`shrink-0 transition-transform ${historyOpen.pinned ? '' : '-rotate-90'}`} />
                      </button>
                      {historyOpen.pinned && (
                        <div className="mt-1 space-y-px">
                          {pinnedHistory.map((chat) => {
                            const run = scheduledRunBySessionId[chat.id];
                            const item = decorateScheduledRunChat(chat, run);
                            return (
                              <RecentItem
                                key={chat.id}
                                chat={item}
                                theme={activeTheme}
                                t={t}
                                active={run
                                  ? !!(bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === chat.id)
                                  : activeChat === chat.id && currentView === 'chat'}
                                personaTarget={!run && activeChat === chat.id && currentView === 'cardpool'}
                                onSelect={run ? () => handleOpenScheduledRunShortcut(run) : handleSwitchSession}
                                onRename={handleRenameSession}
                                onDelete={handleDeleteSession}
                                onTogglePinned={handleToggleSessionPinned}
                                onOpenFolder={(id) => bridge.revealSessionFolder && bridge.revealSessionFolder(id)}
                                onArchive={handleArchiveSession}
                                dragging={!!dragAvatar && dragAvatar.key === 'session:' + chat.id}
                                onPickUp={(geom) => beginTearOff('session', chat.id, item.title, geom)}
                              />
                            );
                          })}
                        </div>
                      )}
                    </div>
                  )}
                  <div>
                    <button
                      type="button"
                      onClick={() => setHistoryOpen(prev => ({ ...prev, regular: !prev.regular }))}
                      className={`w-full h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                    >
                      <span className="truncate">{t.regularTasks} ({regularHistory.length})</span>
                      <ChevronDown size={16} className={`shrink-0 transition-transform ${historyOpen.regular ? '' : '-rotate-90'}`} />
                    </button>
                    {historyOpen.regular && (
                      <div className="mt-1 space-y-px">
                        {regularHistory.map((chat) => (
                          <RecentItem
                            key={chat.id}
                            chat={chat}
                            theme={activeTheme}
                            t={t}
                            active={activeChat === chat.id && currentView === 'chat'}
                            personaTarget={activeChat === chat.id && currentView === 'cardpool'}
                            onSelect={handleSwitchSession}
                            onRename={handleRenameSession}
                            onDelete={handleDeleteSession}
                            onTogglePinned={handleToggleSessionPinned}
                            onOpenFolder={(id) => bridge.revealSessionFolder && bridge.revealSessionFolder(id)}
                            onArchive={handleArchiveSession}
                            dragging={!!dragAvatar && dragAvatar.key === 'session:' + chat.id}
                            onPickUp={(geom) => beginTearOff('session', chat.id, chat.title, geom)}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                  {scheduledRunHistory.length > 0 && (
                    <div>
                      <button
                        type="button"
                        onClick={() => setHistoryOpen(prev => ({ ...prev, scheduledRuns: !prev.scheduledRuns }))}
                        className={`w-full h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                      >
                        <span className="truncate">定时任务记录 ({scheduledRunHistory.length})</span>
                        <ChevronDown size={16} className={`shrink-0 transition-transform ${historyOpen.scheduledRuns ? '' : '-rotate-90'}`} />
                      </button>
                      {historyOpen.scheduledRuns && (
                        <div className="mt-1 space-y-px">
                          {scheduledRunHistory.map((chat) => (
                            <RecentItem
                              key={`${chat.scheduledRun.automationId || ''}:${chat.scheduledRun.id || chat.id}`}
                              chat={chat}
                              theme={activeTheme}
                              t={t}
                              active={!!(bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === chat.id)}
                              personaTarget={false}
                              onSelect={() => handleOpenScheduledRunShortcut(chat.scheduledRun)}
                              onRename={handleRenameSession}
                              onDelete={handleDeleteSession}
                              onTogglePinned={handleToggleSessionPinned}
                              onOpenFolder={(id) => bridge.revealSessionFolder && bridge.revealSessionFolder(id)}
                              onArchive={handleArchiveSession}
                              dragging={!!dragAvatar && dragAvatar.key === 'session:' + chat.id}
                              onPickUp={(geom) => beginTearOff('session', chat.id, chat.title, geom)}
                            />
                          ))}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* Footer Profile */}
            <div className={`p-3 mt-auto flex ${isSidebarOpen ? 'flex-row items-center justify-between' : 'flex-col items-center gap-3 pb-6'}`}>
              {!isSidebarOpen && (
                <>
                  <button
                    onClick={handleOpenRemoteControl}
                    title="手机扫码连接"
                    className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                  >
                    <Smartphone size={18} />
                    {bs && bs.remoteControl && bs.remoteControl.active && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                  </button>
                  <button
                    onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                    title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? '隐藏公仔' : '召唤公仔'}
                    className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                  >
                    <PetPawIcon />
                  </button>
                  <button
                    onClick={() => navigateFromScheduledRun('settings')}
                    title={t.settings}
                    className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                  >
                    <Settings size={18} />
                    {hasUpdate && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
                  </button>
                </>
              )}
              <button
                onClick={() => window.__TAURI__.core.invoke('open_external_url', { url: 'https://www.h3c.com/cn/pub/minisite/202606/MegaCube/megacube/index.html' })}
                title={t.megacubeSite}
                className={`flex items-center rounded-xl transition-colors ${isSidebarOpen ? 'flex-1 min-w-0 px-2 py-1.5 gap-3' : 'justify-center w-10 h-10'} ${activeTheme === 'dark' ? 'hover:bg-[#333537] active:bg-[#3A3C3E]' : 'hover:bg-[#E1E5EA] active:bg-[#D8DCE1]'}`}
              >
                <img src="assets/megacube-icon.png" alt="MegaCube" className="w-8 h-8 shrink-0 rounded-lg object-contain" />
                {isSidebarOpen && (
                  <span className="text-[14px] font-medium leading-none whitespace-nowrap text-left">MegaCube</span>
                )}
              </button>
              {isSidebarOpen && (
                <div className="flex items-center gap-1">
                  <button
                    onClick={handleOpenRemoteControl}
                    title="手机扫码连接"
                    className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                  >
                    <Smartphone size={18} />
                    {bs && bs.remoteControl && bs.remoteControl.active && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                  </button>
                  <button
                    onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                    title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? '隐藏公仔' : '召唤公仔'}
                    className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#C4C7C5]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                  >
                    <PetPawIcon />
                  </button>
                  <button
                    onClick={() => navigateFromScheduledRun('settings')}
                    title={t.settings}
                    className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                  >
                    <Settings size={18} />
                    {hasUpdate && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
                  </button>
                </div>
              )}
            </div>
          </div>

          {/* ================= Main Content ================= */}
          <div className={`flex-1 flex flex-col relative min-w-0 overflow-hidden rounded-tl-[24px] ${activeTheme === 'dark' ? 'bg-[#131314]' : 'bg-white'}`}>

            {/* Gemini Style Background Glow */}
            {(currentView === 'chat' || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && (
              activeTheme === 'light' ? (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[1200px] h-[800px] bg-[radial-gradient(ellipse_at_center,_rgba(232,240,254,0.8)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              ) : (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-[40%] w-[1400px] h-[900px] bg-[radial-gradient(ellipse_at_center,_rgba(168,199,250,0.25)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              )
            )}

            {currentView === 'monitor' && <MonitorView theme={activeTheme} t={t} bs={bs} />}
            {currentView === 'settings' && (
              <SettingsErrorBoundary theme={activeTheme}>
                <SettingsView
                  activeTheme={activeTheme} setActiveTheme={handleSetTheme}
                  language={language} setLanguage={handleSetLanguage}
                  superPerm={superPerm} setSuperPerm={handleToggleSuperPerm}
                  taskCompletedNotif={taskCompletedNotif} setTaskCompletedNotif={handleSetTaskCompletedNotif}
                  searchProvider={searchProvider} setSearchProvider={handleSetSearchProvider}
                  searchApiKey={searchApiKey} setSearchApiKey={handleSetSearchApiKey}
                  searchCredential={searchCredentialForProvider(searchProvider)}
                  searchKeyAction={searchProviderKeyAction(searchProvider)}
                  searchHasSavedKey={searchHasSavedKey(searchProvider)}
                  onKeepSearchApiKey={handleKeepSearchApiKey}
                  onReplaceSearchApiKey={handleReplaceSearchApiKey}
                  onDeleteSearchApiKey={handleDeleteSearchApiKey}
                  savedModels={(bs && bs.savedModels) || []}
                  activeModelId={bs && bs.activeModelId}
                  onSaveModel={(m) => bridge.available && bridge.saveModel(m)}
                  onDeleteModel={(m) => { if (bridge.available && window.confirm(t.deleteModelConfirm(m.name))) bridge.deleteModel(m.id); }}
                  onSetActiveModel={(id) => bridge.available && bridge.setActiveModel(id)}
                  onConfirmSearchConfig={handleConfirmSearchConfig}
                  onMemoryEnabledChange={handleSetMemoryEnabled}
                  onPetEnabledChange={handleSetPetEnabled}
                  searchNeedsRestart={searchNeedsRestart}
                  languageNeedsRestart={languageNeedsRestart}
                  bs={bs}
                  t={t}
                  onRestoreArchived={handleRestoreArchivedSession}
                  onDeleteArchived={handleDeleteSession}
                  updateFocusTick={settingsUpdateFocusTick}
                />
              </SettingsErrorBoundary>
            )}
            {currentView === 'workflow' && <WorkflowView theme={activeTheme} t={t} bs={bs} />}
            {currentView === 'toolStore' && <ToolStoreView theme={activeTheme} onNewChat={handleNewChat} />}
            {currentView === 'cardpool' && <CardPoolView theme={activeTheme} t={t} bs={bs} onEquipped={() => setCurrentView('chat')} onAICreate={startAICard} initialMyOnly={poolMyOnly} />}
            {currentView === 'chat' && <ChatView theme={activeTheme} t={t} bs={bs} prefill={chatPrefill} focusComposerTick={petFocusComposerTick} onPrefillConsumed={() => setChatPrefill('')} onOpenEditor={(initial) => setPersonaEditor({ initial })} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => navigateFromScheduledRun('settings')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} />}
            {SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled' && (
              bs && bs.scheduledRunContext ? (
                <ChatView theme={activeTheme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={(initial) => setPersonaEditor({ initial })} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => navigateFromScheduledRun('settings')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} />
              ) : (
                <ScheduledTasksView theme={activeTheme} t={t} onOpenChat={() => setCurrentView('chat')} />
              )
            )}
            {/* 草稿态(无 session)也渲染挂件,但强制空态——让欢迎页保留「＋加持卡牌」入口。
                点它跳卡牌池,选卡时 equipPersona 会先物化 session(lazy session)。 */}
            {(currentView === 'chat' || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && bs && (
              <Lanyard persona={bs.activeSessionId ? (bs.activePersona || null) : null} isDark={activeTheme === 'dark'} t={t}
                onRemove={() => bridge.available && bridge.unequipPersona()}
                onOpenPicker={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))} />
            )}
            {currentView === 'search' && <SearchView theme={activeTheme} history={chatHistory} t={t} onSelect={handleSwitchSession} />}
            {currentView === 'knowledge' && <KnowledgeView theme={activeTheme} t={t} />}

            {remoteOpen && (
              <RemoteControlModal theme={activeTheme} bs={bs} onClose={() => setRemoteOpen(false)} />
            )}

            {/* App 级自创卡编辑器: 聊天里「存入卡牌池」草稿走这条 */}
            {personaEditor && (
              <PersonaEditorModal initial={personaEditor.initial} isDark={activeTheme === 'dark'} t={t}
                onClose={() => setPersonaEditor(null)}
                onSaved={(sum) => { const isEdit = personaEditor.initial && personaEditor.initial.id; setPersonaEditor(null); if (!isEdit) setSavedConfirm({ name: sum && sum.name }); }}
                onDeleted={() => setPersonaEditor(null)} />
            )}

            {/* 存入成功 → iOS 确认窗:去查看我的卡牌 / 暂不 */}
            {savedConfirm && (
              <div className="fixed inset-0 z-[80] flex items-center justify-center p-4" style={{ background:'rgba(0,0,0,.4)' }} onClick={() => setSavedConfirm(null)}>
                <div onClick={(e) => e.stopPropagation()} className="w-[270px] rounded-[14px] overflow-hidden text-center"
                  style={{ background: activeTheme === 'dark' ? 'rgba(44,44,46,.95)' : 'rgba(250,250,250,.95)', backdropFilter:'blur(20px)', WebkitBackdropFilter:'blur(20px)', fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  <div className="px-4 pt-5 pb-4">
                    <div className="text-[17px] font-semibold" style={{ color: activeTheme === 'dark' ? '#fff' : '#000' }}>{t.cpSavedTitle}</div>
                    <div className="text-[13px] mt-1.5" style={{ color: activeTheme === 'dark' ? 'rgba(235,235,245,.6)' : 'rgba(60,60,67,.6)' }}>{t.cpSavedDesc(savedConfirm.name || '')}</div>
                  </div>
                  <div className="flex" style={{ borderTop: '0.5px solid ' + (activeTheme === 'dark' ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)') }}>
                    <button onClick={() => setSavedConfirm(null)} className="flex-1 h-11 text-[17px]" style={{ color: activeTheme === 'dark' ? '#0A84FF' : '#007AFF' }}>{t.cpSavedLater}</button>
                    <div style={{ width:'0.5px', background: activeTheme === 'dark' ? 'rgba(84,84,88,.65)' : 'rgba(60,60,67,.29)' }} />
                    <button onClick={() => { setPoolMyOnly(true); setSavedConfirm(null); setCurrentView('cardpool'); }} className="flex-1 h-11 text-[17px] font-semibold" style={{ color: activeTheme === 'dark' ? '#0A84FF' : '#007AFF' }}>{t.cpSavedView}</button>
                  </div>
                </div>
              </div>
            )}

            {/* MegaCube(GB10) 本地大模型一键引导 —— 全局首屏弹窗;引导中禁止背景关窗 */}
            {bs && bs.vllmSetup && bs.vllmSetup.eligible && !bs.vllmSetupDismissed && (
              <div className="fixed inset-0 z-[56] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}
                   onClick={() => { if (!bs.vllmBootstrapping) bridge.dismissVllmSetup(); }}>
                <div className="w-full max-w-[440px] rounded-2xl p-6 ts-modal-in" onClick={(e) => e.stopPropagation()}
                     style={{ background: activeTheme === 'dark' ? '#1E1F20' : '#FFFFFF', color: activeTheme === 'dark' ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
                  <div className="flex items-center gap-2 mb-3">
                    <img src="brand-blue.png" width={22} height={22} alt="" className="select-none" />
                    <div className="text-[17px] font-semibold">{vllmDeclineConfirm && !bs.vllmBootstrapping && !bs.vllmBootstrapDone && !bs.vllmBootstrapError ? t.vllmDeclineTitle : t.vllmSetupTitle}</div>
                  </div>
                  {bs.vllmBootstrapping ? (
                    <VllmSetupProgress phase={bs.vllmSetupPhase} attempt={bs.vllmSetupAttempt} isDark={activeTheme === 'dark'} t={t} />
                  ) : bs.vllmBootstrapDone ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4">{t.vllmSetupDone}</div>
                      <div className="flex justify-end">
                        <button onClick={() => bridge.available && bridge.restartApp()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.restartNow}</button>
                      </div>
                    </div>
                  ) : bs.vllmBootstrapError ? (
                    <div>
                      <div className="text-[14px] font-medium mb-1" style={{ color: '#E5484D' }}>{t.vllmSetupFailed}</div>
                      <div className="text-[13px] leading-relaxed mb-4 break-words" style={{ opacity: .75 }}>{bs.vllmBootstrapError}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => bridge.dismissVllmSetup()}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                        <button onClick={() => bridge.bootstrapLocalVllm()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupRetry}</button>
                      </div>
                    </div>
                  ) : vllmDeclineConfirm ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.vllmDeclineDesc}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => setVllmDeclineConfirm(false)}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmDeclineReconsider}</button>
                        <button onClick={() => { setVllmDeclineConfirm(false); bridge.declineVllmSetup(); }}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#E5484D' }}>{t.vllmDeclineConfirm}</button>
                      </div>
                    </div>
                  ) : (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.vllmSetupDesc}</div>
                      <div className="flex items-center justify-between gap-2">
                        <button onClick={() => setVllmDeclineConfirm(true)}
                          className="h-9 px-3 rounded-lg text-[13px] hover:underline" style={{ color: activeTheme === 'dark' ? '#8E8E8E' : '#757575' }}>{t.vllmSetupNever}</button>
                        <div className="flex gap-2">
                          <button onClick={() => bridge.dismissVllmSetup()}
                            className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                          <button onClick={() => bridge.bootstrapLocalVllm()}
                            className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupEnable}</button>
                        </div>
                      </div>
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* Pinvou 检阅弹窗(品/悟) —— 居中弹窗 + 毛玻璃背景(虚化身后 app);全局,任何视图都能弹;点背景或卡内「跳过」关闭 */}
            {bs && bs.pinvouModal && (
              <div className="fixed inset-0 z-[55] flex items-center justify-center p-6"
                   style={{ background: activeTheme === 'dark' ? 'rgba(0,0,0,.45)' : 'rgba(255,255,255,.35)', backdropFilter: 'blur(20px) saturate(140%)', WebkitBackdropFilter: 'blur(20px) saturate(140%)' }}
                   onClick={() => { if (!bs.pinvouModal.loading) bridge.dismissPinvouReview(); }}>
                {/* loading 期间禁止背景点击关窗:召唤(直连 vLLM,5-30s)仍在后台跑、守卫仍 held,
                    点背景误关会表现为"闪一下没反应、要等一会才能再点"。锁住后 spinner 全程可见,
                    出结果/错误后才可点背景关。 */}
                <div className="relative w-full max-w-[720px] overflow-hidden bg-white dark:bg-[#1C1C1E] rounded-[20px] shadow-[0_20px_60px_rgba(0,0,0,0.28)] ts-modal-in"
                     onClick={(e) => e.stopPropagation()}
                     style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  {/* 关闭按钮：所有状态(含 loading)常驻;loading 时点它=取消等待并关窗,in-flight 结果由守卫丢弃 */}
                  <button onClick={() => bridge.available && bridge.dismissPinvouReview()} aria-label={t.pvSkip}
                    className="absolute top-3.5 right-3.5 z-10 w-7 h-7 flex items-center justify-center rounded-full bg-black/[0.06] dark:bg-white/10 text-[#8E8E93] hover:bg-black/10 dark:hover:bg-white/15 active:scale-90 transition-colors">
                    <X size={16} />
                  </button>
                  <div className="max-h-[90vh] overflow-y-auto custom-scrollbar px-5 pt-5 pb-6">
                    <PinvouSummonCard item={bs.pinvouModal} theme={activeTheme} t={t} />
                  </div>
                </div>
              </div>
            )}

          </div>
          </div>

          <UpdateNoticeButton
            theme={activeTheme}
            bs={bs}
            t={t}
            onShowChangelog={() => {
              setCurrentView('settings');
              setSettingsUpdateFocusTick(v => v + 1);
            }}
          />
        </div>
      );
    };

    const UpdateNoticeButton = ({ theme, bs, t, onShowChangelog }) => {
      const isDark = theme === 'dark';
      const logic = window.UpdateNoticeLogic;
      const isPreview = !bridge.available && logic.previewEnabled(window.location);
      const updateInfo = logic.updateInfoFor(bs, { preview: isPreview });
      const [closed, setClosed] = useState(false);

      useEffect(() => { setClosed(false); }, [logic.versionKey(updateInfo)]);

      if (!updateInfo || closed) return null;

      const vm = logic.viewModel(bs, updateInfo, bs && bs.appVersion, {
        downloadInstall: t.downloadInstall,
        downloadInstallRestart: t.downloadInstallRestart,
        downloading: t.downloading,
        installing: t.installing,
        restartNow: t.restartNow,
        updateInstallerStarted: t.updateInstallerStarted,
      });

      const handleUpgrade = () => {
        if (isPreview) {
          return;
        }
        if (!bridge.available) return;
        if (vm.action === 'restart') bridge.restartApp();
        else if (vm.action === 'download') bridge.downloadAndInstallUpdate();
      };

      const handleShowChangelog = () => {
        if (onShowChangelog) onShowChangelog();
      };

      return (
        <div data-update-notice-card="true" className={`fixed left-4 bottom-4 z-[70] w-[260px] p-3.5 backdrop-blur-xl rounded-2xl border shadow-xl shrink-0 transition-all duration-300 ${
          isDark
            ? 'bg-[#1c1c21]/85 border-white/[0.06] text-gray-200 shadow-2xl'
            : 'bg-white/85 border-gray-200/60 text-gray-800'
        }`}>
          <div className="flex items-center gap-3 mb-3">
            <div className={`w-10 h-10 rounded-[10px] border shadow-inner flex items-center justify-center shrink-0 overflow-hidden relative transition-colors duration-300 ${
              isDark
                ? 'bg-gradient-to-br from-[#2c2c35] to-[#1a1a20] border-white/[0.08]'
                : 'bg-gradient-to-br from-gray-100 to-gray-50 border-gray-200/80'
            }`}>
              <img src="brand-blue.png" alt="" className="w-6 h-6 object-contain" />
            </div>

            <div className="flex flex-col justify-center flex-1 min-w-0">
              <div className="flex items-center justify-between">
                <span className={`text-[13px] font-semibold tracking-wide transition-colors duration-300 ${
                  isDark ? 'text-gray-100' : 'text-gray-900'
                }`}>{t.newVersionFound}</span>
                <button
                  type="button"
                  onClick={() => setClosed(true)}
                  className={`p-1 -mr-1 rounded-full transition-colors focus:outline-none ${
                    isDark
                      ? 'text-gray-500 hover:text-gray-300 hover:bg-white/[0.08]'
                      : 'text-gray-400 hover:text-gray-600 hover:bg-gray-100'
                  }`}
                  title={t.winClose}
                >
                  <X size={14} />
                </button>
              </div>
              <span className={`text-[11px] font-mono px-1.5 py-0.5 rounded w-fit mt-0.5 transition-colors duration-300 ${
                isDark ? 'text-gray-400 bg-black/20' : 'text-gray-500 bg-gray-100'
              }`}>PINVOU v{vm.version}</span>
            </div>
          </div>

          {vm.error && (
            <div className="mb-3 text-[11px] leading-relaxed text-[#EA4335] break-words">{vm.error}</div>
          )}

          <div className="flex gap-2 text-xs font-medium">
            <button
              type="button"
              data-update-notes-button="true"
              onClick={handleShowChangelog}
              className={`flex-1 py-2 rounded-xl transition-all active:scale-[0.96] ${
                isDark
                  ? 'bg-white/[0.06] hover:bg-white/[0.1] text-gray-200'
                  : 'bg-gray-100 hover:bg-gray-200 text-gray-700'
              }`}
            >
              {t.updateNotes}
            </button>
            <button
              type="button"
              onClick={handleUpgrade}
              disabled={vm.disabled}
              className="flex-1 py-2 bg-blue-600 hover:bg-blue-500 text-white rounded-xl transition-all active:scale-[0.96] flex justify-center items-center gap-1.5 shadow-sm shadow-blue-900/20 disabled:opacity-80 disabled:cursor-not-allowed"
            >
              {vm.downloading ? <span className="w-3.5 h-3.5 rounded-full border-2 border-white/70 border-t-transparent animate-spin" /> : <RefreshCw size={14} />}
              <span>{vm.label}</span>
            </button>
          </div>
        </div>
      );
    };

    /* ==========================================
       Helpers
       ========================================== */
    const SearchView = ({ theme, history, t, onSelect }) => {
      const isDark = theme === 'dark';
      const [query, setQuery] = useState('');
      const inputRef = useRef(null);
      const filtered = query
        ? history.filter(h => h.title.toLowerCase().includes(query.toLowerCase()))
        : history;

      return (
        <div className="flex-1 flex flex-col w-full h-full relative z-10 animate-in fade-in duration-300">
          <div className="flex-1 overflow-y-auto custom-scrollbar px-6 pt-16 pb-20">
            <div className="max-w-[768px] mx-auto flex flex-col">

              {/* Centered Search Bar */}
              <div className={`flex items-center gap-3 px-6 py-4 rounded-full mb-10 transition-colors ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-[#F0F4F9] text-[#1F1F1F]'}`}>
                <Search size={22} className={isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'} />
                <input
                  ref={inputRef}
                  type="text"
                  placeholder={t.searchPlaceholder}
                  value={query}
                  onChange={e => setQuery(e.target.value)}
                  className={`flex-1 bg-transparent border-none outline-none text-[16px] placeholder:text-[16px] ${isDark ? 'placeholder:text-[#C4C7C5]' : 'placeholder:text-[#444746]'}`}
                />
                {query ? (
                  <button
                    type="button"
                    aria-label={t.clearSearch}
                    title={t.clearSearch}
                    onClick={() => { setQuery(''); inputRef.current && inputRef.current.focus(); }}
                    className={`w-8 h-8 shrink-0 rounded-full flex items-center justify-center transition-colors ${isDark ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#DDE3EA]'}`}
                  >
                    <X size={16} />
                  </button>
                ) : null}
              </div>

              {/* List Section */}
              <div className={`text-[14px] font-medium mb-3 px-4 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
                {t.recent}
              </div>

              <div className="flex flex-col">
                {filtered.map(chat => (
                  <div
                    key={chat.id}
                    onClick={() => onSelect && onSelect(chat.id)}
                    className={`flex justify-between items-center px-4 py-[14px] cursor-pointer rounded-[16px] transition-colors ${isDark ? 'hover:bg-[#1E1F20]' : 'hover:bg-[#F0F4F9]'}`}
                  >
                    <span className={`text-[15px] truncate pr-4 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
                      {chat.title}
                    </span>
                    <span className={`text-[13px] shrink-0 ${isDark ? 'text-[#C4C7C5]' : 'text-[#444746]'}`}>
                      {chat.date}
                    </span>
                  </div>
                ))}
              </div>

            </div>
          </div>
        </div>
      );
    };


    // ==========================================
    // Knowledge View — 本地知识 L0：全系统秒搜 + 去重
    // 后端 kb_* 命令（src-tauri/src/knowledge）。纯 L0 元数据，不过模型。
    // ==========================================
    // KnowledgeView 用 currentView 条件渲染:切 tab 会卸载、丢 state,回来重新加载 → 每次闪
    // 一下"还没建立索引"空状态再加载。模块级缓存上次拉到的 L0/L1 数据:remount 时初始 state
    // 直接用缓存(切回秒显),后台 refresh 更新。loaded=false 时显示加载中而非误判空状态。
    const useDetachedBase = () => {
      const bs = useBridge();
      const [language, setLanguage] = useState('zh');
      const [activeTheme, setActiveTheme] = useState('dark');
      const initRef = useRef(false);
      // 一次性从落盘 settings 恢复语言/主题(镜像 App 的恢复逻辑,但自包含,不动 App)。
      useEffect(() => {
        if (initRef.current || !bs || !bs.settings) return;
        const lang = TAG_TO_LANG[bs.settings.language];
        if (lang) setLanguage(lang);
        setActiveTheme(bs.settings.theme === 'liquid-light' ? 'light' : 'dark');
        initRef.current = true;
      }, [bs]);
      useEffect(() => { document.documentElement.classList.toggle('dark', activeTheme === 'dark'); }, [activeTheme]);
      return { bs, activeTheme, t: dict[language] };
    };

    // 撕离窗口的面板错误边界:某个 View 抛错时不白屏,退化为提示,保留标题栏可关窗。
    class DetachedErrorBoundary extends React.Component {
      constructor(p) { super(p); this.state = { err: null }; }
      static getDerivedStateFromError(err) { return { err }; }
      render() {
        if (this.state.err) {
          return <div className="p-6 text-sm opacity-70">面板加载失败:{String(this.state.err && this.state.err.message || this.state.err)}</div>;
        }
        return this.props.children;
      }
    }

    // kind → 撕离窗口该挂载的面板。复用主窗口同款 View 组件(见主渲染区 currentView 分支);
    // 跨视图导航类 handler 在撕离窗口里是 no-op(单视图,无处可跳)。
    const DETACHED_VIEWS = {
      session:   ({ theme, t, bs }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={()=>{}} onOpenEditor={()=>{}} justInstalledTool={null} setJustInstalledTool={()=>{}} onGotoSettings={()=>{}} onGotoTools={()=>{}} />,
      workflow:  ({ theme, t, bs }) => <WorkflowView theme={theme} t={t} bs={bs} />,
      monitor:   ({ theme, t, bs }) => <MonitorView theme={theme} t={t} bs={bs} />,
      cardpool:  ({ theme, t, bs }) => <CardPoolView theme={theme} t={t} bs={bs} onEquipped={()=>{}} onAICreate={()=>{}} initialMyOnly={false} />,
      toolstore: ({ theme, t, bs }) => <ToolStoreView theme={theme} onNewChat={()=>{}} />,
      knowledge: ({ theme, t, bs }) => <KnowledgeView theme={theme} t={t} />,
    };

    const DetachedShell = ({ kind, id }) => {
      const { bs, activeTheme, t } = useDetachedBase();
      // session 窗口:boot 时把该 session 切为 active,让 ChatView 显示它。
      useEffect(() => {
        if (kind === 'session' && id && bridge.available && bridge.switchToSession) bridge.switchToSession(id);
      }, [kind, id]);
      useEffect(() => {
        if (kind !== 'monitor' || !bridge.available) return;
        bridge.startMonitorPolling();
        return () => { if (bridge.stopMonitorPolling) bridge.stopMonitorPolling(); };
      }, [kind]);
      // 关闭时通知主窗口回坞(去角标)。
      useEffect(() => {
        const key = kind + ':' + (id || '');
        const onUnload = () => { try { window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.emit('detach:closed', key); } catch (_) {} };
        window.addEventListener('beforeunload', onUnload);
        return () => window.removeEventListener('beforeunload', onUnload);
      }, [kind, id]);
      const View = DETACHED_VIEWS[kind] || DETACHED_VIEWS.monitor;
      const isDark = activeTheme === 'dark';
      return (
        <div className={`h-screen w-screen flex flex-col ${isDark ? 'bg-[#1B1C1D] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>
          <div data-tauri-drag-region className="h-9 shrink-0 flex items-center px-3 text-[13px] font-medium select-none"
               style={{ borderBottom: '1px solid rgba(128,128,128,.2)' }}>
            <span data-tauri-drag-region className="pointer-events-none">{(t && t.tearoffTitle) || '撕离窗口'} · {kind}</span>
          </div>
          <div className="flex-1 min-h-0 overflow-auto">
            {bs ? <DetachedErrorBoundary><View theme={activeTheme} t={t} bs={bs} /></DetachedErrorBoundary> : <div className="p-6 text-sm opacity-60">…</div>}
          </div>
        </div>
      );
    };

    // 长按撕离:按住 ~350ms 不动 → onPickUp(info)(DOM avatar 浮起跟手 + begin_detach_drag 原生判落点);
    // 长按达成前移动 >10px = 视为滚动/取消;长按达成后吞掉随之而来的 click(避免又切视图);
    // 按在内部按钮/输入框上不起手(让它们自理)。按下即禁选,防止长按选中下方文字。
    const root = createRoot(document.getElementById('root'));
    const __q = new URLSearchParams(window.location.search);
    if (__q.get('detached') === '1') {
      window.__PINVOU_DETACHED__ = true;
      root.render(<DetachedShell kind={__q.get('kind') || 'monitor'} id={__q.get('id') || ''} />);
    } else {
      root.render(<App />);
    }
