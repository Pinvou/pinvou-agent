import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { createRoot } from 'react-dom/client';
import '../styles/base.css';
import { I, Plus, Edit2, Trash2, ClipboardList, BarChart2, Settings, Monitor, Smartphone, Brain, BrainCircuit, Clock, Sun, Moon, Zap, Package, RotateCcw, Search, Upload, Lightbulb, Paperclip, Mic, Send, Store, Terminal, ChevronDown, IconGrid, IconList, Copy, CheckCircle2, AlertTriangle, Menu, MoreHorizontal, Check, Filter, Database, Download, FolderPlus, Award, Feather, AppWindow, Radio, Palette, Briefcase, StopCircle, XCircle, Wrench, Layers, MessageSquare, X, ArrowLeft, FolderOpen, ExternalLink, BookOpen, Code, FileText, Hexagon, Layout, Presentation, Mail, MessageCircle, Navigation, Video, Puzzle, LineChart, Building2, Cpu, Server, Globe, ChevronLeft, XIcon, CloudSun, TrendingUp, TrendingDown, GridIcon, TableIcon, PresentationIcon, ImageIcon, Archive, PetPawIcon } from '../components/icons.jsx';
import { ArchiveConfirmDialog, ArchiveToast, NavItem, RecentItem } from '../components/layout/NavigationComponents.jsx';
import { AcpAgentLogo } from '../features/codex/AcpAgentLogo.jsx';
import { PinvouLogo } from '../components/PinvouLogo.jsx';
import { MobileMoreSheet, MobileTabBar, MobileTopBar } from '../components/layout/MobileShell.jsx';
import { VllmSetupProgress } from '../components/VllmSetupProgress.jsx';
import { bridge, useBridgeState, activeModelIsLocal, shouldShowApiKeyGate } from '../hooks/useBridge.js';
import { useCompactViewport, useVisualViewportHeight } from '../hooks/useViewport.js';
import { dict, LANG_TO_TAG, initialSystemLanguage, SEARCH_KEY_PROVIDERS, TAG_TO_LANG } from '../shared/i18n.js';
import { formatSessionDate, localDateKey, formatDateGroupLabel } from '../shared/date-utils.js';
import { runSessionBatch } from '../shared/session-management.js';
import { can, isWeb } from '../shared/platform.js';
import { installGlobalMarkdownRenderer } from '../shared/markdown-renderer.js';
import { KnowledgeView } from '../features/knowledge/KnowledgeView.jsx';
import { MonitorView } from '../features/monitor/MonitorView.jsx';
import { SettingsView, WebAccessModal } from '../features/settings/SettingsView.jsx';
import { SettingsErrorBoundary } from '../features/settings/SettingsErrorBoundary.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { createPinvouModeScopeKey, savePinvouModeState } from '../features/chat/pinvou-mode-state.js';
import { CodexAcpView } from '../features/codex/CodexAcpView.jsx';
import { ScheduledTasksView } from '../features/scheduled/ScheduledTasksView.jsx';
import { WebConnectionStatus } from '../features/web/WebConnectionStatus.jsx';
import { createPetActivationGuard } from '../features/pet/activation-guard.js';
import { SessionAttachmentTitle } from '../features/attachments/SessionAttachmentTitle.jsx';
import {
  sessionTitlePlainText,
  sessionTitlePresentation,
} from '../features/attachments/attachment-message.js';
import {
  invokeTauri,
  isTauriAvailable,
  tauriCommands,
  tauriEvents,
} from '../platform/tauri/client.js';
import { revealStartupWindow } from '../platform/tauri/startup-window.js';

// 定时任务创建与运行链路已恢复，展示入口并允许自动跳转。
const SCHEDULED_TASKS_ENTRY_ENABLED = true;

// 后端默认会话标题哨兵集合(bridge 按当前语言生成三语兜底标题,并据此判断是否自动改名)——
// 显示层把任意一种哨兵标题映射成当前语言的「新对话」文案。
const DEFAULT_CHAT_TITLES = new Set(Object.values(dict).map(d => d && d.newChat).filter(Boolean));
// Static regression anchor: SCHEDULED_TASKS_ENTRY_ENABLED && (<NavItem icon={<Clock size={18} />} label={t.scheduledPlans} unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))} />)
const PREVIEW_SCHEDULED_RUN_SHORTCUTS = [
  { id: 'preview-run-1', automationId: 'preview-daily-brief', taskNameKey: 'previewTaskDailyBrief', sessionId: 'preview-session-1', status: 'completed', scheduledFor: '2026-07-14T08:00:00+08:00', unread: true },
  { id: 'preview-run-4', automationId: 'preview-follow-up', taskNameKey: 'previewTaskFollowUp', sessionId: 'preview-session-4', status: 'running', scheduledFor: '2026-07-14T09:00:00+08:00', unread: false },
  { id: 'preview-run-6', automationId: 'preview-weekly-report', taskNameKey: 'previewTaskSalesWeekly', sessionId: 'preview-session-6', status: 'completed', scheduledFor: '2026-07-10T16:00:00+08:00', unread: false },
];
import { ToolStoreView } from '../features/tools/ToolStoreView.jsx';
import { PinvouSummonCard } from '../features/tools/tool-renderers.jsx';
import { CardPoolView, Lanyard, PersonaEditorModal } from '../features/personas/Personas.jsx';
import { SearchView } from '../features/search/SearchView.jsx';
import { SearchOverlay } from '../features/search/SearchOverlay.jsx';
import { UpdateNoticeButton } from '../features/updater/UpdateNoticeButton.jsx';
import { DetachedShell } from './DetachedShell.jsx';
import { TitleBar } from './DesktopTitleBar.jsx';

installGlobalMarkdownRenderer(window);
window.__PINVOU_STARTUP__.mark('app:main_module_body_enter');

let appFirstRenderMarked = false;

const APP_BRIDGE_STATE_DOMAINS = [
  'platform', 'sessions', 'chat', 'voice', 'knowledge', 'scheduled', 'monitor',
  'settings', 'models', 'vllm', 'interaction', 'personas',
  'memory', 'remoteControl', 'updater', 'dependencies',
];

function emitPetEvent(ev, name, payload) {
  if (!ev) return Promise.resolve(false);
  try {
    if (typeof ev.emitTo === 'function') {
      return Promise.resolve(ev.emitTo('pet', name, payload));
    }
    if (typeof ev.emit === 'function') {
      return Promise.resolve(ev.emit(name, payload));
    }
  } catch (error) {
    return Promise.reject(error);
  }
  return Promise.resolve(false);
}

// 当前平台是否支持本地 vLLM。macOS/Windows 后端已 cfg 掉本地 vLLM 命令(discover_local_vllm /
// detect_local_vllm_setup 等),前端默认预设与探测入口都据此守卫,避免新用户首启落在
// 127.0.0.1:8000 永远连不上、或调用不存在的后端命令报错。与 bridge prefs::ModelPreset::default() 对齐。
function defaultModelPresetForCapabilities(capabilities) {
  return capabilities && capabilities.localVllmSupported ? 'local_vllm' : 'deepseek';
}

function workspaceDisplayName(path) {
  const parts = String(path || '').split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || String(path || '');
}

    const App = () => {
      if (!appFirstRenderMarked) {
        appFirstRenderMarked = true;
        window.__PINVOU_STARTUP__.mark('react:app_render_start');
      }
      const bs = useBridgeState(APP_BRIDGE_STATE_DOMAINS);
      useLayoutEffect(() => {
        window.__PINVOU_STARTUP__.mark('react:first_commit');
        window.__PINVOU_STARTUP__.flush();
        // Linux 的主窗口在配置中隐藏创建。首次 React 提交说明可交互 DOM 已就绪，
        // 此时再映射 XWayland 窗口，避免冷启动阶段把尚未稳定的输入表面暴露给用户。
        void revealStartupWindow().then((revealed) => {
          if (!revealed) return;
          window.__PINVOU_STARTUP__.mark('react:startup_window_revealed');
          window.__PINVOU_STARTUP__.flush();
        });
      }, []);
      useEffect(() => {
        window.__PINVOU_STARTUP__.mark('react:first_effect');
        window.__PINVOU_STARTUP__.flush();
        // 连续两个 rAF：第二个回调发生在首次提交已经交给 WebView 绘制之后。此时再启动
        // 558 MiB embedding 模型的 blocking 后台加载，避免模型 IO/ONNX 初始化阻塞白屏。
        let secondFrame = 0;
        const firstFrame = window.requestAnimationFrame(() => {
          window.__PINVOU_STARTUP__.mark('react:first_animation_frame');
          secondFrame = window.requestAnimationFrame(() => {
            window.__PINVOU_STARTUP__.mark('react:first_frame_presented');
            window.__PINVOU_STARTUP__.flush();
            if (bridge.available && bridge.knowledge.loadKnowledgeEmbedderAfterFirstFrame) {
              bridge.knowledge.loadKnowledgeEmbedderAfterFirstFrame();
            }
          });
        });
        // 让首帧先交给 WebView 绘制，再异步校验飞书/企微实时鉴权状态。
        // 后端并行跑两个 CLI；结果只刷新技能目录，不阻塞主界面。
        const authTimer = window.setTimeout(() => {
          if (bridge.available && bridge.platform.refreshConnectorAuthGates) {
            bridge.platform.refreshConnectorAuthGates().catch(error => {
              console.warn('[startup] connector auth refresh failed', error);
            });
          }
        }, 0);
        return () => {
          window.cancelAnimationFrame(firstFrame);
          if (secondFrame) window.cancelAnimationFrame(secondFrame);
          window.clearTimeout(authTimer);
        };
      }, []);
      const [activeChat, setActiveChat] = useState(null);
      const [currentView, setCurrentView] = useState('chat');
      const [activeTheme, setActiveTheme] = useState('dark');
      const platformCapabilities = (bs && bs.platformCapabilities) || {};
      const showMegacubeSite = !!platformCapabilities.showMegacubeSite;
      const codexAcpSupported = !!platformCapabilities.codexAcpSupported;
      const [codexSessions, setCodexSessions] = useState([]);
      const [codexDraftEpoch, setCodexDraftEpoch] = useState(0);
      const [activeCodexId, setActiveCodexId] = useState(() => {
        try {
          return localStorage.getItem('pinvou_codex_active_session') || null;
        } catch {
          return null;
        }
      });
      const [codexBusyBySession, setCodexBusyBySession] = useState({});
      // 代码会话等待用户输入（request_user_input 挂起）的会话集合：侧边栏用
      // 「等待你的选择」橙色点提示，与 running 灰点区分——后台会话提问不再无感知。
      const [codexWaitingInputBySession, setCodexWaitingInputBySession] = useState({});
      // 全局事件监听器按 id 判断是否为代码会话（监听器注册一次，不能闭包旧列表）。
      const codexSessionIdsRef = useRef(new Set());
      // 进入设置前的页面（openSettingsSection 记录），关闭设置时原路返回。
      const settingsReturnViewRef = useRef(null);
      useEffect(() => {
        codexSessionIdsRef.current = new Set(codexSessions.map(session => session && session.id));
      }, [codexSessions]);
      const refreshCodexSessions = useCallback(async () => {
        if (!codexAcpSupported || !isTauriAvailable()) {
          setCodexSessions([]);
          return [];
        }
        const sessions = await invokeTauri('list_codex_acp_sessions');
        const next = Array.isArray(sessions) ? sessions : [];
        setCodexSessions(next);
        return next;
      }, [codexAcpSupported]);
      const updateActiveCodexSession = useCallback((id) => {
        const next = id || null;
        setActiveCodexId(next);
        try {
          if (next) localStorage.setItem('pinvou_codex_active_session', next);
          else localStorage.removeItem('pinvou_codex_active_session');
        } catch {
          // WebView 禁用 storage 时仍允许当前窗口内切换。
        }
      }, []);
      useEffect(() => {
        if (!codexAcpSupported || !isTauriAvailable()) {
          setCodexSessions([]);
          return undefined;
        }
        let disposed = false;
        const unlisteners = [];
        refreshCodexSessions().catch(error => {
          if (!disposed) console.warn('[codex] list sessions failed', error);
        });
        tauriEvents.listen('acp:event', (message) => {
          if (disposed) return;
          const incoming = message && message.payload;
          const sessionId = incoming && incoming.sessionId;
          const type = incoming && incoming.event && incoming.event.type;
          if (!sessionId || !type) return;
          if (type === 'turn_started') {
            setCodexBusyBySession(current => ({ ...current, [sessionId]: true }));
            refreshCodexSessions().catch(() => {});
          } else if (type === 'turn_completed') {
            setCodexBusyBySession(current => ({ ...current, [sessionId]: false }));
            refreshCodexSessions().catch(() => {});
          }
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        tauriEvents.listen('session:deleted', () => {
          if (!disposed) refreshCodexSessions().catch(() => {});
        }).then(unlisten => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        }).catch(() => {});
        // 原生（品悟）代码会话的 turn 走 chat:* 事件：busy 徽标与 ACP 会话同机制，
        // 只跟踪代码会话列表内的 session，普通聊天会话不影响。
        ['chat:turn_started', 'chat:done'].forEach(eventName => {
          tauriEvents.listen(eventName, (message) => {
            if (disposed) return;
            const sessionId = message && message.payload && message.payload.session_id;
            if (!sessionId || !codexSessionIdsRef.current.has(sessionId)) return;
            setCodexBusyBySession(current => ({ ...current, [sessionId]: eventName === 'chat:turn_started' }));
            if (eventName === 'chat:done') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: false }));
            }
            refreshCodexSessions().catch(() => {});
          }).then(unlisten => {
            if (disposed) unlisten();
            else unlisteners.push(unlisten);
          }).catch(() => {});
        });
        // 后台会话提问（request_user_input 挂起）时点亮「等待你的选择」提示，
        // 收口（提交/取消/超时→tool_end）后熄灭；turn 结束由上面 chat:done 兜底。
        ['chat:user_input_required', 'chat:tool_end'].forEach(eventName => {
          tauriEvents.listen(eventName, (message) => {
            if (disposed) return;
            const p = message && message.payload || {};
            const sessionId = p.session_id;
            if (!sessionId || !codexSessionIdsRef.current.has(sessionId)) return;
            if (eventName === 'chat:user_input_required') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: true }));
              setCodexBusyBySession(current => ({ ...current, [sessionId]: true }));
            } else if (p.name === 'request_user_input') {
              setCodexWaitingInputBySession(current => ({ ...current, [sessionId]: false }));
              // 只有提问收口才刷新会话列表；普通工具 tool_end 不动列表，避免
              // 工具密集 turn 下每个 chat:tool_end 都触发一次 IPC + 重渲染。
              refreshCodexSessions().catch(() => {});
            }
          }).then(unlisten => {
            if (disposed) unlisten();
            else unlisteners.push(unlisten);
          }).catch(() => {});
        });
        return () => {
          disposed = true;
          unlisteners.forEach(unlisten => unlisten());
        };
      }, [codexAcpSupported, refreshCodexSessions]);
      // 供全局事件监听器读取最新视图状态（监听器只注册一次，不能闭包旧值）。
      const activeChatRef = useRef(activeChat);
      activeChatRef.current = activeChat;
      const currentViewRef = useRef(currentView);
      currentViewRef.current = currentView;
      useEffect(() => {
        if (!isTauriAvailable()) return undefined;
        const guard = createPetActivationGuard();
        let disposed = false;
        let unlisten = null;
        tauriEvents.listen('pet:activation_guard', guard.arm).then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        }).catch(() => {});
        // 只拦截由上面的桌宠专用事件武装后的一个 click。普通 window.focus、
        // Alt-Tab、任务栏回焦和其它平台不会触发保护，也就不会丢掉正常首击。
        window.addEventListener('click', guard.handleClick, true);
        return () => {
          disposed = true;
          if (unlisten) unlisten();
          window.removeEventListener('click', guard.handleClick, true);
        };
      }, []);
      useEffect(() => {
        const liveBridge = window.TauriBridge || bridge;
        if (!liveBridge?.monitor || typeof liveBridge.monitor.startMonitorPolling !== 'function') return;
        if (currentView === 'monitor') {
          liveBridge.monitor.startMonitorPolling();
          return () => { if (typeof liveBridge.monitor.stopMonitorPolling === 'function') liveBridge.monitor.stopMonitorPolling(); };
        }
      }, [currentView]);
      // 工具商店/卡片用 Tailwind dark: 变体(darkMode:'class'),全局挂 <html>.dark 让其随 app 主题切换
      useEffect(() => { document.documentElement.classList.toggle('dark', activeTheme === 'dark'); }, [activeTheme]);
      // MegaCube(GB10) 首屏检测:仅启动一次,检测「预装但未启用」本地大模型环境(后端短路保证普通机零开销)。
      useEffect(() => {
        if (bridge.available && platformCapabilities.localVllmSupported) {
          bridge.vllm.detectLocalVllmSetup();
        }
      }, [platformCapabilities.localVllmSupported]);
      const [vllmDeclineConfirm, setVllmDeclineConfirm] = useState(false); // 引导框「不再提醒」二次确认子态
      const [language, setLanguage] = useState(() => {
        const systemLanguage = initialSystemLanguage();
        if (!isWeb) return systemLanguage;
        try {
          const value = window.localStorage.getItem('pinvou.web.language');
          return value && dict[value] ? value : systemLanguage;
        } catch (_) { return systemLanguage; }
      });
      const [superPerm, setSuperPerm] = useState(false);
      const defaultTaskCompletedNotif = platformCapabilities.taskCompletionNotificationsDefault !== false;
      const [taskCompletedNotif, setTaskCompletedNotif] = useState(defaultTaskCompletedNotif);
      // search 后端配置:provider 默认 bing(对齐 bridge prefs::SearchProvider::default());
      // bs.settings 加载后 useEffect 同步进来。
      const [searchProvider, setSearchProvider] = useState('bing');
      const [enabledSearchProviders, setEnabledSearchProviders] = useState(['bing']);
      const [searchApiKey, setSearchApiKey] = useState('');
      const [searchKeyDrafts, setSearchKeyDrafts] = useState({});
      const [searchKeyActions, setSearchKeyActions] = useState({});
      // 模型配置（动态适配）——草稿模式，确认后才保存
      // 默认预设平台感知:macOS/Windows 无本地 vLLM(后端命令已 cfg 掉),默认 DeepSeek;
      // Linux 保持 local_vllm(麒麟环境默认有本地大模型)。与 bridge prefs::ModelPreset::default() 对齐。
      const [modelPreset, setModelPreset] = useState(() => defaultModelPresetForCapabilities(platformCapabilities));
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
        kimi:        { baseUrl: 'https://api.moonshot.cn/v1',              model: 'kimi-k3' },
        openai_compatible: { baseUrl: 'https://api.openai.com/v1',        model: 'gpt-5.6-terra' },
        qwen:        { baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', model: 'qwen3.7-plus' },
        doubao:      { baseUrl: 'https://ark.cn-beijing.volces.com/api/v3', model: 'doubao-seed-evolving' },
        minimax:     { baseUrl: 'https://api.minimaxi.com/v1',            model: 'MiniMax-M3' },
        glm:         { baseUrl: 'https://open.bigmodel.cn/api/paas/v4',   model: 'glm-5.2' },
        mimo:        { baseUrl: 'https://api.xiaomimimo.com/v1',          model: 'mimo-v2.5-pro' },
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
        const defs = PRESET_DEFAULTS[preset] || PRESET_DEFAULTS[defaultModelPresetForCapabilities(platformCapabilities)];
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
      // 移动壳层只作用于 Web 端紧凑视口：底部 Tab + 顶栏，侧栏只保留抽屉形态。
      const compactViewport = useCompactViewport();
      const isCompactShell = isWeb && compactViewport;
      // iOS Safari 上 100dvh 不等于真实可见高度（动态工具栏/安全区），用 visualViewport 兜底。
      const visualViewportHeight = useVisualViewportHeight();
      // iOS Safari 聚焦输入框时会尝试滚动整个文档。紧凑 Web 壳层本身已经按
      // visualViewport 缩高，若再允许文档级平移，整个应用会被推到键盘上方，只剩白屏。
      useEffect(() => {
        if (!isCompactShell) return undefined;

        const html = document.documentElement;
        const body = document.body;
        html.classList.add('compact-web-viewport');

        let frame = 0;
        let settleTimer = 0;
        const resetDocumentScroll = () => {
          window.cancelAnimationFrame(frame);
          window.clearTimeout(settleTimer);
          const reset = () => {
            window.scrollTo(0, 0);
            html.scrollTop = 0;
            body.scrollTop = 0;
          };
          frame = window.requestAnimationFrame(reset);
          // Safari 的自动聚焦平移可能晚于 focusin/viewport resize，再收敛一次。
          settleTimer = window.setTimeout(reset, 120);
        };

        const viewport = window.visualViewport;
        document.addEventListener('focusin', resetDocumentScroll);
        viewport?.addEventListener('resize', resetDocumentScroll);
        viewport?.addEventListener('scroll', resetDocumentScroll);
        resetDocumentScroll();

        return () => {
          html.classList.remove('compact-web-viewport');
          document.removeEventListener('focusin', resetDocumentScroll);
          viewport?.removeEventListener('resize', resetDocumentScroll);
          viewport?.removeEventListener('scroll', resetDocumentScroll);
          window.cancelAnimationFrame(frame);
          window.clearTimeout(settleTimer);
        };
      }, [isCompactShell]);
      const [mobileMoreOpen, setMobileMoreOpen] = useState(false);
      const canDetachWindows = can('detachWindows');
      const [chatPrefill, setChatPrefill] = useState('');
      const [searchOverlayOpen, setSearchOverlayOpen] = useState(false);
      const composerPrefillSeenRef = useRef(0);
      const scheduledTaskAutoOpenSeenRef = useRef(null);
      const [personaEditor, setPersonaEditor] = useState(null); // 聊天里"存入卡牌池"草稿 → App 级编辑器
      const [savedConfirm, setSavedConfirm] = useState(null); // 存入成功 → iOS 确认窗 {name}
      const [poolMyOnly, setPoolMyOnly] = useState(false); // 跳卡池时是否直接落「我的卡牌」筛选(从确认窗"去查看"进来=true)
      const [webAccessOpen, setWebAccessOpen] = useState(false);
      const [settingsUpdateFocusTick, setSettingsUpdateFocusTick] = useState(0);
      const [settingsInitialSection, setSettingsInitialSection] = useState('general');
      // 收纳 toast「前往查看」→ 对话管理页并直接展开「已收纳」面板(一次性信号,SearchView 消费后复位)
      const [searchShowArchived, setSearchShowArchived] = useState(false);
      const [petFocusComposerTick, setPetFocusComposerTick] = useState(0);
      const petSnapshotRef = useRef([]);
      const petSnapshotSequenceRef = useRef(0);

      // ── 多窗口(撕离/tear-off):长按标签 → 浮起跟手 → 拖到目标屏 → 松手 → 该屏最大化打开 ──
      // dragAvatar = 被拎起的标签副本(跟随光标的 DOM 元素);null=没在拖。原生只判落点,视觉全在这。
      const [dragAvatar, setDragAvatar] = useState(null); // {key,label,dx,dy,w,h,x,y}
      const dragOffsetRef = useRef({ dx: 0, dy: 0 });
      const beginTearOff = (kind, id, label, info) => {
        const inv = isTauriAvailable() ? invokeTauri : null;
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
        if (!isTauriAvailable()) return;
        let un;
        tauriEvents.listen('detach:drag-ended', () => setDragAvatar(null)).then(f => { un = f; });
        return () => { if (un) un(); };
      }, []);

      const t = dict[language];
      // 静态 HTML 的 <title>/<html lang> 与非模块脚本(远程文件选择器、web bootstrap)拿不到语言上下文,
      // 在此按当前语言同步,并把选择器/bootstrap 错误文案暴露给 platform/web/ 下的脚本。
      // 桌宠窗口标题由 PetWindow 自行同步(主包不做桌宠检测,见 pet_bootstrap_isolation 测试)。
      useEffect(() => {
        const misc = t.uiPlatformMisc;
        if (!misc) return;
        document.title = misc.appTitle;
        if (misc.htmlLang) document.documentElement.lang = misc.htmlLang;
        window.PinvouHostFilePickerStrings = misc.hostFilePicker;
        // platform/web/bootstrap.js 的 invoke 拒绝错误文案（web bootstrap 内置中文兜底）。
        window.PinvouWebClientStrings = misc.webClientErrors;
      }, [t]);
      // 有可用新版 → 侧边栏设置图标亮红点（不弹窗不打断）
      const hasUpdate = !!(bs && bs.updateInfo && bs.updateInfo.available);
      const isWebAccessConnected = !!(bs && bs.webAccess && bs.webAccess.web_client_connected);
      function handleOpenWebAccess() {
        if (!can('webAccessAdmin')) return;
        setWebAccessOpen(true);
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
          if (bs.activeSessionId && currentView !== 'codex' && currentView !== 'monitor' && currentView !== 'settings' && currentView !== 'search' && currentView !== 'scheduled') {
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
        // UI 语言/主题:启动时从落盘 settings 恢复一次；无语言配置时后端已按系统 locale 补齐。
        if (!uiPrefsInitRef.current && bs.settings) {
          if (isWeb) {
            bootedLanguageRef.current = language;
          } else {
            const lang = TAG_TO_LANG[bs.settings.language];
            if (lang && lang !== language) setLanguage(lang);
            // engine 已用此语言启动,作为「需重启」基线(切语言不重启 engine,见 commands.rs)
            bootedLanguageRef.current = lang || language;
            // 后端 Theme 枚举(prefs.rs)只认 genesis/liquid-light/liquid-dark;深色=genesis,浅色=liquid-light
            const th = bs.settings.theme === 'liquid-light' ? 'light' : 'dark';
            if (th !== activeTheme) setActiveTheme(th);
          }
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
            enabledProviders: Array.isArray(search.enabled_providers) && search.enabled_providers.length
              ? Array.from(new Set(['bing', ...search.enabled_providers]))
              : ['bing', search.provider || 'bing'].filter(Boolean),
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
          setEnabledSearchProviders(saved.enabledProviders);
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
          const preset = effective.preset || adv.model_preset || defaultModelPresetForCapabilities(platformCapabilities);
          const profiles = { ...(adv.model_profiles || {}) };
          const fallback = {
            name: effective.model || adv.custom_model_name || '',
            baseUrl: effective.base_url || adv.custom_base_url || '',
            apiKey: '',
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

      // HMR/旧前端状态可能仍停在已下线入口；立即回到仍可访问的视图。
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
          enabled_providers: Array.from(new Set(['bing', ...enabledSearchProviders, searchProvider])),
          api_key: null,
          credentials,
        };
      }
      // 搜索配置也影响 EngineConfig,需保存后重启进程才生效。
      const savedSearch = savedSearchConfigRef.current;
      const searchCredentialDirty = SEARCH_KEY_PROVIDERS.some(searchProviderCredentialDirty);
      const searchNeedsRestart = !!savedSearch && (
        searchProvider !== savedSearch.provider ||
        JSON.stringify(Array.from(new Set(enabledSearchProviders)).sort()) !== JSON.stringify(Array.from(new Set(savedSearch.enabledProviders || ['bing'])).sort()) ||
        searchCredentialDirty
      );
      // 语言已即时写盘+切 UI,但 LLM 的 locale_tag 要重启 engine 才生效 → 偏离启动语言就提示。
      const languageNeedsRestart = !!bootedLanguageRef.current && language !== bootedLanguageRef.current;

      // Build chat history from sessions
      const sessionBusy = (bs && bs.sessionBusy) || {};
      const chatHistory = bs && bs.sessions ? bs.sessions.map(s => {
        const isPlaceholder = !s.title || DEFAULT_CHAT_TITLES.has(s.title);
        const titlePresentation = isPlaceholder
          ? { text: t.newChat, attachments: [] }
          : sessionTitlePresentation(s.title, s.title_attachment_names);
        return {
          id: s.id,
          // 后端默认标题是三语哨兵之一(见 DEFAULT_CHAT_TITLES;bridge 以此判断是否自动改名)——显示层映射成当前语言
          title: sessionTitlePlainText(titlePresentation),
          titleContent: titlePresentation.attachments.length
            ? <SessionAttachmentTitle presentation={titlePresentation} />
            : null,
          date: formatSessionDate(s.updated_at || s.created_at, language),
          updatedAt: s.updated_at || s.created_at || '',
          pinned: !!s.pinned,
          pinnedAt: s.pinned_at || '',
          working: !!sessionBusy[s.id], // 多 session 并发:该 session 是否正在后台生成
          leadingIcon: <PinvouLogo className="h-[18px] w-[18px]" />,
          testId: 'regular-sidebar-item',
          menuTestId: 'regular-sidebar-menu',
        };
      }) : [];
      const codexHistory = codexSessions.map(session => ({
        id: session.id,
        title: (!session.title || DEFAULT_CHAT_TITLES.has(session.title))
          ? t.newChat
          : session.title,
        subtitle: session.workspace_kind === 'project'
          ? workspaceDisplayName(session.workspace_path)
          : t.uiCodex.temporarySession,
        date: formatSessionDate(session.updated_at || session.created_at, language),
        updatedAt: session.updated_at || session.created_at || '',
        pinned: !!session.pinned,
        pinnedAt: session.pinned_at || '',
        working: !!codexBusyBySession[session.id],
        waitingInput: !!codexWaitingInputBySession[session.id],
        taskKind: 'codex',
        leadingIcon: <AcpAgentLogo agentId={session.agent_id} className="h-[18px] w-[18px]" title={session.agent_name || t.acpAgent} />,
        testId: 'codex-sidebar-item',
        menuTestId: 'codex-sidebar-menu',
        codexSession: session,
      }));
      const pinnedChatHistory = chatHistory
        .filter(chat => chat.pinned)
        .sort((a, b) => String(b.pinnedAt || b.updatedAt).localeCompare(String(a.pinnedAt || a.updatedAt)));
      const scheduledRunShortcuts = (bs && bs.scheduledTaskRecentRuns && bs.scheduledTaskRecentRuns.length)
        ? bs.scheduledTaskRecentRuns
        : (!bridge.available ? PREVIEW_SCHEDULED_RUN_SHORTCUTS.map(run => ({ ...run, taskName: t[run.taskNameKey] || run.taskNameKey })) : []);
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
          const title = (!rawTitle || DEFAULT_CHAT_TITLES.has(rawTitle))
            ? (run.taskName || t.scheduledPlans)
            : rawTitle;
          return {
            id: run.sessionId,
            title,
            updatedAt: run.createdAt || run.scheduledFor || '',
            pinned: !!run.pinned,
            pinnedAt: run.pinnedAt || '',
            working: run.status === 'running' || run.status === 'queued',
            subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
            date: '',
            leadingIcon: (
              <span className="relative inline-flex h-5 w-5 items-center justify-center">
                <Clock size={18} />
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
        const title = (!chat.title || DEFAULT_CHAT_TITLES.has(chat.title))
          ? (run.taskName || t.scheduledPlans)
          : chat.title;
        return Object.assign({}, chat, {
          title,
          subtitle: `${scheduledRunLabel(run.status)} · ${formatSessionDate(run.scheduledFor || run.createdAt, language)}`,
          leadingIcon: (
            <span className="relative inline-flex h-5 w-5 items-center justify-center">
              <Clock size={18} />
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
      const [taskListFilter, setTaskListFilter] = useState('all');
      const [taskListSort, setTaskListSort] = useState('pinned_first');
      const [taskFilterOpen, setTaskFilterOpen] = useState(false);
      const taskFilterRef = useRef(null);
      // 日期组展开状态:未点过的组按默认值走(今天展开、以往折叠),点过后记住用户选择
      const [dateGroupOpen, setDateGroupOpen] = useState({});
      const [archiveConfirm, setArchiveConfirm] = useState(null);
      const [archiveToast, setArchiveToast] = useState(false);
      const [settingsToast, setSettingsToast] = useState('');

      useEffect(() => {
        if (!taskFilterOpen) return undefined;
        const closeOnPointerDown = (event) => {
          if (taskFilterRef.current && !taskFilterRef.current.contains(event.target)) {
            setTaskFilterOpen(false);
          }
        };
        const closeOnEscape = (event) => {
          if (event.key === 'Escape') {
            event.preventDefault();
            setTaskFilterOpen(false);
          }
        };
        document.addEventListener('pointerdown', closeOnPointerDown);
        window.addEventListener('keydown', closeOnEscape);
        return () => {
          document.removeEventListener('pointerdown', closeOnPointerDown);
          window.removeEventListener('keydown', closeOnEscape);
        };
      }, [taskFilterOpen]);

      const sidebarTaskFilterOptions = [
        { id: 'all', label: t.sidebarTaskFilterAll },
        { id: 'pinned', label: t.sidebarTaskFilterPinned },
        { id: 'code', label: t.sidebarTaskFilterCode },
        { id: 'scheduled', label: t.sidebarTaskFilterScheduled },
      ];
      const sidebarTaskSortOptions = [
        { id: 'pinned_first', label: t.sidebarTaskSortPinnedFirst },
        { id: 'recent', label: t.sidebarTaskSortRecent },
      ];
      const allSidebarTasks = pinnedHistory
        .map((chat) => {
          const run = chat.scheduledRun || scheduledRunBySessionId[chat.id];
          const item = decorateScheduledRunChat(chat, run);
          return { ...item, taskKind: run ? 'scheduled' : 'regular' };
        })
        .concat(regularHistory.map(chat => ({ ...chat, taskKind: 'regular' })))
        .concat(scheduledRunHistory.map(chat => ({ ...chat, taskKind: 'scheduled' })))
        .concat(codexHistory);
      const sidebarTaskHistory = allSidebarTasks
        .filter((chat) => {
          if (taskListFilter === 'pinned') return !!chat.pinned;
          if (taskListFilter === 'code') return chat.taskKind === 'codex';
          if (taskListFilter === 'scheduled') return chat.taskKind === 'scheduled';
          return true;
        })
        .sort((a, b) => {
          if (taskListSort === 'pinned_first' && !!a.pinned !== !!b.pinned) {
            return a.pinned ? -1 : 1;
          }
          const aTime = (taskListSort === 'pinned_first' && a.pinned)
            ? (a.pinnedAt || a.updatedAt)
            : (a.updatedAt || a.pinnedAt);
          const bTime = (taskListSort === 'pinned_first' && b.pinned)
            ? (b.pinnedAt || b.updatedAt)
            : (b.updatedAt || b.pinnedAt);
          return String(bTime || '').localeCompare(String(aTime || ''));
        });

      // 任务列表按日期堆叠:今天默认展开、以往默认折叠;组内顺序沿用上面的筛选+排序结果,
      // 组间按日期倒序,无时间戳的落 'unknown' 组沉底。
      // 「置顶优先」排序下置顶项提升到所有日期组之上,否则旧会话会埋进默认折叠的以往分组,
      // 只剩置顶标志、没有置顶效果。
      const todayDateKey = localDateKey(Date.now());
      const sidebarPinnedHoisted = taskListSort === 'pinned_first'
        ? sidebarTaskHistory.filter(chat => !!chat.pinned)
        : [];
      const sidebarTaskGroups = [];
      {
        const byDate = new Map();
        sidebarTaskHistory.forEach(chat => {
          if (sidebarPinnedHoisted.length && chat.pinned) return;
          const key = localDateKey(chat.updatedAt || chat.pinnedAt);
          if (!byDate.has(key)) byDate.set(key, []);
          byDate.get(key).push(chat);
        });
        byDate.forEach((rows, key) => sidebarTaskGroups.push({ key, rows }));
        sidebarTaskGroups.sort((a, b) => {
          if (a.key === 'unknown') return 1;
          if (b.key === 'unknown') return -1;
          return b.key.localeCompare(a.key);
        });
      }

      petSnapshotRef.current = chatHistory.map(chat => ({
        id: chat.id,
        title: chat.title,
        working: chat.working,
      }));
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return undefined;
        let disposed = false;
        let unlisten = null;
        const broadcast = () => {
          if (typeof ev.emit !== 'function') return Promise.resolve();
          return ev.emit('pet:activity_snapshot', {
            sequence: ++petSnapshotSequenceRef.current,
            sessions: petSnapshotRef.current,
          }).catch(() => {});
        };
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
        if (bs && bs.scheduledRunContext && bridge.available && bridge.scheduled.exitScheduledRunChat) {
          const exited = await bridge.scheduled.exitScheduledRunChat();
          if (!exited) return false;
        }
        if (beforeNavigate) beforeNavigate();
        setCurrentView(nextView);
        closeMobileSidebar();
        return true;
      }

      function openSettingsSection(section = 'general') {
        // 记录进入设置前的页面（代码页齿轮等深链入口），关闭设置时原路返回，
        // 而不是一律回工作页。
        if (currentView !== 'settings') settingsReturnViewRef.current = currentView;
        setSettingsInitialSection(section);
        return navigateFromScheduledRun('settings');
      }

      function closeMobileSidebar() {
        if (!isWeb || typeof window === 'undefined') return;
        if (window.matchMedia && window.matchMedia('(max-width: 639px)').matches) {
          setIsSidebarOpen(false);
        }
      }

      function scheduledRunLabel(value) {
        return (t.uiScheduled.runStatus[value] || value || t.uiScheduled.unknown);
      }

      async function handleOpenScheduledRunShortcut(run) {
        if (!run || !run.sessionId) return;
        if (!bridge.available || !bridge.scheduled.openScheduledRunChat) {
          setCurrentView('scheduled');
          closeMobileSidebar();
          return;
        }
        const task = {
          id: run.automationId,
          name: run.taskName || t.scheduledPlans,
          model: run.taskModel || null,
        };
        const opened = await bridge.scheduled.openScheduledRunChat(run, task);
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
        if (currentView === 'codex' && codexAcpSupported) {
          updateActiveCodexSession(null);
          setCodexDraftEpoch(value => value + 1);
          setCurrentView('codex');
        } else {
          if (bridge.available) bridge.sessions.createNewSession();
          setCurrentView('chat');
        }
        closeMobileSidebar();
      }

      function handleSwitchHomeMode(mode) {
        if (mode === 'code' && codexAcpSupported) {
          updateActiveCodexSession(null);
          setCodexDraftEpoch(value => value + 1);
          setCurrentView('codex');
        } else if (mode === 'design') {
          // 仅草稿态（无活跃会话）才开新会话：从 code 页切回时 bridge 的
          // activeSessionId 仍是原工作会话，强制 createNewSession 会新建一个
          // plain 会话（默认 Yolo），把用户切过的 Plan 顶掉——表现为「从代码
          // 切回工作/设计，审批模式变回 Yolo」。保留原会话，ChatView 挂载后
          // 显示其实测 mode。与 ChatView 内 work↔design 本地切换（不建会话）
          // 行为保持一致。
          const scopeKey = bridge.activeSessionId
            ? createPinvouModeScopeKey(bridge.activeSessionId)
            : undefined;
          savePinvouModeState({ mode: 'design' }, undefined, scopeKey);
          if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();
          // code 页期间原工作会话的 mode 可能已被修改（code 页独立链路），
          // 切回前拉一次实测值，避免 ChatView 挂载后显示旧 modeState。
          if (bridge.available && bridge.activeSessionId) {
            bridge.interaction.syncModeState().catch(() => {});
          }
          setCurrentView('chat');
        } else if (mode === 'work') {
          const scopeKey = bridge.activeSessionId
            ? createPinvouModeScopeKey(bridge.activeSessionId)
            : undefined;
          savePinvouModeState({ mode: 'work' }, undefined, scopeKey);
          if (bridge.available && !bridge.activeSessionId) bridge.sessions.createNewSession();
          if (bridge.available && bridge.activeSessionId) {
            bridge.interaction.syncModeState().catch(() => {});
          }
          setCurrentView('chat');
        }
        closeMobileSidebar();
      }

      // AI 造卡:新对话 + 加持「卡牌制造专家」+ 一条 iOS 引导卡 → 用户在空输入框描述需求,复用 persona-card 草稿流程入库
      async function startAICard() {
        handleNewChat();
        if (!bridge.available) return;
        await bridge.personas.equipPersona('pinvou-card-creator');           // 先加持(落新 session + 加持气泡)
        bridge.personas.postCardCreatorIntro();                              // 再排在加持气泡之后(持久化,切会话/重启不丢)
      }

      async function handleSwitchSession(id) {
        if (!bridge.available) return;
        // Web RPC 可能跨公网 Relay：先关闭抽屉并切入聊天路由，后台再加载会话。
        setCurrentView('chat');
        closeMobileSidebar();
        const switched = await bridge.sessions.switchToSession(id);
        if (!switched) return;
        setActiveChat(id);
      }

      async function handleSearchSelect(id) {
        await handleSwitchSession(id);
        setSearchOverlayOpen(false);
      }

      function handleSwitchCodexSession(id) {
        updateActiveCodexSession(id);
        setCurrentView('codex');
        closeMobileSidebar();
      }

      // 用户在主窗口里亲眼看着完成的会话，公仔的活动卡属于冗余提醒——
      // 完成瞬间若该会话正处于前台聊天视图且窗口有焦点，直接标记已读，
      // 卡片自动消失，不需要用户再去点。
      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev) return undefined;
        let disposed = false;
        const unlisteners = [];
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
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
        const ev = isTauriAvailable() ? tauriEvents : null;
        if (!ev || currentView !== 'chat' || !activeChat) return;
        if (typeof document.hasFocus === 'function' && !document.hasFocus()) return;
        const emit = emitPetEvent(ev, 'pet:session_viewed', { session_id: activeChat });
        emit.catch(() => {});
      }, [currentView, activeChat]);

      useEffect(() => {
        const ev = isTauriAvailable() ? tauriEvents : null;
        const core = isTauriAvailable() ? tauriCommands : null;
        if (!ev || !core) return undefined;
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
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
              if (!bridge.available || !bridge.scheduled.openScheduledRunChat) {
                emitToPet('pet:scheduled_notice_open_failed', { run_id: runId }).catch(() => {});
                return;
              }
              let opened = false;
              try {
                opened = await bridge.scheduled.openScheduledRunChat({
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
            const switched = await bridge.sessions.switchToSession(sid);
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
        const ev = isTauriAvailable() ? tauriEvents : null;
        const core = isTauriAvailable() ? tauriCommands : null;
        if (!ev || !core || !bridge.available || !bridge.chat.sendMessageToSession) return undefined;
        let disposed = false;
        let consuming = false;
        let rerun = false;
        let unlisten = null;
        const emitToPet = (name, payload) => emitPetEvent(ev, name, payload);
        const consume = async () => {
          if (disposed) return;
          if (consuming) {
            rerun = true;
            return;
          }
          consuming = true;
          try {
            if (typeof bridge.lifecycle.init === 'function') await bridge.lifecycle.init();
            while (!disposed) {
              const request = await core.invoke('take_pet_reply');
              if (!request) break;
              const requestId = request.request_id || request.requestId;
              const sid = request.session_id || request.sessionId;
              const text = String(request.text || '').trim();
              const liveSessions = bridge.state
                ? (bridge.state.get('sessions').sessions || [])
                : [];
              const sessionExists = petSnapshotRef.current.some(
                session => String(session.id) === String(sid),
              ) || liveSessions.some(session => String(session.id) === String(sid));
              if (!sessionExists) {
                await emitToPet('pet:reply_failed', {
                  request_id: requestId,
                  session_id: sid,
                  error: t.uiMainApp.petSessionMissing,
                  unavailable: true,
                }).catch(() => {});
                continue;
              }
              try {
                const result = await bridge.chat.sendMessageToSession(sid, text);
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
                      error: String(outcome?.error?.message || outcome?.error || t.uiMainApp.petTaskStartFailed),
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

      async function handleDeleteSession(id) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.deleteSession(id);
        if (isCodexSession) {
          if (activeCodexId === id) updateActiveCodexSession(null);
          await refreshCodexSessions().catch(() => {});
        }
      }

      async function handleRenameSession(id, title) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.renameSession(id, title);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }

      async function handleToggleSessionPinned(id, pinned) {
        const isCodexSession = codexSessions.some(session => session.id === id);
        if (bridge.available) await bridge.sessions.toggleSessionPinned(id, pinned);
        if (isCodexSession) await refreshCodexSessions().catch(() => {});
      }

      function handleArchiveSession(id) {
        const chat = allSidebarTasks.find(c => c.id === id);
        setArchiveConfirm(chat || { id, title: t.newChat });
      }

      async function confirmArchiveSession() {
        const id = archiveConfirm && archiveConfirm.id;
        const isCodexSession = archiveConfirm && archiveConfirm.taskKind === 'codex';
        setArchiveConfirm(null);
        if (id && bridge.available) {
          try {
            const archived = await bridge.sessions.archiveSession(id);
            if (archived === false) {
              setSettingsToast(t.sessionBatchFailed(1));
              return;
            }
            if (isCodexSession) {
              if (activeCodexId === id) updateActiveCodexSession(null);
              await refreshCodexSessions().catch(() => {});
            }
            setArchiveToast(true);
          } catch (error) {
            console.warn('archive session failed', error);
            setSettingsToast(t.sessionBatchFailed(1));
          }
        }
      }

      async function handleRestoreArchivedSession(id) {
        if (bridge.available) await bridge.sessions.restoreArchivedSession(id);
        await refreshCodexSessions().catch(() => {});
      }

      function sessionRowsForIds(ids) {
        const byId = new Map(allSidebarTasks.map(item => [item.id, item]));
        return (ids || []).map(id => byId.get(id) || { id });
      }

      function reportBatchFailures(result) {
        if (result.failed > 0) setSettingsToast(t.sessionBatchFailed(result.failed));
      }

      // 对话管理页批量操作:按会话类型分流并等待全部结果,避免未执行完成就误报成功。
      async function handleBatchArchiveSessions(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(sessionRowsForIds(ids), 'archive', {
          archive: id => bridge.sessions.archiveSession(id),
          archiveCodex: id => bridge.sessions.archiveSession(id),
        });
        const nextCodexSessions = await refreshCodexSessions().catch(() => null);
        if (activeCodexId && Array.isArray(nextCodexSessions) && !nextCodexSessions.some(session => session.id === activeCodexId)) {
          updateActiveCodexSession(null);
        }
        if (result.succeeded > 0) setArchiveToast(true);
        reportBatchFailures(result);
        return result;
      }

      async function handleBatchDeleteSessions(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(sessionRowsForIds(ids), 'delete', {
          delete: id => bridge.sessions.deleteSession(id),
        });
        const nextCodexSessions = await refreshCodexSessions().catch(() => null);
        if (activeCodexId && Array.isArray(nextCodexSessions) && !nextCodexSessions.some(session => session.id === activeCodexId)) {
          updateActiveCodexSession(null);
        }
        reportBatchFailures(result);
        return result;
      }

      async function handleBatchRestoreArchived(ids) {
        if (!bridge.available || !ids || !ids.length) return;
        const result = await runSessionBatch(ids.map(id => ({ id })), 'restore', {
          restore: id => bridge.sessions.restoreArchivedSession(id),
        });
        await refreshCodexSessions().catch(() => {});
        reportBatchFailures(result);
        return result;
      }

      useEffect(() => {
        if (!archiveToast) return;
        const timer = setTimeout(() => setArchiveToast(false), 3500);
        return () => clearTimeout(timer);
      }, [archiveToast]);

      useEffect(() => {
        if (!settingsToast) return;
        const timer = setTimeout(() => setSettingsToast(''), 3000);
        return () => clearTimeout(timer);
      }, [settingsToast]);

      async function handleToggleSuperPerm() {
        const target = !superPerm;
        if (!bridge.available) {
          setSuperPerm(target);
          return;
        }
        setSuperPerm(target);
        try {
          const result = await bridge.interaction.toggleSuperPerm();
          if (!result || result.ok === false) {
            setSuperPerm(!!(result && result.enabled));
            setSettingsToast((result && result.error) || t.uiMainApp.superPermFailed);
          }
        } catch (error) {
          setSuperPerm(!target);
          setSettingsToast(String(error || t.uiMainApp.superPermFailed));
        }
      }

      function handleSetTheme(th) {
        setActiveTheme(th);
        if (isWeb) {
          try { window.localStorage.setItem('pinvou.web.theme', th); } catch (_) {}
          return;
        }
        if (bridge.available) {
          bridge.settings.saveSettings({ theme: th === 'dark' ? 'genesis' : 'liquid-light' });
        }
      }

      function handleSetSearchProvider(p) {
        if (p === searchProvider) return;
        setEnabledSearchProviders(prev => Array.from(new Set(['bing', ...prev, p])));
        setSearchProvider(p);
        setSearchApiKey(searchKeyDrafts[p] || '');
      }

      function handleAddSearchProvider(p) {
        setEnabledSearchProviders(prev => Array.from(new Set(['bing', ...prev, p])));
        handleSetSearchProvider(p);
      }

      function handleDeleteSearchProvider(p) {
        if (p === 'bing') return;
        setEnabledSearchProviders(prev => {
          const next = prev.filter(x => x !== p);
          return next.length ? next : ['bing'];
        });
        setSearchKeyDrafts(prev => ({ ...prev, [p]: '' }));
        setSearchKeyActions(prev => ({ ...prev, [p]: 'delete' }));
        if (searchProvider === p) handleSetSearchProvider('bing');
      }

      function handleTestSearchProvider(p) {
        if (!bridge.available || !bridge.settings.testSearchProvider) return Promise.resolve(t.uiMainApp.searchTestUnavailable);
        const action = searchProviderKeyAction(p);
        const draft = searchKeyDrafts[p] || '';
        return bridge.settings.testSearchProvider(p, action === 'replace' ? draft : '');
      }

      function handleSetSearchApiKey(k, providerOverride) {
        const targetProvider = providerOverride || searchProvider;
        if (targetProvider === searchProvider) setSearchApiKey(k);
        setSearchKeyDrafts(prev => ({ ...prev, [targetProvider]: k }));
        setSearchKeyActions(prev => ({ ...prev, [targetProvider]: k.trim() ? 'replace' : 'keep_existing' }));
      }

      async function handleConfirmSearchConfig() {
        if (!bridge.available) return;
        const search = buildSearchSettingsPayload();
        // 浏览器宿主没有重启桌面进程的权限；只保存，待桌面端下次重启后生效。
        const saved = isWeb
          ? await bridge.settings.saveSearchSettings(search)
          : await bridge.settings.saveSearchSettingsAndRestart(search);
        if (saved === false) setSettingsToast(t.uiMainApp.searchSaveFailed);
      }

      async function handleSaveSearchConfig() {
        if (!bridge.available) return true;
        const search = buildSearchSettingsPayload();
        const saved = await bridge.settings.saveSearchSettings(search);
        if (saved === false) {
          setSettingsToast(t.uiMainApp.searchSaveFailed);
          return false;
        }
        return true;
      }

      function handleSetLanguage(lang) {
        setLanguage(lang);
        if (isWeb) {
          try { window.localStorage.setItem('pinvou.web.language', lang); } catch (_) {}
          return;
        }
        if (isTauriAvailable()) {
          tauriEvents.emit('ui:language_changed', { language: lang }).catch(() => {});
        }
        if (bridge.available) {
          bridge.settings.saveSettings({ language: LANG_TO_TAG[lang] || 'zh-Hans' });
        }
      }

      function handleSetMemoryEnabled(enabled) {
        if (bridge.available) {
          const memoryAvailable = (LANG_TO_TAG[language] || 'zh-Hans') === 'zh-Hans';
          bridge.settings.saveSettings({ memory_enabled: memoryAvailable && !!enabled });
        }
      }

      function handleSetPetEnabled(enabled) {
        if (!can('pet') || !bridge.available) return;
        // 单一路径:set_pet_enabled 负责持久化 + 窗口显隐 + 广播
        // pet:enabled_changed(bridge 听到后刷新 settings 副本,防旧值回写)。
        invokeTauri('set_pet_enabled', { enabled: !!enabled }).catch(() => {});
      }

      async function handleSetTaskCompletedNotif(enabled) {
        const nextEnabled = !!enabled;
        const previousEnabled = taskCompletedNotif;
        setTaskCompletedNotif(nextEnabled);
        if (bridge.available) {
          const saved = await bridge.settings.saveSettings({
            notifications: { enabled: nextEnabled, task_completed: nextEnabled },
          });
          if (saved === false) {
            setTaskCompletedNotif(previousEnabled);
          }
        }
      }

      // 侧栏任务列表「按日期折叠」开关:纯 UI 偏好,写 settings.sidebar.date_grouping
      function handleSetSidebarDateGrouping(enabled) {
        if (bridge.available) bridge.settings.saveSettings({ sidebar: { date_grouping: !!enabled } });
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
          bridge.settings.saveSettingsAndRestart({
            advanced: buildAdvancedOverrides({
              model_preset: modelPreset,
              custom_model_name: customModelName || null,
              custom_base_url: customBaseUrl || null,
              custom_api_key: customApiKey || null,
            }),
          });
        }
      }

      // 移动壳层派生数据：顶栏标题跟随当前视图（对话态显示会话标题）；
      // 未读红点与侧栏入口同源，避免两套提醒逻辑漂移。
      const scheduledUnread = !!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns));
      const mobileTitle = currentView === 'chat'
        ? ((((chatHistory || []).find(c => c.id === activeChat)) || {}).title || 'PINVOU')
        : currentView === 'codex'
          ? ((((codexHistory || []).find(c => c.id === activeCodexId)) || {}).title || t.sidebarTaskFilterCode)
        : ({ search: t.searchChats, scheduled: t.scheduledPlans, monitor: t.monitor, cardpool: t.cardPool, toolStore: t.toolStore, outputs: t.outputs, knowledge: t.knowledge, settings: t.settings }[currentView] || 'PINVOU');
      const mobileNavigate = (view, beforeNavigate) => {
        setMobileMoreOpen(false);
        navigateFromScheduledRun(view, beforeNavigate);
      };
      const mobileMoreViews = ['search', 'outputs', 'knowledge', 'toolStore', 'settings'];
      const mobileMoreActive = mobileMoreViews.includes(currentView)
        || (currentView === 'scheduled' && !(bs && bs.scheduledRunContext));

      // 侧栏任务列表按日期折叠(默认开;settings.sidebar.date_grouping === false 时平铺)
      const sidebarDateGrouping = !bs || !bs.settings || !bs.settings.sidebar || bs.settings.sidebar.date_grouping !== false;
      // 日期分组/平铺两种布局共用的任务项渲染
      const renderSidebarTaskItem = (chat) => {
        const detachKind = chat.taskKind === 'codex' ? 'codex-session' : 'session';
        return (
          <RecentItem
            key={chat.taskKind === 'scheduled' ? `${chat.scheduledRun?.automationId || ''}:${chat.scheduledRun?.id || chat.id}` : `${chat.taskKind}:${chat.id}`}
            chat={chat}
            theme={activeTheme}
            t={t}
            active={chat.taskKind === 'codex'
              ? activeCodexId === chat.id && currentView === 'codex'
              : chat.scheduledRun
                ? !!(bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === chat.id)
                : activeChat === chat.id && currentView === 'chat'}
            personaTarget={chat.taskKind !== 'codex' && !chat.scheduledRun && activeChat === chat.id && currentView === 'cardpool'}
            onSelect={chat.taskKind === 'codex'
              ? handleSwitchCodexSession
              : chat.scheduledRun
                ? () => handleOpenScheduledRunShortcut(chat.scheduledRun)
                : handleSwitchSession}
            onRename={handleRenameSession}
            onDelete={handleDeleteSession}
            onTogglePinned={handleToggleSessionPinned}
            onOpenFolder={can('externalSystemOpen') ? ((id) => bridge.artifacts.revealSessionFolder && bridge.artifacts.revealSessionFolder(id)) : undefined}
            onArchive={handleArchiveSession}
            dragKind={detachKind}
            dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === `${detachKind}:${chat.id}`}
            onPickUp={canDetachWindows ? ((geom) => beginTearOff(detachKind, chat.id, chat.title, geom)) : undefined}
          />
        );
      };

      return (
        <div data-testid="app-root" data-current-view={currentView} data-platform={isWeb ? 'web' : 'desktop'}
          className={`flex flex-col h-screen font-sans overflow-hidden antialiased transition-colors duration-300 ${activeTheme === 'dark' ? 'bg-[#131314] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}
          style={isWeb ? {
            ...(isCompactShell ? { position: 'fixed', inset: 0, width: '100%' } : {}),
            height: visualViewportHeight ? `${visualViewportHeight}px` : '100dvh',
            paddingTop: 'env(safe-area-inset-top)',
            paddingRight: 'env(safe-area-inset-right)',
            paddingBottom: 'env(safe-area-inset-bottom)',
            paddingLeft: 'env(safe-area-inset-left)',
          } : undefined}>

          <WebConnectionStatus theme={activeTheme} t={t} />

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
                setSearchShowArchived(true);
                navigateFromScheduledRun('search');
              }}
            />,
            document.body
          )}

          {settingsToast && createPortal(
            <div className="fixed left-1/2 bottom-8 z-[120] -translate-x-1/2 rounded-full bg-black/80 px-4 py-2 text-[13px] font-medium text-white shadow-2xl">
              {settingsToast}
            </div>,
            document.body
          )}

          {searchOverlayOpen && createPortal(
            <SearchOverlay
              theme={activeTheme}
              history={chatHistory}
              t={t}
              onSelect={handleSearchSelect}
              onClose={() => setSearchOverlayOpen(false)}
            />,
            document.body
          )}

          {can('desktopChrome') && <TitleBar theme={activeTheme} t={t} sidebarOpen={isSidebarOpen} />}

          {isCompactShell && (
            <MobileTopBar theme={activeTheme} t={t} title={mobileTitle}
              onMenu={() => setIsSidebarOpen(true)}
              onNewChat={currentView === 'chat' || currentView === 'codex' ? () => handleNewChat() : undefined} />
          )}

          <div className={`flex flex-1 min-h-0 ${activeTheme === 'dark' ? (isSidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]') : 'bg-[#F0F4F9]'}`}>

          {isWeb && isSidebarOpen && (
            <button
              type="button"
              data-testid="mobile-navigation-close"
              aria-label={t.uiMainApp.closeNavigation}
              onClick={() => setIsSidebarOpen(false)}
              className="fixed inset-0 z-30 hidden bg-black/40 max-sm:block"
            />
          )}

          {/* ================= Sidebar (Gemini Style) ================= */}
          <div
            data-testid="app-sidebar"
            style={isCompactShell ? {
              display: isSidebarOpen ? 'flex' : 'none',
              position: 'fixed',
              left: 0,
              top: 48,
              bottom: 56,
            } : undefined}
            className={`${isSidebarOpen ? 'w-[280px]' : 'w-[68px]'} shrink-0 flex flex-col z-40 transition-all duration-300 ${
              activeTheme === 'light'
                ? 'bg-[#F0F4F9]'
                : (isSidebarOpen ? 'bg-[#1E1F20]' : 'bg-[#131314]')
            }`}>

            {/* Header / Logo */}
            <div className={`px-4 py-3 max-sm:px-3 max-sm:py-0 flex items-center ${isSidebarOpen ? 'gap-3' : 'justify-center'} overflow-hidden`}>
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
              {isSidebarOpen && !isCompactShell && (
                <button
                  type="button"
                  onClick={() => setSearchOverlayOpen(true)}
                  title={t.searchChats}
                  aria-label={t.searchChats}
                  className={`ml-auto w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                    searchOverlayOpen
                      ? (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#0B57D0]')
                      : (activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]')
                  }`}
                >
                  <Search size={19} />
                </button>
              )}
            </div>

            {/* Navigation — shrink-0 固定不滚动,list 再多也不挤压 nav */}
            <div data-testid="sidebar-primary-nav" className={`shrink-0 flex flex-col gap-0.5 mt-1.5 max-sm:gap-0 max-sm:mt-1 ${isSidebarOpen ? 'px-3' : 'px-2 items-center'}`}>
              <NavItem
                icon={<Edit2 size={18} />} label={t.newChat}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => handleNewChat()}
              />
              {(!isSidebarOpen || isCompactShell) && (
                <NavItem
                  icon={<Search size={18} />} label={t.searchChats}
                  active={searchOverlayOpen}
                  theme={activeTheme}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => setSearchOverlayOpen(true)}
                />
              )}
              {SCHEDULED_TASKS_ENTRY_ENABLED && (
                <NavItem
                  icon={<Clock size={18} />} label={t.scheduledPlans}
                  active={currentView === 'scheduled'}
                  unread={!!(bs && (bs.scheduledTasks || []).some(task => task.hasUnreadRuns))}
                  theme={activeTheme}
                  t={t}
                  isSidebarOpen={isSidebarOpen}
                  onClick={() => navigateFromScheduledRun('scheduled')}
                />
              )}
              <NavItem
                icon={<Package size={18} />} label={t.outputs}
                active={currentView === 'outputs'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('outputs')}
                dragKind={canDetachWindows ? 'outputs' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'outputs:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('outputs', undefined, t.outputs, geom) : undefined}
              />
              <NavItem
                icon={<BarChart2 size={18} />} label={t.monitor}
                active={currentView === 'monitor'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => {
                  navigateFromScheduledRun('monitor', () => {
                    const liveBridge = window.TauriBridge || bridge;
                    if (liveBridge?.monitor && typeof liveBridge.monitor.startMonitorPolling === 'function') liveBridge.monitor.startMonitorPolling();
                  });
                }}
                dragKind={canDetachWindows ? 'monitor' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'monitor:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('monitor', undefined, t.monitor, geom) : undefined}
              />
              <NavItem
                icon={<Puzzle size={18} />} label={t.toolStore}
                active={currentView === 'toolStore'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('toolStore')}
                dragKind={canDetachWindows ? 'toolstore' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'toolstore:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('toolstore', undefined, t.toolStore, geom) : undefined}
              />
              <NavItem
                icon={<Layers size={18} />} label={t.cardPool}
                active={currentView === 'cardpool'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))}
                dragKind={canDetachWindows ? 'cardpool' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'cardpool:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('cardpool', undefined, t.cardPool, geom) : undefined}
              />
              <NavItem
                icon={<BookOpen size={18} />} label={t.knowledge}
                active={currentView === 'knowledge'}
                theme={activeTheme}
                isSidebarOpen={isSidebarOpen}
                onClick={() => navigateFromScheduledRun('knowledge')}
                dragKind={canDetachWindows ? 'knowledge' : undefined} dragging={canDetachWindows && !!dragAvatar && dragAvatar.key === 'knowledge:'} onPickUp={canDetachWindows ? (geom) => beginTearOff('knowledge', undefined, t.knowledge, geom) : undefined}
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
                <div data-testid="sidebar-recents" className="pt-5 pb-2 max-sm:pt-2">
                  <div ref={taskFilterRef} className="relative mb-2">
                    <div className={`group h-8 px-4 flex items-center justify-between rounded-full text-[13px] font-semibold ${
                      activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'
                    }`}>
                      <span className="truncate">
                        {t.sidebarTaskList} ({sidebarTaskHistory.length})
                      </span>
                      <span className="flex items-center">
                        {/* 对话管理页入口:悬停任务列表行显现(触屏常显),替代原搜索入口 */}
                        <button
                          type="button"
                          onClick={() => navigateFromScheduledRun('search')}
                          className={`mr-1 h-6 px-2 shrink-0 rounded-full text-[12px] font-normal transition-opacity opacity-0 group-hover:opacity-100 max-sm:opacity-100 ${activeTheme === 'dark' ? 'text-[#A8C7FA] hover:bg-[#282A2C]' : 'text-[#0B57D0] hover:bg-[#E1E5EA]'}`}
                        >
                          {t.sidebarViewAll}
                        </button>
                        <button
                          type="button"
                          data-testid="sidebar-task-filter"
                          onClick={() => setTaskFilterOpen(v => !v)}
                          title={t.sidebarTaskFilter}
                          className={`w-7 h-7 -mr-2 shrink-0 rounded-full flex items-center justify-center transition-colors ${
                            taskFilterOpen
                              ? (activeTheme === 'dark' ? 'bg-[#333537] text-[#E3E3E3]' : 'bg-[#E1E5EA] text-[#444746]')
                              : (activeTheme === 'dark' ? 'hover:bg-[#282A2C]' : 'hover:bg-[#E1E5EA]')
                          }`}
                        >
                          <Filter size={15} />
                        </button>
                      </span>
                    </div>
                    {taskFilterOpen && (
                      <div
                        data-testid="sidebar-task-filter-menu"
                        className={`absolute right-0 top-9 z-50 w-44 overflow-hidden rounded-2xl border p-1.5 shadow-xl ${
                          activeTheme === 'dark' ? 'border-white/10 bg-[#202124]' : 'border-black/10 bg-white'
                        }`}
                      >
                        <div className={`px-2.5 pb-1 pt-1 text-[11px] font-semibold ${activeTheme === 'dark' ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>
                          {t.sidebarTaskFilter}
                        </div>
                        {sidebarTaskFilterOptions.map(option => (
                          <button
                            key={option.id}
                            type="button"
                            onClick={() => setTaskListFilter(option.id)}
                            className={`w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`}
                          >
                            <span className="w-4 shrink-0">{taskListFilter === option.id && <Check size={13} />}</span>
                            <span className="truncate">{option.label}</span>
                          </button>
                        ))}
                        <div className={`my-1 h-px ${activeTheme === 'dark' ? 'bg-white/10' : 'bg-black/10'}`} />
                        <div className={`px-2.5 pb-1 pt-1 text-[11px] font-semibold ${activeTheme === 'dark' ? 'text-[#8E8E93]' : 'text-[#8A8A8E]'}`}>
                          {t.sidebarTaskSort}
                        </div>
                        {sidebarTaskSortOptions.map(option => (
                          <button
                            key={option.id}
                            type="button"
                            onClick={() => setTaskListSort(option.id)}
                            className={`w-full px-2.5 py-1.5 flex items-center gap-2 rounded-xl text-left text-[13px] leading-5 transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#303134]' : 'text-[#1F1F1F] hover:bg-[#F1F3F4]'}`}
                          >
                            <span className="w-4 shrink-0">{taskListSort === option.id && <Check size={13} />}</span>
                            <span className="truncate">{option.label}</span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="space-y-1">
                    {!sidebarDateGrouping ? (
                      <div className="space-y-0.5">
                        {sidebarTaskHistory.length > 0 ? sidebarTaskHistory.map(renderSidebarTaskItem) : (
                          <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
                            {t.sidebarTaskEmpty}
                          </div>
                        )}
                      </div>
                    ) : (sidebarPinnedHoisted.length > 0 || sidebarTaskGroups.length > 0) ? (
                      <>
                        {sidebarPinnedHoisted.length > 0 && (
                          <div className="space-y-0.5">
                            {sidebarPinnedHoisted.map(renderSidebarTaskItem)}
                          </div>
                        )}
                        {sidebarTaskGroups.map((group) => {
                      const isOpen = dateGroupOpen[group.key] ?? (group.key === todayDateKey);
                      return (
                        <div key={group.key}>
                          <button
                            type="button"
                            onClick={() => setDateGroupOpen(prev => ({ ...prev, [group.key]: !isOpen }))}
                            className={`w-full h-7 px-4 flex items-center justify-between rounded-full text-[12px] transition-colors ${activeTheme === 'dark' ? 'text-[#9AA0A6] hover:bg-[#282A2C]' : 'text-[#8A8F94] hover:bg-[#E1E5EA]'}`}
                          >
                            <span className="truncate">{formatDateGroupLabel(group.key, language)} ({group.rows.length})</span>
                            <ChevronDown size={14} className={`shrink-0 transition-transform ${isOpen ? '' : '-rotate-90'}`} />
                          </button>
                          {isOpen && (
                            <div className="mt-1 space-y-0.5">
                              {group.rows.map(renderSidebarTaskItem)}
                            </div>
                          )}
                        </div>
                      );
                        })}
                      </>
                    ) : (
                      <div className={`px-3 py-3 text-[13px] ${activeTheme === 'dark' ? 'text-[#9AA0A6]' : 'text-[#8A8F94]'}`}>
                        {t.sidebarTaskEmpty}
                      </div>
                    )}
                  </div>
                </div>
              </div>
            )}

            {/* Footer Profile */}
            <div className={`p-3 mt-auto ${isSidebarOpen ? 'space-y-2' : 'flex flex-col items-center gap-3 pb-6'}`}>
              <div className={`${isSidebarOpen ? 'flex items-center justify-between gap-2' : 'flex flex-col items-center gap-3'}`}>
                {!isSidebarOpen && (
                  <>
                    {can('webAccessAdmin') && <button
                      onClick={handleOpenWebAccess}
                      title={t.uiRemote.title}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Smartphone size={18} />
                      {isWebAccessConnected && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                    </button>}
                    {can('pet') && <button
                      onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                      title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? t.uiPet.hide : t.uiMainApp.petSummon}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#E3E3E3]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                    >
                      <PetPawIcon />
                    </button>}
                    <button
                      data-testid="nav-settings"
                      onClick={() => openSettingsSection('general')}
                      title={t.settings}
                      className={`relative w-10 h-10 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#E3E3E3] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Settings size={18} />
                      {hasUpdate && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#EA4335]" />}
                    </button>
                  </>
                )}
                {showMegacubeSite && (
                  <button
                    onClick={() => invokeTauri('open_external_url', { url: 'https://www.h3c.com/cn/pub/minisite/202606/MegaCube/megacube/index.html' })}
                    title={t.megacubeSite}
                    className={`flex items-center rounded-xl transition-colors ${isSidebarOpen ? 'flex-1 min-w-0 px-2 py-1.5 gap-3' : 'justify-center w-10 h-10'} ${activeTheme === 'dark' ? 'hover:bg-[#333537] active:bg-[#3A3C3E]' : 'hover:bg-[#E1E5EA] active:bg-[#D8DCE1]'}`}
                  >
                    <img src="assets/megacube-icon.png" alt="MegaCube" className="w-8 h-8 shrink-0 rounded-lg object-contain" />
                    {isSidebarOpen && (
                      <span className="text-[14px] font-medium leading-none whitespace-nowrap text-left">MegaCube</span>
                    )}
                  </button>
                )}
                {isSidebarOpen && (
                  <div className="flex items-center gap-1">
                    {can('webAccessAdmin') && <button
                      onClick={handleOpenWebAccess}
                      title={t.uiRemote.title}
                      className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${activeTheme === 'dark' ? 'text-[#C4C7C5] hover:bg-[#333537]' : 'text-[#444746] hover:bg-[#E1E5EA]'}`}
                    >
                      <Smartphone size={18} />
                      {isWebAccessConnected && <span className="absolute top-1 right-1 w-2 h-2 rounded-full bg-[#34A853]" />}
                    </button>}
                    {can('pet') && <button
                      onClick={() => handleSetPetEnabled(!(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled))}
                      title={(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? t.uiPet.hide : t.uiMainApp.petSummon}
                      className={`relative w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors ${(bs && bs.settings && bs.settings.pet && bs.settings.pet.enabled) ? 'text-[#34A853]' : (activeTheme === 'dark' ? 'text-[#C4C7C5]' : 'text-[#444746]')} ${activeTheme === 'dark' ? 'hover:bg-[#333537]' : 'hover:bg-[#E1E5EA]'}`}
                    >
                      <PetPawIcon />
                    </button>}
                    <button
                      data-testid="nav-settings"
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
          </div>

          {/* ================= Main Content ================= */}
          <div className={`flex-1 flex flex-col relative min-w-0 overflow-hidden ${activeTheme === 'dark' ? 'bg-[#131314]' : 'bg-white'} ${isCompactShell ? '' : 'rounded-tl-[28px]'}`}>

            {/* Gemini Style Background Glow */}
            {(currentView === 'chat'
              || currentView === 'codex'
              || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && (
              activeTheme === 'light' ? (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[1200px] h-[800px] bg-[radial-gradient(ellipse_at_center,_rgba(232,240,254,0.8)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              ) : (
                <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-[40%] w-[1400px] h-[900px] bg-[radial-gradient(ellipse_at_center,_rgba(168,199,250,0.25)_0%,_transparent_60%)] pointer-events-none z-0"></div>
              )
            )}

            {currentView === 'monitor' && <MonitorView theme={activeTheme} t={t} bs={bs} />}
            {currentView === 'settings' && (
              <SettingsErrorBoundary theme={activeTheme} t={t}>
                <SettingsView
                  activeTheme={activeTheme} setActiveTheme={handleSetTheme}
                  language={language} setLanguage={handleSetLanguage}
                  superPerm={superPerm} setSuperPerm={handleToggleSuperPerm}
                  taskCompletedNotif={taskCompletedNotif} setTaskCompletedNotif={handleSetTaskCompletedNotif}
                  searchProvider={searchProvider} setSearchProvider={handleSetSearchProvider}
                  enabledSearchProviders={enabledSearchProviders}
                  onAddSearchProvider={handleAddSearchProvider}
                  onDeleteSearchProvider={handleDeleteSearchProvider}
                  onTestSearchProvider={handleTestSearchProvider}
                  searchApiKey={searchApiKey} setSearchApiKey={handleSetSearchApiKey}
                  searchHasSavedKey={searchHasSavedKey(searchProvider)}
                  savedModels={(bs && bs.savedModels) || []}
                  activeModelId={bs && bs.activeModelId}
                  onSaveModel={(m) => bridge.available && bridge.models.saveModel(m)}
                  onDeleteModel={(m) => { if (bridge.available) bridge.models.deleteModel(m.id); }}
                  onSetActiveModel={(id) => bridge.available && bridge.models.setActiveModel(id)}
                  onSaveSearchConfig={handleSaveSearchConfig}
                  onConfirmSearchConfig={handleConfirmSearchConfig}
                  onMemoryEnabledChange={handleSetMemoryEnabled}
                  onPetEnabledChange={handleSetPetEnabled}
                  searchNeedsRestart={searchNeedsRestart}
                  languageNeedsRestart={languageNeedsRestart}
                  bs={bs}
                  t={t}
                  sidebarDateGrouping={sidebarDateGrouping}
                  onSidebarDateGroupingChange={handleSetSidebarDateGrouping}
                  updateFocusTick={settingsUpdateFocusTick}
                  initialSection={settingsInitialSection}
                  onCloseSettings={() => navigateFromScheduledRun(settingsReturnViewRef.current || 'chat')}
                />
              </SettingsErrorBoundary>
            )}
            {currentView === 'toolStore' && <ToolStoreView theme={activeTheme} t={t} onNewChat={handleNewChat} />}
            {currentView === 'cardpool' && <CardPoolView theme={activeTheme} t={t} bs={bs} onEquipped={() => setCurrentView('chat')} onAICreate={startAICard} initialMyOnly={poolMyOnly} />}
            {currentView === 'chat' && <ChatView theme={activeTheme} t={t} bs={bs} prefill={chatPrefill} focusComposerTick={petFocusComposerTick} onPrefillConsumed={() => setChatPrefill('')} onOpenEditor={(initial) => setPersonaEditor({ initial })} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => openSettingsSection('general')} onGotoModelSettings={() => openSettingsSection('model')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} codeModeAvailable={codexAcpSupported} onSwitchHomeMode={handleSwitchHomeMode} />}
            {codexAcpSupported && currentView === 'codex' && (
              <CodexAcpView
                theme={activeTheme}
                t={t}
                sessions={codexSessions}
                activeId={activeCodexId}
                draftEpoch={codexDraftEpoch}
                onActiveSessionChange={updateActiveCodexSession}
                onSessionsChange={setCodexSessions}
                onSwitchHomeMode={handleSwitchHomeMode}
                onOpenSettingsSection={openSettingsSection}
                bs={bs}
                onGotoModelSettings={() => openSettingsSection('model')}
                onGotoSettings={() => openSettingsSection('general')}
                onGotoTools={() => navigateFromScheduledRun('toolStore')}
              />
            )}
            {SCHEDULED_TASKS_ENTRY_ENABLED && currentView === 'scheduled' && (
              bs && bs.scheduledRunContext ? (
                <ChatView theme={activeTheme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={(initial) => setPersonaEditor({ initial })} justInstalledTool={justInstalledTool} setJustInstalledTool={setJustInstalledTool} onGotoSettings={() => openSettingsSection('general')} onGotoModelSettings={() => openSettingsSection('model')} onGotoTools={() => navigateFromScheduledRun('toolStore')} onBackScheduledRun={() => navigateFromScheduledRun('scheduled')} />
              ) : (
                <ScheduledTasksView theme={activeTheme} t={t} onOpenChat={() => setCurrentView('chat')} onGotoModelSettings={() => openSettingsSection('model')} />
              )
            )}
            {/* 草稿态(无 session)也渲染挂件,但强制空态——让欢迎页保留「＋加持卡牌」入口。
                点它跳卡牌池,选卡时 equipPersona 会先物化 session(lazy session)。 */}
            {(currentView === 'chat' || (currentView === 'scheduled' && bs && bs.scheduledRunContext)) && bs && (
              <Lanyard persona={bs.activeSessionId ? (bs.activePersona || null) : null} isDark={activeTheme === 'dark'} t={t}
                onRemove={() => bridge.available && bridge.personas.unequipPersona()}
                onOpenPicker={() => navigateFromScheduledRun('cardpool', () => setPoolMyOnly(false))} />
            )}
            {currentView === 'search' && (
              <SearchView
                theme={activeTheme} history={allSidebarTasks} t={t} language={language}
                archived={(bs && bs.archivedSessions) || []}
                showArchived={searchShowArchived}
                onShowArchivedConsumed={() => setSearchShowArchived(false)}
                onSelect={handleSwitchSession}
                onOpenCodex={handleSwitchCodexSession}
                onOpenScheduledRun={handleOpenScheduledRunShortcut}
                onRename={handleRenameSession}
                onDelete={handleDeleteSession}
                onTogglePinned={handleToggleSessionPinned}
                onOpenFolder={can('externalSystemOpen') ? ((id) => bridge.artifacts.revealSessionFolder && bridge.artifacts.revealSessionFolder(id)) : undefined}
                onArchive={handleArchiveSession}
                onArchiveMany={handleBatchArchiveSessions}
                onDeleteMany={handleBatchDeleteSessions}
                onRestoreArchived={handleRestoreArchivedSession}
                onRestoreMany={handleBatchRestoreArchived}
              />
            )}
            {currentView === 'outputs' && <KnowledgeView theme={activeTheme} t={t} mode="outputs" />}
            {currentView === 'knowledge' && <KnowledgeView theme={activeTheme} t={t} />}

            {can('webAccessAdmin') && webAccessOpen && (
              <WebAccessModal theme={activeTheme} bs={bs} t={t} onClose={() => setWebAccessOpen(false)} />
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

            {/* API Key 拦截遮罩 —— 云端模型未配 key 时只盖住聊天界面,强制先配置。
                根因:此前前后端都无 key gate,空 key 打云端 → 401 静默无回应。
                设置页必须保持可操作,否则“去配置”后遮罩仍在,用户反而无法录入 Key。
                条件:credential_state 为 missing 或 unavailable 且非本地模型。本地 vLLM
                和 loopback OpenAI-compatible 端点允许无鉴权。unavailable 同样需拦截:macOS 上用户在 Keychain
                授权弹窗点"拒绝"时 credential_state 变 unavailable(见 prefs.rs:785),
                此时不盖遮罩用户仍可发消息 → 命中 Keychain 错误,与 missing 同等后果。 */}
            {shouldShowApiKeyGate(bs, currentView, bridge.available) && (
              <div className="fixed inset-0 z-[57] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}>
                <div className="w-full max-w-[400px] rounded-2xl p-6 ts-modal-in"
                     style={{ background: activeTheme === 'dark' ? '#1E1F20' : '#FFFFFF', color: activeTheme === 'dark' ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
                  <div className="flex items-center gap-2 mb-3">
                    <PinvouLogo className="h-[22px] w-[22px] select-none" />
                    <div className="text-[17px] font-semibold">{t.apiKeyGateTitle}</div>
                  </div>
                  <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.apiKeyGateDesc}</div>
                  <div className="flex justify-end">
                    <button onClick={() => openSettingsSection('model')}
                      className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.apiKeyGateBtn}</button>
                  </div>
                </div>
              </div>
            )}

            {/* MegaCube(GB10) 本地大模型一键引导 —— 全局首屏弹窗;引导中禁止背景关窗 */}
            {can('localModelSetup') && bs && bs.vllmSetup && bs.vllmSetup.eligible && !bs.vllmSetupDismissed && (
              <div className="fixed inset-0 z-[56] flex items-center justify-center p-6" style={{ background: 'rgba(0,0,0,.5)' }}
                   onClick={() => { if (!bs.vllmBootstrapping) bridge.vllm.dismissVllmSetup(); }}>
                <div className="w-full max-w-[440px] rounded-2xl p-6 ts-modal-in" onClick={(e) => e.stopPropagation()}
                     style={{ background: activeTheme === 'dark' ? '#1E1F20' : '#FFFFFF', color: activeTheme === 'dark' ? '#E3E3E3' : '#1F1F1F', boxShadow: '0 12px 48px rgba(0,0,0,.35)' }}>
                  <div className="flex items-center gap-2 mb-3">
                    <PinvouLogo className="h-[22px] w-[22px] select-none" />
                    <div className="text-[17px] font-semibold">{vllmDeclineConfirm && !bs.vllmBootstrapping && !bs.vllmBootstrapDone && !bs.vllmBootstrapError ? t.vllmDeclineTitle : t.vllmSetupTitle}</div>
                  </div>
                  {bs.vllmBootstrapping ? (
                    <VllmSetupProgress phase={bs.vllmSetupPhase} attempt={bs.vllmSetupAttempt} isDark={activeTheme === 'dark'} t={t} />
                  ) : bs.vllmBootstrapDone ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4">{t.vllmSetupDone}</div>
                      <div className="flex justify-end">
                        <button onClick={() => bridge.available && bridge.updater.restartApp()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.restartNow}</button>
                      </div>
                    </div>
                  ) : bs.vllmBootstrapError ? (
                    <div>
                      <div className="text-[14px] font-medium mb-1" style={{ color: '#E5484D' }}>{t.vllmSetupFailed}</div>
                      <div className="text-[13px] leading-relaxed mb-4 break-words" style={{ opacity: .75 }}>{bs.vllmBootstrapError}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => bridge.vllm.dismissVllmSetup()}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                        <button onClick={() => bridge.vllm.bootstrapLocalVllm()}
                          className="h-9 px-4 rounded-lg text-[14px] font-medium text-white" style={{ background: '#0A84FF' }}>{t.vllmSetupRetry}</button>
                      </div>
                    </div>
                  ) : vllmDeclineConfirm ? (
                    <div>
                      <div className="text-[14px] leading-relaxed mb-4" style={{ opacity: .85 }}>{t.vllmDeclineDesc}</div>
                      <div className="flex justify-end gap-2">
                        <button onClick={() => setVllmDeclineConfirm(false)}
                          className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmDeclineReconsider}</button>
                        <button onClick={() => { setVllmDeclineConfirm(false); bridge.vllm.declineVllmSetup(); }}
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
                          <button onClick={() => bridge.vllm.dismissVllmSetup()}
                            className="h-9 px-4 rounded-lg text-[14px]" style={{ background: activeTheme === 'dark' ? 'rgba(255,255,255,.08)' : 'rgba(0,0,0,.06)' }}>{t.vllmSetupSkip}</button>
                          <button onClick={() => bridge.vllm.bootstrapLocalVllm()}
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
                   onClick={() => { if (!bs.pinvouModal.loading) bridge.interaction.dismissPinvouReview(); }}>
                {/* loading 期间禁止背景点击关窗:召唤(直连 vLLM,5-30s)仍在后台跑、守卫仍 held,
                    点背景误关会表现为"闪一下没反应、要等一会才能再点"。锁住后 spinner 全程可见,
                    出结果/错误后才可点背景关。 */}
                <div className="relative w-full max-w-[720px] overflow-hidden bg-white dark:bg-[#1C1C1E] rounded-[20px] shadow-[0_20px_60px_rgba(0,0,0,0.28)] ts-modal-in"
                     onClick={(e) => e.stopPropagation()}
                     style={{ fontFamily:'-apple-system, BlinkMacSystemFont, "SF Pro Text", "PingFang SC", "Microsoft YaHei", sans-serif' }}>
                  {/* 关闭按钮：所有状态(含 loading)常驻;loading 时点它=取消等待并关窗,in-flight 结果由守卫丢弃 */}
                  <button onClick={() => bridge.available && bridge.interaction.dismissPinvouReview()} aria-label={t.pvSkip}
                    className="absolute top-3.5 right-3.5 z-10 w-7 h-7 flex items-center justify-center rounded-full bg-black/[0.06] dark:bg-white/10 text-[#8E8E93] hover:bg-black/10 dark:hover:bg-white/15 active:scale-90 transition-colors">
                    <X size={16} />
                  </button>
                  <div className="max-h-[90vh] overflow-y-auto custom-scrollbar px-5 pt-5 pb-6">
                    <PinvouSummonCard item={bs.pinvouModal} theme={activeTheme} t={t} isLocal={activeModelIsLocal(bs)} />
                  </div>
                </div>
              </div>
            )}

          </div>
          </div>

          {isCompactShell && (
            <MobileTabBar theme={activeTheme} tabs={[
              { key: 'chat', label: t.currentChat, icon: <MessageSquare size={18} />,
                active: currentView === 'chat' || !!(currentView === 'scheduled' && bs && bs.scheduledRunContext),
                onClick: () => mobileNavigate('chat') },
              { key: 'cardpool', label: t.cardPool, icon: <Layers size={18} />,
                active: currentView === 'cardpool', onClick: () => mobileNavigate('cardpool', () => setPoolMyOnly(false)) },
              { key: 'monitor', label: t.monitor, icon: <BarChart2 size={18} />,
                active: currentView === 'monitor',
                onClick: () => mobileNavigate('monitor', () => {
                  const liveBridge = window.TauriBridge || bridge;
                  if (liveBridge && typeof liveBridge.startMonitorPolling === 'function') liveBridge.startMonitorPolling();
                }) },
              { key: 'more', label: t.mobileMore, icon: <MoreHorizontal size={18} />,
                active: mobileMoreActive, dot: hasUpdate || scheduledUnread,
                onClick: () => setMobileMoreOpen(true) },
            ]} />
          )}

          {isCompactShell && mobileMoreOpen && (
            <MobileMoreSheet theme={activeTheme} title={t.mobileMore} onClose={() => setMobileMoreOpen(false)} items={[
              { key: 'search', label: t.searchChats, icon: <Search size={18} />,
                active: currentView === 'search', onClick: () => mobileNavigate('search') },
              ...(SCHEDULED_TASKS_ENTRY_ENABLED ? [{ key: 'scheduled', label: t.scheduledPlans, icon: <Clock size={18} />,
                active: currentView === 'scheduled', dot: scheduledUnread,
                onClick: () => mobileNavigate('scheduled') }] : []),
              { key: 'outputs', label: t.outputs, icon: <Package size={18} />,
                active: currentView === 'outputs', onClick: () => mobileNavigate('outputs') },
              { key: 'knowledge', label: t.knowledge, icon: <BookOpen size={18} />,
                active: currentView === 'knowledge', onClick: () => mobileNavigate('knowledge') },
              { key: 'toolStore', label: t.toolStore, icon: <Puzzle size={18} />,
                active: currentView === 'toolStore', onClick: () => mobileNavigate('toolStore') },
              { key: 'settings', label: t.settings, icon: <Settings size={18} />,
                active: currentView === 'settings', dot: hasUpdate, onClick: () => mobileNavigate('settings') },
            ]} />
          )}

          <UpdateNoticeButton
            theme={activeTheme}
            bs={bs}
            t={t}
            onShowChangelog={() => {
              setSettingsInitialSection('update');
              setCurrentView('settings');
              setSettingsUpdateFocusTick(v => v + 1);
            }}
          />
        </div>
      );
    };

    // ==========================================
    // 长按撕离:按住 ~350ms 不动 → onPickUp(info)(DOM avatar 浮起跟手 + begin_detach_drag 原生判落点);
    // 长按达成前移动 >10px = 视为滚动/取消;长按达成后吞掉随之而来的 click(避免又切视图);
    // 按在内部按钮/输入框上不起手(让它们自理)。按下即禁选,防止长按选中下方文字。
    window.__PINVOU_STARTUP__.mark('react:create_root_start');
    const root = createRoot(document.getElementById('root'));
    window.__PINVOU_STARTUP__.mark('react:create_root_done');
    const __q = new URLSearchParams(window.location.search);
    if (__q.get('detached') === '1') {
      window.__PINVOU_DETACHED__ = true;
      root.render(<DetachedShell kind={__q.get('kind') || 'monitor'} id={__q.get('id') || ''} />);
    } else {
      window.__PINVOU_STARTUP__.mark('react:render_call');
      root.render(<App />);
    }
