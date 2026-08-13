import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, ArrowLeft, BarChart2, BookOpen, Brain, Briefcase, Check, ChevronDown, ChevronRight, ClipboardList, Copy, Edit2, FileText, ImageIcon, Mic, Monitor, Package, Palette, Paperclip, Presentation, Send, Sparkles, StopCircle, Trash2, Upload, X, Zap } from '../../components/icons.jsx';
import { bridge, activeModelIsLocal } from '../../hooks/useBridge.js';
import { can, isWeb } from '../../shared/platform.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { ArtifactsPanel } from '../artifacts/ArtifactsPanel.jsx';
import { AppIcon, DEPT_ORDER, deptColor, deptLabelFor, personaText } from '../personas/Personas.jsx';
import { ComposerModelSelector, ComposerToolMenu } from '../settings/SettingsView.jsx';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import { PinvouLogo } from '../../components/PinvouLogo.jsx';
import { ArtifactCard, localizeTool, tsToolsData } from '../tools/tool-common.jsx';
import { CarefulBlockedCard, PlanCard, PlanStuckCard, ToolCard, UserInputCard, cardBtnCls } from '../tools/tool-renderers.jsx';
import {
  ConversationActivityIndicator,
  ConversationTimeline,
} from '../conversation/ConversationTimeline.jsx';
import { HomeModeSwitcher } from '../conversation/HomeModeSwitcher.jsx';
import {
  conversationItemsForMode,
  projectDeepSeekConversation,
} from '../conversation/deepseek-conversation.js';
import {
  captureConversationScrollPosition,
  isFetchTool,
  isNearConversationBottom,
  isSearchTool,
  restoreConversationScrollPosition,
} from '../conversation/conversation-model.js';
import { AttachmentChips } from '../attachments/AttachmentChips.jsx';
import { AttachmentDropOverlay } from '../attachments/AttachmentDropOverlay.jsx';
import { ConversationAttachmentBubble } from '../attachments/ConversationAttachmentBubble.jsx';
import { splitAttachmentLine } from '../attachments/attachment-message.js';
import { SubagentTranscriptPanel } from '../multiagent/SubagentTranscriptPanel.jsx';
import { CHAT_INPUT_MAX_LENGTH, constrainChatInput } from './chat-input-limit.js';
import { AssistantMessageActions, AssistantMessageFooter } from '../conversation/AssistantMessageActions.jsx';
import {
  assistantItemCopyText,
  copyClipboardText,
  fallbackCopyText,
  readClipboardText,
} from '../conversation/message-clipboard.js';
import {
  extractBalancedJson,
  parseJsonChain,
  parseLooseJson,
} from '../conversation/structured-assistant-content.js';
import {
  FLOATING_VOICE_CLICK_SUPPRESSION_MS,
  canStartFloatingVoiceDrag,
  clearFloatingVoiceDragClick,
  consumeFloatingVoiceDragClick,
  createFloatingVoiceDragSession,
  finishFloatingVoiceDrag,
  moveFloatingVoiceDrag,
} from './floating-voice-drag.mjs';
import {
  createPinvouModeScopeKey,
  loadPinvouModeState,
  reducePinvouModeState,
  savePinvouModeState,
} from './pinvou-mode-state.js';
import { createDesignChange, createDesignChangeScopeKey, reduceScopedDesignChanges, uniqueDesignChanges } from './design-changes.js';
import { createVisualPosterMessageMeta, shouldUseVisualPosterScene } from './visual-poster-scene.js';
import {
  createDataVisualizationMessageMeta,
  createDocumentWritingMessageMeta,
  createPersonalWorkbenchMessageMeta,
  PERSONAL_WORKBENCH_SCENE_KEY,
  shouldUseDataVisualizationScene,
  shouldUseDocumentWritingScene,
  shouldUsePersonalWorkbenchScene,
} from './work-scene-routes.js';
import {
  PERSONAL_WORKBENCH_TEMPLATES,
  findPersonalWorkbenchTemplateDraft,
  getPersonalWorkbenchTemplate,
  getPersonalWorkbenchTemplateById,
  isPersonalWorkbenchTemplateDraftForTemplate,
} from './personal-workbench-scene.js';
import { canPrepareSceneCapabilities, prepareSceneCapabilities, requiredCapabilitiesForMeta } from './scene-capabilities.js';
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  COMPOSER_ICON_BUTTON_CLASS,
  ComposerKbSelector,
  ComposerModeChip,
} from './composer-controls.jsx';

const UNIFIED_CONVERSATION_UI_KEY = 'pinvou_conversation_ui_v2';
const MULTI_AGENT_ENABLED = can('multiAgent');

function unifiedConversationUiEnabled() {
  try {
    return localStorage.getItem(UNIFIED_CONVERSATION_UI_KEY) !== 'false';
  } catch {
    return true;
  }
}

const WORK_MODE_SUBTABS = [
  { key: PERSONAL_WORKBENCH_SCENE_KEY, labelKey: 'personalWorkbench', Icon: Briefcase },
  { key: 'document-writing', labelKey: 'documentWriting', Icon: FileText },
];

const DESIGN_MODE_SUBTABS = [
  { key: 'poster', labelKey: 'poster', Icon: ImageIcon },
  { key: 'data-visualization', labelKey: 'dataVisualization', Icon: BarChart2 },
  { key: 'ppt', labelKey: 'pptDesign', Icon: Presentation, disabled: true, disabledReasonKey: 'pptUnavailable' },
];

function localizeSceneTabs(items, copy) {
  return items.map(item => ({
    ...item,
    label: copy[item.labelKey],
    disabledReason: item.disabledReasonKey ? copy[item.disabledReasonKey] : undefined,
  }));
}

function pinvouSceneDisplay(scene, copy) {
  switch (scene) {
    case 'work:document-writing':
      return { label: copy.documentWriting, Icon: FileText };
    case 'work:personal-workbench':
      return { label: copy.personalWorkbench, Icon: Briefcase };
    case 'design:poster':
      return { label: copy.poster, Icon: ImageIcon };
    case 'design:data-visualization':
      return { label: copy.dataVisualization, Icon: BarChart2 };
    default:
      return null;
  }
}

const openChatExternalUrl = (url) => {
  if (isWeb) {
    const opened = window.open(url, '_blank', 'noopener,noreferrer');
    if (opened) opened.opener = null;
    return;
  }
  invokeTauri('open_user_external_url', { url }).catch(() => {});
};

const ToolWelcomeCard = ({ toolId, theme, t, onSend }) => {
      const isDark = theme === 'dark';
      const [hovered, setHovered] = useState(null);
      const tool = localizeTool(tsToolsData.find(item => item.backendId === toolId), t);
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
                  {t.uiChat.ready}
                </div>
              </div>
            </div>
            <div className="p-5">
              <p className={`leading-relaxed text-[15px] ${isDark ? 'text-slate-300' : 'text-slate-600'}`}>
                {tool.desc.split('。')[0]}。{t.uiChat.naturalQuestion}
              </p>
              <div className="flex items-center my-5">
                <div className={`flex-grow h-px ${isDark ? 'bg-gradient-to-r from-transparent via-[#3A3A3C] to-transparent' : 'bg-gradient-to-r from-transparent via-slate-200 to-transparent'}`}></div>
                <span className={`px-4 text-[11px] uppercase tracking-wider font-semibold flex items-center gap-1.5 ${isDark ? 'text-slate-500' : 'text-slate-400'}`}>
                  <Sparkles size={13} />
                  <span>{t.uiChat.askMe}</span>
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
    // 检索注入相关片段(commands::chat)。草稿态选集会经 bridge.knowledge.mountCollection 先物化 session。
    // 附件入口。桌面端与旧桌面实例保持原有单入口(直开各自的文件选择器);
    // 仅当桌面实例通过能力协商声明支持浏览器上传(deviceFileUpload)时,才展开
    // "从此设备上传 / 从桌面实例选择"双入口菜单。能力在点击时评估,不依赖
    // 能力快照到达时机的渲染竞态。
    const ComposerAttachButton = ({ t, compact }) => {
      const [open, setOpen] = useState(false);
      const triggerRef = useRef(null);
      const fileInputRef = useRef(null);
      function onTriggerClick() {
        if (!bridge.available) return;
        if (can('deviceFileUpload')) setOpen(v => !v);
        else bridge.attachments.pickAndAttach();
      }
      function pickFromDevice() {
        setOpen(false);
        if (fileInputRef.current) fileInputRef.current.click();
      }
      function pickFromHost() {
        setOpen(false);
        bridge.attachments.pickAndAttach();
      }
      function onFilesChosen(event) {
        const files = event.target.files;
        if (files && files.length && bridge.available) bridge.attachments.uploadDeviceFiles(files);
        event.target.value = '';
      }
      const entryCls = 'w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group';
      return (
        <div className="relative">
          <button ref={triggerRef} onClick={onTriggerClick} title={t.attachAdd} className={COMPOSER_ICON_BUTTON_CLASS}>
            <Paperclip size={18} />
          </button>
          <input ref={fileInputRef} type="file" multiple className="hidden" data-testid="device-file-input" onChange={onFilesChosen} />
          <ComposerPopover open={open} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
            desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-56 bg-white/95 dark:bg-[#1E1E20]/95 backdrop-blur-xl border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
            <button onClick={pickFromDevice} className={entryCls}>
              <Upload size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
              {t.attachFromDevice}
            </button>
            <button onClick={pickFromHost} className={entryCls}>
              <Monitor size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
              {t.attachFromHost}
            </button>
          </ComposerPopover>
        </div>
      );
    };

    const SubModePicker = ({ isDark, value, onChange, items, icons, testId = 'mode-subtab-picker', comingSoonLabel = '' }) => {
      const trackRef = useRef(null);
      const buttonRefs = useRef({});
      const [indicator, setIndicator] = useState({ left: 0, width: 20, ready: false });

      const updateIndicator = useCallback(() => {
        const track = trackRef.current;
        const button = buttonRefs.current[value];
        if (!track || !button) {
          setIndicator((prev) => (prev.ready ? { left: 0, width: 20, ready: false } : prev));
          return;
        }
        const trackRect = track.getBoundingClientRect();
        const buttonRect = button.getBoundingClientRect();
        const width = Math.min(28, Math.max(20, buttonRect.width * 0.32));
        setIndicator({
          left: buttonRect.left - trackRect.left + (buttonRect.width - width) / 2,
          width,
          ready: true,
        });
      }, [value]);

      useLayoutEffect(() => {
        updateIndicator();
        window.addEventListener('resize', updateIndicator);
        return () => window.removeEventListener('resize', updateIndicator);
      }, [items, updateIndicator]);

      return (
        <div
          data-testid={testId}
          className="mb-1.5 flex justify-center px-1"
        >
          <div ref={trackRef} className="relative inline-flex max-w-full items-center justify-center gap-5 overflow-x-auto px-2 py-1">
            <span
              aria-hidden="true"
              className={`absolute bottom-0 h-0.5 rounded-full transition-all duration-200 ease-out ${isDark ? 'bg-[#F5F5F7]' : 'bg-[#1D1D1F]'}`}
              style={{
                left: `${indicator.left}px`,
                width: `${indicator.width}px`,
                opacity: indicator.ready ? 1 : 0,
              }}
            />
            {items.map((item) => {
              const selected = value === item.key;
              const disabled = !!item.disabled;
              const icon = icons && icons[item.key];
              const ItemIcon = item.Icon;
              return (
                <button
                  ref={(node) => { buttonRefs.current[item.key] = node; }}
                  key={item.key}
                  type="button"
                  data-testid={`${testId}-option-${item.key}`}
                  title={disabled ? (item.disabledReason || comingSoonLabel) : item.label}
                  aria-pressed={selected}
                  aria-disabled={disabled ? 'true' : undefined}
                  disabled={disabled}
                  onClick={() => {
                    if (disabled) return;
                    if (onChange) onChange(item.key);
                  }}
                  className={`relative flex h-7 min-w-0 items-center justify-center gap-1.5 px-0.5 text-[13px] font-medium transition-colors duration-200 ${
                    disabled
                      ? isDark
                        ? 'cursor-not-allowed text-[#5F6368] opacity-60'
                        : 'cursor-not-allowed text-[#A0A4AA] opacity-65'
                      : selected
                      ? isDark
                        ? 'text-[#F5F5F7]'
                        : 'text-[#1D1D1F]'
                      : isDark
                        ? 'text-[#8E8E93] hover:text-[#F5F5F7]'
                        : 'text-[#8E8E93] hover:text-[#1D1D1F]'
                  }`}
                >
                  {ItemIcon && (
                    <ItemIcon size={15} className="shrink-0" />
                  )}
                  {icon && (
                    <span className={`flex h-[18px] w-[18px] shrink-0 items-center justify-center rounded-[5px] bg-white ${
                      isDark ? 'shadow-[0_0_0_1px_rgba(255,255,255,.14)]' : 'ring-1 ring-black/[0.06]'
                    }`}>
                      <img src={icon} alt="" className="h-[13px] w-[13px] object-contain" />
                    </span>
                  )}
                  <span className="min-w-0 truncate">{item.label}</span>
                </button>
              );
            })}
          </div>
        </div>
      );
    };

    const PersonalWorkbenchTemplatePicker = ({ isDark, selectedIndex, onSelect, templates }) => {
      return (
        <div
          data-testid="personal-workbench-template-picker"
          className="mb-2 flex justify-center px-1"
        >
          <div className="flex max-w-full items-center gap-2 overflow-x-auto px-1 py-1">
            {templates.map((template, index) => {
              const selected = selectedIndex === index;
              return (
                <button
                  key={template.title}
                  type="button"
                  data-testid={`personal-workbench-template-${index}`}
                  aria-pressed={selected}
                  onClick={() => onSelect(index)}
                  className={`shrink-0 rounded-full px-3 py-1.5 text-[13px] font-medium transition-colors ${
                    selected
                      ? isDark
                        ? 'bg-[#F5F5F7] text-[#1D1D1F]'
                        : 'bg-[#1D1D1F] text-white'
                      : isDark
                        ? 'bg-[#2A2B2D] text-[#C7C7CC] hover:bg-[#333537] hover:text-white'
                        : 'bg-[#EEF0F2] text-[#3C4043] hover:bg-[#E3E5E8] hover:text-[#1D1D1F]'
                  }`}
                >
                  {template.title}
                </button>
              );
            })}
          </div>
        </div>
      );
    };

    const SceneModeTag = ({ isDark, scene, onClear, clearLabel }) => {
      if (!scene) return null;
      const SceneIcon = scene.Icon || Sparkles;
      return (
        <div className="mb-2 flex flex-wrap justify-start gap-2 px-1" data-testid="pinvou-scene-tag">
          <div className={`inline-flex h-8 items-center gap-2 rounded-[14px] px-3 text-[13px] font-semibold shadow-sm ${
            isDark
              ? 'bg-[#2A2B2D] text-[#F5F5F7] ring-1 ring-white/10'
              : 'bg-[#F5F5F7] text-[#1D1D1F] ring-1 ring-black/[0.06]'
          }`}>
            <SceneIcon size={15} className="shrink-0" />
            <span>{scene.label}</span>
            {onClear && (
              <button
                type="button"
                data-testid="pinvou-scene-tag-clear"
                aria-label={clearLabel}
                title={clearLabel}
                onClick={(event) => {
                  event.preventDefault();
                  event.stopPropagation();
                  onClear();
                }}
                className={`-mr-1 flex h-5 w-5 items-center justify-center rounded-full transition-colors ${
                  isDark ? 'text-[#C7C7CC] hover:bg-white/10 hover:text-white' : 'text-[#5F6368] hover:bg-black/10 hover:text-[#1D1D1F]'
                }`}
              >
                <X size={13} />
              </button>
            )}
          </div>
        </div>
      );
    };

    const ChatView = ({ theme, t, bs, prefill, focusComposerTick = 0, onPrefillConsumed, onOpenEditor, justInstalledTool, setJustInstalledTool, onGotoSettings, onGotoModelSettings, onGotoTools, onBackScheduledRun, codeModeAvailable = false, onSwitchHomeMode }) => {
      const isDark = theme === 'dark';
      const chatCopy = t.uiChat;
      const chatViewCopy = t.uiChatView;
      const sceneCopy = chatCopy.sceneModes;
      const workModeSubtabs = localizeSceneTabs(WORK_MODE_SUBTABS, sceneCopy);
      const designModeSubtabs = localizeSceneTabs(DESIGN_MODE_SUBTABS, sceneCopy);
      const canInstallLocalAsr = can('localModelSetup') && can('dependencyInstall');
      const initialInput = constrainChatInput(
        bridge.available && bridge.chat && bridge.chat.getComposerDraft
          ? bridge.chat.getComposerDraft()
          : ((bs && bs.composerDraft) || '')
      );
      const [inputText, setInputTextState] = useState(initialInput.text);
      const [inputLimitReached, setInputLimitReached] = useState(initialInput.limitReached);
      const inputTextRef = useRef(initialInput.text);
      const setInputText = useCallback((valueOrUpdater) => {
        const rawValue = typeof valueOrUpdater === 'function'
          ? valueOrUpdater(inputTextRef.current)
          : valueOrUpdater;
        const constrained = constrainChatInput(rawValue);
        inputTextRef.current = constrained.text;
        setInputTextState(constrained.text);
        setInputLimitReached(constrained.limitReached);
        if (bridge.available && bridge.chat && bridge.chat.setComposerDraft) {
          bridge.chat.setComposerDraft(constrained.text);
        }
      }, []);
      const [artifactsOpen, setArtifactsOpen] = useState(false);
      const [artifactsFullscreen, setArtifactsFullscreen] = useState(false);
      const [activeArtifactPath, setActiveArtifactPath] = useState(null);
      const initialPinvouModeScope = createPinvouModeScopeKey(bs && bs.activeSessionId);
      const [pinvouModeState, setPinvouModeState] = useState(() => loadPinvouModeState(undefined, initialPinvouModeScope));
      const pinvouModeScopeRef = useRef(initialPinvouModeScope);
      const pinvouModeStateRef = useRef(pinvouModeState);
      const pendingModeScopeMigrationRef = useRef(null);
      const [personalWorkbenchTemplateId, setPersonalWorkbenchTemplateId] = useState(null);
      const personalWorkbenchTemplateIdRef = useRef(null);
      const [selectedDesignElement, setSelectedDesignElement] = useState(null);
      const [designChangesByScope, setDesignChangesByScope] = useState({});
      const [designCommand, setDesignCommand] = useState(null);
      const [designAiState, setDesignAiState] = useState({ text: '', status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 });
      const [sceneCapabilityStatus, setSceneCapabilityStatus] = useState(null);
      const designAiSessionRef = useRef(null);
      const previousArtifactCountRef = useRef(0);
      const updateDesignAiState = useCallback((valueOrUpdater) => {
        setDesignAiState((prev) => {
          const next = typeof valueOrUpdater === 'function' ? valueOrUpdater(prev) : valueOrUpdater;
          const merged = { text: '', status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0, ...(next || {}) };
          if (typeof window !== 'undefined') {
            if (merged.status !== 'idle' || merged.text || merged.lastPrompt) {
              window.__PINVOU_DESIGN_AI_STATE__ = merged;
            } else {
              window.__PINVOU_DESIGN_AI_STATE__ = null;
            }
          }
          return merged;
        });
      }, []);
      // ── 产物分栏:宽屏(≥900)并排可拖、窄屏回退覆盖抽屉 ──
      const ART_MIN = 360, CHAT_MIN = 360, ART_MAX_RATIO = 0.65, ART_DEFAULT_RATIO = 0.45, ART_NARROW = 900;
      const clampArtifactWidth = (w, rootW) => {
        const max = Math.max(ART_MIN, Math.min(Math.round(rootW * ART_MAX_RATIO), rootW - CHAT_MIN));
        return Math.max(ART_MIN, Math.min(Math.round(w), max));
      };
      const rootRef = useRef(null);
      const artColRef = useRef(null);
      const [isWide, setIsWide] = useState(() => (typeof window !== 'undefined' ? window.innerWidth : 1200) >= ART_NARROW);
      const [artifactW, setArtifactW] = useState(() => {
        const s = parseInt(localStorage.getItem('pinvou_artifactW') || '', 10);
        const w = (typeof window !== 'undefined' ? window.innerWidth : 1200);
        const next = Number.isFinite(s) && s >= ART_MIN ? s : Math.round(w * ART_DEFAULT_RATIO);
        return clampArtifactWidth(next, w);
      });
      useEffect(() => {
        const onResize = () => {
          const rootW = rootRef.current ? rootRef.current.getBoundingClientRect().width : window.innerWidth;
          setIsWide(window.innerWidth >= ART_NARROW);
          setArtifactW(w => clampArtifactWidth(w, rootW));
        };
        onResize();                 // 挂载即测一次(maximized 启动时 init 可能读到小值,这里校正)
        const t = setTimeout(onResize, 300);  // 再补一发,防 webview 首帧尺寸未定
        window.addEventListener('resize', onResize);
        return () => { clearTimeout(t); window.removeEventListener('resize', onResize); };
      }, []);
      const startArtifactDrag = (e) => {
        e.preventDefault();
        const rect = rootRef.current ? rootRef.current.getBoundingClientRect() : { right: window.innerWidth, width: window.innerWidth };
        const max = Math.max(ART_MIN, Math.min(Math.round(rect.width * ART_MAX_RATIO), rect.width - CHAT_MIN));
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
        const rootW = rootRef.current ? rootRef.current.getBoundingClientRect().width : window.innerWidth;
        const w = clampArtifactWidth(Math.round(rootW * ART_DEFAULT_RATIO), rootW);
        setArtifactW(w); localStorage.setItem('pinvou_artifactW', String(w));
      };
      const scrollRef = useRef(null);
      const autoScrollRef = useRef(true);
      const lastScrollTopRef = useRef(0);
      const subagentPanelScrollRef = useRef(null);
      const [showScrollBottom, setShowScrollBottom] = useState(false);
      const chatRootRef = useRef(null);
      const composerRef = useRef(null);
      const floatingVoiceRef = useRef(null);
      const voiceDragRef = useRef(null);
      const voiceDragClickResetRef = useRef(null);
      const [floatingVoicePos, setFloatingVoicePos] = useState(null);
      const [floatingVoicePressed, setFloatingVoicePressed] = useState(false);
      useEffect(() => {
        if (!focusComposerTick) return undefined;
        const timer = window.setTimeout(() => {
          if (composerRef.current) composerRef.current.focus();
        }, 80);
        return () => window.clearTimeout(timer);
      }, [focusComposerTick]);
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
      const activeSessionId = bs ? bs.activeSessionId : null;
      const busy = bs ? bs.busy : false;
      const activeModelLocal = activeModelIsLocal(bs);
      const hasMessages = chatItems.length > 0;
      const attachments = (bs && bs.attachments) || [];
      const attachmentDragActive = !!(bs && bs.attachmentDragActive);
      const formatAttachmentError = (error) => {
        const raw = String(error || '');
        if (/under sensitive system dir|crosses sensitive (dir|component)/i.test(raw)) {
          return t.attachProtectedLocation;
        }
        return '';
      };
      const queued = (bs && bs.queued) || []; // 排队待发消息（当前 session 生成中时积压）
      const ctxTokens = (bs && bs.tokens) || null; // {input, max}，chat:usage 每轮更新
      const ctxPct = ctxTokens && ctxTokens.max > 0 ? ctxTokens.input / ctxTokens.max : 0;
      const fmtCtxTok = (n) => n >= 1e6 ? (n / 1e6).toFixed(1) + 'M' : n >= 1e3 ? (n / 1e3).toFixed(1) + 'k' : String(n);
      const artifactItems = (bs && bs.artifacts) || [];
      const artifactCount = artifactItems.length;
      const latestArtifact = artifactItems[artifactItems.length - 1] || null;
      const conversationStarted = chatItems.some(item => item && item.type === 'user') || artifactCount > 0;
      const pinvouMode = pinvouModeState.mode;
      const workSubtab = pinvouModeState.workSubtab;
      const designSubtab = pinvouModeState.designSubtab;
      const designScopeKey = createDesignChangeScopeKey(activeSessionId, activeArtifactPath);
      const designChanges = designChangesByScope[designScopeKey] || [];
      const visibleDesignChanges = uniqueDesignChanges(designChanges);
      const reduceCurrentDesignChanges = useCallback((action) => {
        setDesignChangesByScope((prev) => reduceScopedDesignChanges(prev, designScopeKey, action));
      }, [designScopeKey]);
      const updatePinvouModeState = useCallback((action) => {
        setPinvouModeState((prev) => {
          const next = savePinvouModeState(
            reducePinvouModeState(prev, action),
            undefined,
            pinvouModeScopeRef.current,
          );
          pinvouModeStateRef.current = next;
          return next;
        });
      }, []);
      useEffect(() => {
        const nextScope = createPinvouModeScopeKey(activeSessionId);
        if (nextScope === pinvouModeScopeRef.current) return;

        const pending = pendingModeScopeMigrationRef.current;
        if (pending && activeSessionId) {
          const lastUser = [...chatItems].reverse().find((item) => item && item.type === 'user');
          if (!lastUser) return;
          pendingModeScopeMigrationRef.current = null;
          const lastUserText = String(lastUser.text || '').trim();
          if (
            !pending.text ||
            lastUserText === pending.text ||
            lastUserText.startsWith(`${pending.text}\n\n📎 `)
          ) {
            const migrated = savePinvouModeState(pending.state, undefined, nextScope);
            pinvouModeScopeRef.current = nextScope;
            pinvouModeStateRef.current = migrated;
            setPinvouModeState(migrated);
            return;
          }
        }

        const restored = loadPinvouModeState(undefined, nextScope);
        pinvouModeScopeRef.current = nextScope;
        pinvouModeStateRef.current = restored;
        setPinvouModeState(restored);
      }, [activeSessionId, chatItems.length]);
      useEffect(() => {
        setSelectedDesignElement(null);
        updatePinvouModeState({ type: 'set-selected-design-element', elementId: undefined });
      }, [designScopeKey, updatePinvouModeState]);
      const clearPersonalWorkbenchTemplateDraft = useCallback(() => {
        if (personalWorkbenchTemplateIdRef.current || findPersonalWorkbenchTemplateDraft(inputTextRef.current)) setInputText('');
        personalWorkbenchTemplateIdRef.current = null;
        setPersonalWorkbenchTemplateId(null);
      }, [setInputText]);
      const handlePinvouModeChange = useCallback((mode) => {
        updatePinvouModeState({ type: 'set-mode', mode });
        if (mode !== 'work') clearPersonalWorkbenchTemplateDraft();
        if (mode !== 'design') setSelectedDesignElement(null);
        if (mode === 'design' && artifactCount > 0) {
          if (latestArtifact && latestArtifact.path) setActiveArtifactPath(latestArtifact.path);
          setArtifactsOpen(true);
          setArtifactsFullscreen(true);
        }
      }, [artifactCount, clearPersonalWorkbenchTemplateDraft, latestArtifact, updatePinvouModeState]);
      const handleHomeModeChange = useCallback((mode) => {
        if (mode === 'code') {
          if (onSwitchHomeMode) onSwitchHomeMode(mode);
          return;
        }
        handlePinvouModeChange(mode);
      }, [handlePinvouModeChange, onSwitchHomeMode]);
      const handleWorkSubtabChange = useCallback((subtab) => {
        const nextSubtab = subtab === pinvouModeStateRef.current.workSubtab ? 'general' : subtab;
        if (nextSubtab !== PERSONAL_WORKBENCH_SCENE_KEY) clearPersonalWorkbenchTemplateDraft();
        updatePinvouModeState({
          type: 'set-work-subtab',
          subtab: nextSubtab,
        });
      }, [clearPersonalWorkbenchTemplateDraft, updatePinvouModeState]);
      const handleDesignSubtabChange = useCallback((subtab) => {
        updatePinvouModeState({
          type: 'set-design-subtab',
          subtab: subtab === pinvouModeStateRef.current.designSubtab ? 'general' : subtab,
        });
      }, [updatePinvouModeState]);
      const handleClearActiveScene = useCallback(() => {
        if (pinvouModeStateRef.current.mode === 'work') {
          clearPersonalWorkbenchTemplateDraft();
          updatePinvouModeState({ type: 'set-work-subtab', subtab: 'general' });
        } else if (pinvouModeStateRef.current.mode === 'design') {
          updatePinvouModeState({ type: 'set-design-subtab', subtab: 'general' });
        }
      }, [clearPersonalWorkbenchTemplateDraft, updatePinvouModeState]);
      const handlePersonalWorkbenchTemplateSelect = useCallback((index) => {
        const template = getPersonalWorkbenchTemplate(index);
        const normalized = template ? template.id : null;
        personalWorkbenchTemplateIdRef.current = normalized;
        setPersonalWorkbenchTemplateId(normalized);
        if (template) {
          setInputText(template.prompt);
          window.requestAnimationFrame(() => {
            if (composerRef.current) {
              composerRef.current.focus();
              composerRef.current.selectionStart = composerRef.current.value.length;
              composerRef.current.selectionEnd = composerRef.current.value.length;
            }
          });
        }
      }, [setInputText]);
      const handleComposerInputChange = useCallback((value) => {
        setInputText(value);
        const currentTemplate = getPersonalWorkbenchTemplateById(personalWorkbenchTemplateIdRef.current);
        if (!currentTemplate) return;
        if (!isPersonalWorkbenchTemplateDraftForTemplate(value, currentTemplate)) {
          personalWorkbenchTemplateIdRef.current = null;
          setPersonalWorkbenchTemplateId(null);
        }
      }, [setInputText]);
      const handleDesignRuntimeStatus = useCallback((status) => {
        updatePinvouModeState({ type: 'set-design-runtime-status', status });
      }, [updatePinvouModeState]);
      const handleDesignElementSelected = useCallback((element) => {
        updatePinvouModeState({ type: 'set-selected-design-element', elementId: element && element.id });
        setSelectedDesignElement(element || null);
      }, [updatePinvouModeState]);
      const handleApplyDesignChange = useCallback(({ type, property, oldValue, newValue }) => {
        if (!selectedDesignElement || !selectedDesignElement.selector) return;
        if (String(oldValue == null ? '' : oldValue) === String(newValue == null ? '' : newValue)) return;
        if (designChanges.some((change) => (
          change.selector === selectedDesignElement.selector &&
          change.type === type &&
          change.property === property &&
          change.oldValue === String(oldValue == null ? '' : oldValue) &&
          change.newValue === String(newValue == null ? '' : newValue)
        ))) return;
        const change = createDesignChange({
          element: selectedDesignElement,
          type,
          property,
          oldValue,
          newValue,
        });
        reduceCurrentDesignChanges({ type: 'add', change });
        setDesignCommand({
          seq: Date.now(),
          kind: 'apply',
          payload: {
            selector: selectedDesignElement.selector,
            changeId: change.id,
            changeType: type,
            property,
            oldValue,
            value: newValue,
          },
        });
        setSelectedDesignElement((prev) => {
          if (!prev) return prev;
          if (type === 'text') return { ...prev, text: String(newValue || '') };
          return {
            ...prev,
            computedStyle: {
              ...(prev.computedStyle || {}),
              [property]: String(newValue || ''),
            },
          };
        });
      }, [designChanges, reduceCurrentDesignChanges, selectedDesignElement]);
      const handleDesignChangeApplied = useCallback((result) => {
        if (!result || !result.changeId || result.changeId === 'clear') return;
        reduceCurrentDesignChanges({
          type: 'mark-applied',
          changeId: result.changeId,
          ok: result.ok,
          error: result.error,
        });
      }, [reduceCurrentDesignChanges]);
      const handleDesignMutation = useCallback((payload) => {
        const element = payload && payload.element;
        const changes = Array.isArray(payload && payload.changes) ? payload.changes : [];
        if (!element || !changes.length) return;
        const groupId = `design-group-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
        const groupLabel = payload.groupLabel || chatViewCopy.designEditGroup;
        changes.forEach((item) => {
          if (!item || String(item.oldValue == null ? '' : item.oldValue) === String(item.newValue == null ? '' : item.newValue)) return;
          const change = createDesignChange({
            element,
            type: item.type || 'style',
            property: item.property,
            oldValue: item.oldValue,
            newValue: item.newValue,
            groupId,
            groupLabel,
          });
          reduceCurrentDesignChanges({ type: 'add', change });
          reduceCurrentDesignChanges({ type: 'mark-applied', changeId: change.id, ok: true });
        });
        setSelectedDesignElement(element);
        updatePinvouModeState({ type: 'set-selected-design-element', elementId: element.id });
      }, [reduceCurrentDesignChanges, updatePinvouModeState, chatViewCopy]);
      const handleClearDesignChanges = useCallback(() => {
        setDesignCommand({ seq: Date.now(), kind: 'clear' });
        reduceCurrentDesignChanges({ type: 'clear' });
        setSelectedDesignElement(null);
      }, [reduceCurrentDesignChanges]);
      const visualPosterSceneActive = shouldUseVisualPosterScene(pinvouMode, designSubtab);
      const documentWritingSceneActive = shouldUseDocumentWritingScene(pinvouMode, workSubtab);
      const personalWorkbenchSceneActive = shouldUsePersonalWorkbenchScene(pinvouMode, workSubtab);
      const dataVisualizationSceneActive = shouldUseDataVisualizationScene(pinvouMode, designSubtab);
      const activeScene = pinvouMode === 'work'
        ? workModeSubtabs.find(item => item.key === workSubtab)
        : pinvouMode === 'design'
          ? designModeSubtabs.find(item => item.key === designSubtab)
          : null;
      const composerPlaceholder = pinvouMode === 'design'
        ? selectedDesignElement
          ? chatViewCopy.placeholderDesignAdjust
          : dataVisualizationSceneActive
            ? chatViewCopy.placeholderDesignDataViz
          : visualPosterSceneActive
            ? chatViewCopy.placeholderDesignPoster
            : sceneCopy.designGeneralPlaceholder
        : pinvouMode === 'work'
          ? personalWorkbenchSceneActive
            ? chatViewCopy.placeholderPersonalWorkbench
            : documentWritingSceneActive
              ? chatViewCopy.placeholderWorkDocument
              : t.placeholder
        : t.placeholder;
      const hasSkill = !!(bs && bs.workflow && bs.workflow.activeSkillName);
      const isScheduledTaskCreationChat = !!(bs && bs.scheduledTaskCreationSessionId && bs.activeSessionId === bs.scheduledTaskCreationSessionId);
      const scheduledRunContext = bs && bs.scheduledRunContext && bs.scheduledRunContext.sessionId === bs.activeSessionId
        ? bs.scheduledRunContext
        : null;
      let lastUserId = null;
      for (let i = chatItems.length - 1; i >= 0; i--) { if (chatItems[i].type === 'user') { lastUserId = chatItems[i].id; break; } }
      const useUnifiedConversationUi = unifiedConversationUiEnabled();
      const visibleChatItems = chatItems.filter((item) => !(item.type === 'memory_candidate' && !item.resolved));
      const latestArtIdByPath = {};
      chatItems.forEach((item) => {
        if (item.type === 'artifact_card' && item.path) latestArtIdByPath[item.path] = item.id;
      });
      const latestArtifactIds = new Set(Object.values(latestArtIdByPath));
      const conversationProjection = projectDeepSeekConversation({
        chatItems: conversationItemsForMode(visibleChatItems, useUnifiedConversationUi),
        busy,
        thinking: bs && bs.thinking,
        tokens: ctxTokens,
        sessionId: bs && bs.activeSessionId,
        timelineEvents: bs && bs.turnTimeline,
        allowScheduledTaskDraft: isScheduledTaskCreationChat,
      });
      const activeConversationTurn = [...conversationProjection.turns]
        .reverse()
        .find(turn => turn.status === 'running') || null;
      const [conversationNow, setConversationNow] = useState(Date.now());
      useEffect(() => {
        if (!busy || !useUnifiedConversationUi) return undefined;
        setConversationNow(Date.now());
        const timer = window.setInterval(() => setConversationNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
      }, [busy, useUnifiedConversationUi, bs && bs.thinking && bs.thinking.startedAt]);

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

      // 用户向上翻历史时暂停流式自动贴底；回到底部或发送新消息后恢复。
      useEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const onScroll = () => {
          const near = isNearConversationBottom(el);
          const movingUp = el.scrollTop < lastScrollTopRef.current - 1;
          lastScrollTopRef.current = el.scrollTop;
          if (movingUp) autoScrollRef.current = false;
          else if (near) autoScrollRef.current = true;
          const shouldShow = !autoScrollRef.current && el.scrollHeight > el.clientHeight + 4;
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

      // Auto-scroll：只在原本贴底时滚内部容器到底（绝不动外层窗口，避免浏览历史时被拉回底部）
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
          const shouldShow = el.scrollHeight > el.clientHeight + 4;
          setShowScrollBottom(v => v === shouldShow ? v : shouldShow);
        }
      }, [
        chatItems.length,
        chatItems[chatItems.length - 1]?.html,
        chatItems[chatItems.length - 1]?.state === 'running'
          ? chatItems[chatItems.length - 1]?.output?.length
          : 0,
        composerH,
      ]);

      // 切换/加载会话:无条件把新会话滚到最底部(最新消息)并复位 autoScrollRef。
      // 上面的流式 auto-scroll 复用了跨会话持久的 autoScrollRef + 不 remount 的滚动容器,
      // 若切走前在旧会话翻过历史(autoScrollRef=false),切来的新会话会命中 else 分支、停在
      // 残留 scrollTop 半空处。按 activeSessionId 单独滚底,且在流式 effect 之后声明→后跑覆盖它。
      useEffect(() => {
        const el = scrollRef.current;
        autoScrollRef.current = true;
        lastScrollTopRef.current = 0;
        setShowScrollBottom(false);
        if (el) {
          el.scrollTop = el.scrollHeight;
          lastScrollTopRef.current = el.scrollTop;
        }
      }, [bs && bs.activeSessionId]);

      // 安装工具后新建会话 → 本地显示欢迎卡片（不发 LLM query，不浪费 token）。
      // welcomeToolId 是一次性引导态,必须跟随会话身份:只有"装完工具"(justInstalledTool 非
      // null)才显示;其余任何新建对话/切换会话(activeSessionId 变)都清掉,否则残留的工具卡会
      // 顶掉「你好」欢迎语(该 tool 无 welcomeQueries 时 ToolWelcomeCard 渲染 null → 整块空白)。
      // 设置与清空收进同一 effect,按 justInstalledTool 优先,避免多 effect 同帧竞态。
      const [welcomeToolId, setWelcomeToolId] = useState(null);
      const welcomeSessionKeyRef = useRef(null);
      // Web 只读判定：多智能体是桌面专属能力（ADR-0006），Web 端只读呈现。
      // modeState.multiAgent 经 get_mode_state 双端同步（开关已持久化）。
      const isMultiAgentReadOnly = !MULTI_AGENT_ENABLED
        && !!(bs && bs.modeState && bs.modeState.multiAgent);
      const artifactsVisible = Boolean(activeSessionId && artifactsOpen);
      useEffect(() => {
        if (designAiSessionRef.current && designAiSessionRef.current !== activeSessionId) {
          updateDesignAiState({ text: '', status: 'idle', lastPrompt: '', pendingPath: '', startedAt: 0 });
          if (typeof window !== 'undefined') window.__PINVOU_DESIGN_AI_STATE__ = null;
        }
        designAiSessionRef.current = activeSessionId || null;
      }, [activeSessionId, updateDesignAiState]);
      useEffect(() => {
        if (!artifactsFullscreen || typeof window === 'undefined') return;
        const saved = window.__PINVOU_DESIGN_AI_STATE__;
        if (!saved || (!saved.text && !saved.lastPrompt && saved.status === 'idle')) return;
        if (!designAiState.text && !designAiState.lastPrompt && designAiState.status === 'idle') {
          setDesignAiState(saved);
        }
      }, [artifactsFullscreen, designAiState.text, designAiState.lastPrompt, designAiState.status]);
      useEffect(() => {
        if (!activeSessionId) setArtifactsOpen(false);
        if (!activeSessionId) setArtifactsFullscreen(false);
      }, [activeSessionId]);
      useEffect(() => {
        if (!artifactsVisible) setArtifactsFullscreen(false);
      }, [artifactsVisible]);
      const closeArtifactsPanel = useCallback(() => {
        setArtifactsFullscreen(false);
        setArtifactsOpen(false);
      }, []);
      // 子智能体只读执行记录面板（Codex 式右侧列，ADR-0006）。任何工作会话可开
      // （裸 agent 在普通工作对话同样可用）；与产物面板互斥，否则窄窗下聊天列被挤没。
      // null=关闭；agentId 为空进列表页。selectionRequestId 让“详情→返回列表→
      // 再点同一张主对话卡”也成为一次新选择，不能只靠相同 agentId 的 prop 变化。
      const [subagentPanel, setSubagentPanel] = useState(null);
      useEffect(() => { setSubagentPanel(null); }, [activeSessionId]);
      const rememberScrollBeforeSubagentPanelChange = useCallback(() => {
        subagentPanelScrollRef.current = captureConversationScrollPosition(
          scrollRef.current,
          autoScrollRef.current,
        );
      }, []);
      const closeSubagentPanel = useCallback(() => {
        rememberScrollBeforeSubagentPanelChange();
        setSubagentPanel(null);
      }, [rememberScrollBeforeSubagentPanelChange]);
      useLayoutEffect(() => {
        const snapshot = subagentPanelScrollRef.current;
        if (!snapshot) return;
        subagentPanelScrollRef.current = null;
        const el = scrollRef.current;
        if (!el) return;
        restoreConversationScrollPosition(el, snapshot);
        lastScrollTopRef.current = el.scrollTop;
        if (snapshot.stickToBottom) {
          autoScrollRef.current = true;
          setShowScrollBottom(false);
        }
      }, [subagentPanel]);
      useEffect(() => {
        if (typeof window === 'undefined') return undefined;
        const onOpen = (event) => {
          const detail = event && event.detail;
          if (detail?.sessionId && detail.sessionId !== activeSessionId) return;
          rememberScrollBeforeSubagentPanelChange();
          setSubagentPanel((current) => ({
            agentId: (detail && detail.agentId) || null,
            selectionRequestId: (current?.selectionRequestId || 0) + 1,
          }));
          closeArtifactsPanel();
        };
        window.addEventListener('pinvou:open-subagent', onOpen);
        return () => window.removeEventListener('pinvou:open-subagent', onOpen);
      }, [activeSessionId, closeArtifactsPanel, rememberScrollBeforeSubagentPanelChange]);
      useEffect(() => {
        if (artifactsVisible) setSubagentPanel(null);
      }, [artifactsVisible]);
      const handlePreviewArtifact = useCallback((artifact) => {
        setActiveArtifactPath(artifact && artifact.path ? artifact.path : null);
      }, []);
      const openArtifactsPreview = useCallback(() => {
        if (latestArtifact && latestArtifact.path) setActiveArtifactPath(latestArtifact.path);
        setArtifactsOpen(true);
      }, [latestArtifact]);
      useEffect(() => {
        const previousCount = previousArtifactCountRef.current;
        previousArtifactCountRef.current = artifactCount;
        if (pinvouMode !== 'design') return;
        if (artifactCount <= previousCount || !latestArtifact || !latestArtifact.path) return;
        setActiveArtifactPath(latestArtifact.path);
        setArtifactsOpen(true);
      }, [artifactCount, latestArtifact, pinvouMode]);
      const draftEpoch = bs ? bs.draftEpoch : 0;
      // 切换 session / 新建草稿会话时读取各自 working set 里的未发送内容。
      // 从设置、工具商店等页面返回时 ChatView 会重新挂载，初始 state 也从
      // 同一份内存草稿恢复。
      useEffect(() => {
        const restored = bridge.available && bridge.chat && bridge.chat.getComposerDraft
          ? bridge.chat.getComposerDraft()
          : ((bs && bs.composerDraft) || '');
        setInputText(restored);
      }, [activeSessionId, draftEpoch, setInputText]);
      const voiceInput = (bs && bs.voiceInput) || { status: 'idle' };
      const voiceActive = voiceInput.status === 'requesting_permission' || voiceInput.status === 'recording' || voiceInput.status === 'transcribing';
      const voiceRecording = voiceInput.status === 'recording';
      const voiceBusy = voiceInput.status === 'transcribing';
      const voiceNotice = voiceInput.status !== 'idle' && voiceInput.message;
      const hasDraftText = inputText.trim().length > 0;
      const hasReadyAttachment = attachments.some(a => a.status === 'ready');
      const firstTurnPending = !activeSessionId && chatItems.some(item => (
        item && item.type === 'user' && !!item.deliveryState
      ));
      const canSend = !isMultiAgentReadOnly
        && !firstTurnPending
        && (hasDraftText || hasReadyAttachment);
      const sceneCapabilityPreparing = sceneCapabilityStatus && sceneCapabilityStatus.kind === 'preparing';
      const canFloatingSend = canSend && !voiceActive && !sceneCapabilityPreparing;
      const canClearInput = hasDraftText && !voiceActive;
      const sendChatMessage = useCallback(async (text) => {
        if (!bridge.available) return false;
        const outgoing = String(text || '').trim();
        const matchedPersonalWorkbenchDraft = findPersonalWorkbenchTemplateDraft(outgoing);
        const templateId = personalWorkbenchTemplateIdRef.current
          || (matchedPersonalWorkbenchDraft && matchedPersonalWorkbenchDraft.template
            ? matchedPersonalWorkbenchDraft.template.id
            : null);
        const visibleOutgoing = outgoing;
        let meta;
        if (visibleOutgoing || hasReadyAttachment) {
          const scenePrompt = outgoing || '请根据附件内容继续处理。';
          if (visualPosterSceneActive) meta = createVisualPosterMessageMeta(scenePrompt);
          else if (documentWritingSceneActive) meta = createDocumentWritingMessageMeta(scenePrompt);
          else if (personalWorkbenchSceneActive) meta = createPersonalWorkbenchMessageMeta(scenePrompt, templateId);
          else if (dataVisualizationSceneActive) meta = createDataVisualizationMessageMeta(scenePrompt);
        }
        const requirements = requiredCapabilitiesForMeta(meta);
        if (requirements) {
          const sceneCopy = t.uiChatScenes[requirements.key];
          if (!canPrepareSceneCapabilities({ isWebHost: isWeb, dependencyInstallAvailable: can('dependencyInstall') })) {
            setSceneCapabilityStatus(null);
          } else {
            setSceneCapabilityStatus({ kind: 'preparing', text: sceneCopy.preparing });
            try {
              const prepared = await prepareSceneCapabilities(meta, invokeTauri);
              if (!prepared.ok) {
                const missing = prepared.missing && prepared.missing.length
                  ? t.uiChatScenes.missingCapabilities(prepared.missing.join(', '))
                  : '';
                throw new Error(missing || sceneCopy.failure);
              }
              if (prepared.installed) {
                setSceneCapabilityStatus({ kind: 'ready', text: sceneCopy.ready });
                window.setTimeout(() => setSceneCapabilityStatus((current) => (
                  current && current.kind === 'ready' ? null : current
                )), 1800);
              } else {
                setSceneCapabilityStatus(null);
              }
            } catch (error) {
              const message = error && error.message ? error.message : String(error || '');
              setSceneCapabilityStatus({
                kind: 'error',
                text: message ? `${sceneCopy.failure} ${message}` : sceneCopy.failure,
              });
              return false;
            }
          }
        } else {
          setSceneCapabilityStatus(null);
        }
        if (!activeSessionId) {
          pendingModeScopeMigrationRef.current = {
            text: visibleOutgoing,
            state: pinvouModeStateRef.current,
          };
        }
        try {
          await bridge.chat.sendMessage(visibleOutgoing, meta);
        } catch (error) {
          pendingModeScopeMigrationRef.current = null;
          throw error;
        }
        return true;
      }, [activeSessionId, dataVisualizationSceneActive, documentWritingSceneActive, hasReadyAttachment, personalWorkbenchSceneActive, t, visualPosterSceneActive]);
      const handleDesignAiSubmit = useCallback((text) => {
        const raw = String(text || '').trim();
        if (!raw) return;
        const elementLabel = selectedDesignElement
          ? `${selectedDesignElement.tagName || chatViewCopy.designElementFallback}${selectedDesignElement.className ? `.${String(selectedDesignElement.className).trim().split(/\s+/)[0]}` : ''}`
          : '';
        const scopedText = selectedDesignElement
          ? chatViewCopy.designAdjustSelected(elementLabel || chatViewCopy.designElementFallback, raw)
          : raw;
        sendChatMessage(scopedText);
      }, [selectedDesignElement, sendChatMessage]);
      const [deviceMode, setDeviceMode] = useState(() => {
        const w = typeof window !== 'undefined' ? window.innerWidth : 1280;
        const h = typeof window !== 'undefined' ? window.innerHeight : 900;
        const coarse = typeof window !== 'undefined' && window.matchMedia ? window.matchMedia('(pointer: coarse)').matches : false;
        const touch = coarse || (typeof navigator !== 'undefined' && navigator.maxTouchPoints > 0);
        return { w, h, touch };
      });
      const isTabletSized = Math.min(deviceMode.w, deviceMode.h) <= 820 || Math.max(deviceMode.w, deviceMode.h) <= 1180;
      // 浮动语音球是给 Windows 平板/触屏大屏用的；手机输入栏已有麦克风，浮球只会遮挡消息。
      const isPhoneSized = Math.min(deviceMode.w, deviceMode.h) < 640;
      const tabletVoiceMode = (deviceMode.touch || isTabletSized) && !isPhoneSized;
      const primaryVoiceDisabled = !bridge.available || voiceBusy;
      const primaryVoiceLabel = voiceInput.status === 'recording'
        ? t.voiceStop
        : voiceInput.status === 'failed'
          ? t.voiceRetry
          : voiceInput.status === 'requesting_permission'
            ? t.voiceCancel
            : voiceInput.status === 'transcribing'
              ? t.voiceTranscribing
              : t.voiceStart;
      function clampFloatingVoicePos(x, y) {
        const root = chatRootRef.current;
        const floater = floatingVoiceRef.current;
        if (!root || !floater) return { x, y };
        const rootRect = root.getBoundingClientRect();
        const floatRect = floater.getBoundingClientRect();
        const margin = 12;
        const maxX = Math.max(margin, rootRect.width - floatRect.width - margin);
        const maxY = Math.max(margin, rootRect.height - floatRect.height - margin);
        return {
          x: Math.min(Math.max(x, margin), maxX),
          y: Math.min(Math.max(y, margin), maxY),
        };
      }
      const floatingVoiceStyle = floatingVoicePos
        ? { left: floatingVoicePos.x + 'px', top: floatingVoicePos.y + 'px' }
        : { left: 'calc(100% - 220px)', top: '50%', transform: 'translateY(-50%)' };
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

      useEffect(() => {
        const measureDeviceMode = () => {
          const w = window.innerWidth;
          const h = window.innerHeight;
          const coarse = window.matchMedia ? window.matchMedia('(pointer: coarse)').matches : false;
          const touch = coarse || navigator.maxTouchPoints > 0;
          setDeviceMode({ w, h, touch });
        };
        measureDeviceMode();
        window.addEventListener('resize', measureDeviceMode);
        window.addEventListener('orientationchange', measureDeviceMode);
        const mq = window.matchMedia ? window.matchMedia('(pointer: coarse)') : null;
        if (mq && mq.addEventListener) mq.addEventListener('change', measureDeviceMode);
        return () => {
          window.removeEventListener('resize', measureDeviceMode);
          window.removeEventListener('orientationchange', measureDeviceMode);
          if (mq && mq.removeEventListener) mq.removeEventListener('change', measureDeviceMode);
        };
      }, []);

      useEffect(() => {
        const finishFromWindow = (event) => {
          finishFloatingVoicePointer(event.pointerId, event, true, event.type);
        };
        const finishOnBlur = () => {
          const drag = voiceDragRef.current;
          if (drag && drag.pointerId !== null) finishFloatingVoicePointer(drag.pointerId, null, true, 'blur');
        };
        window.addEventListener('pointerup', finishFromWindow, true);
        window.addEventListener('pointercancel', finishFromWindow, true);
        window.addEventListener('blur', finishOnBlur);
        return () => {
          window.removeEventListener('pointerup', finishFromWindow, true);
          window.removeEventListener('pointercancel', finishFromWindow, true);
          window.removeEventListener('blur', finishOnBlur);
          if (voiceDragClickResetRef.current) window.clearTimeout(voiceDragClickResetRef.current);
          const drag = voiceDragRef.current;
          if (drag && drag.pointerId !== null) {
            const pointerId = drag.pointerId;
            const target = drag.target;
            finishFloatingVoiceDrag(drag, pointerId);
            try {
              if (target && target.hasPointerCapture(pointerId)) target.releasePointerCapture(pointerId);
            } catch (_) {}
          }
          voiceDragRef.current = null;
        };
      }, []);

      useEffect(() => {
        if (!tabletVoiceMode || !floatingVoicePos) return;
        const raf = requestAnimationFrame(() => {
          setFloatingVoicePos(pos => pos ? clampFloatingVoicePos(pos.x, pos.y) : pos);
        });
        return () => cancelAnimationFrame(raf);
      }, [tabletVoiceMode, hasDraftText, hasReadyAttachment, deviceMode.w, deviceMode.h]);

      // chip 显示当前会话绑定的模型:切会话/草稿时刷新 currentSessionModelId
      useEffect(() => {
        if (bridge.available) bridge.models.loadSessionModel(activeSessionId);
      }, [activeSessionId]);

      // 普通会话选图即时警告(阶段 G):当前模型图片路由为 unsupported 时在附件区提示,
      // 仅提示不拦截,发送时后端仍按同一路径复核(chat 命令 image_input_unsupported)。
      // scheduled 会话发送时不做图片路由(固定工具兜底),这里同样不提示。
      const hasImageAttachment = attachments.some(a => !!(a && a.result && a.result.kind === 'image'));
      const isScheduledSession = !!(scheduledRunContext || isScheduledTaskCreationChat);
      const sessionModelKey = (bs && bs.currentSessionModelId) || (bs && bs.activeModelId) || '';
      const [imageInputInfo, setImageInputInfo] = useState(null);
      useEffect(() => {
        if (!hasImageAttachment || isScheduledSession || !bridge.available
          || !bridge.models || typeof bridge.models.getImageInputCapability !== 'function') {
          setImageInputInfo(null);
          return undefined;
        }
        let cancelled = false;
        bridge.models.getImageInputCapability(activeSessionId)
          .then(info => { if (!cancelled) setImageInputInfo(info || null); })
          // 查询失败(如凭据未备/旧后端无此命令)按无警告处理,绝不误报。
          .catch(() => { if (!cancelled) setImageInputInfo(null); });
        return () => { cancelled = true; };
      }, [hasImageAttachment, isScheduledSession, activeSessionId, sessionModelKey, bs && bs.savedModels]);
      const imageInputWarning = imageInputInfo && imageInputInfo.image_mode === 'unsupported'
        ? (imageInputInfo.capability === 'unknown' ? t.uiAttachments.imageUnknown : t.uiAttachments.imageUnsupported)
        : '';
      // 云上传隐私提示(§11.8/§11.9):图片字节离开本机时告知去向——native 直发看主模型
      // 端点,fallback 看兜底视觉模型端点(图片实际发给视觉模型);本机 loopback 不显示
      // 任何云上传字样;查询失败或旧后端无对应字段时 fail-open 不显示。
      const imagePrivacyHint = imageInputInfo && (
        (imageInputInfo.image_mode === 'native' && imageInputInfo.is_local_endpoint === false)
        || (imageInputInfo.image_mode === 'vision_tool_fallback' && imageInputInfo.vision_is_local_endpoint === false)
      )
        ? (imageInputInfo.image_mode === 'vision_tool_fallback'
          ? t.uiAttachments.imageCloudUploadVision
          : t.uiAttachments.imageCloudUpload)
        : '';

      async function handleSend() {
        // 不再因 busy 拦截:bridge.chat.sendMessage 在生成中会把这句排队(本轮跑完自动发)。
        if (isMultiAgentReadOnly || !canSend) return;
        const constrained = constrainChatInput(inputText);
        if (constrained.truncated) {
          setInputText(constrained.text);
          return;
        }
        const accepted = await sendChatMessage(constrained.text);
        if (accepted) {
          setInputText('');
          personalWorkbenchTemplateIdRef.current = null;
          setPersonalWorkbenchTemplateId(null);
        }
      }

      function handleKeyDown(e) {
        // 输入法合成期间(例如中文输入法敲回车确认候选词上屏)不要触发发送,
        // 否则一次回车会既上屏又发送消息。与 PetWindow 处理保持一致。
        if (e.key === 'Enter' && !e.shiftKey && !isImeComposing(e)) {
          e.preventDefault();
          handleSend();
        }
      }

      function handleCancel() {
        if (bridge.available) bridge.chat.cancelGeneration();
      }

      function finishFloatingVoicePointer(pointerId, event, releaseCapture, reason) {
        const drag = voiceDragRef.current;
        const target = drag && drag.target;
        const result = finishFloatingVoiceDrag(drag, pointerId, {
          suppressCompatibleClick: reason === 'pointerup' || reason === 'lostpointercapture' || reason === 'buttons-released',
        });
        if (!result.matched) return false;

        setFloatingVoicePressed(false);
        if (voiceDragClickResetRef.current) {
          window.clearTimeout(voiceDragClickResetRef.current);
          voiceDragClickResetRef.current = null;
        }
        if (drag.suppressClick) {
          if (event && event.preventDefault) event.preventDefault();
          voiceDragClickResetRef.current = window.setTimeout(() => {
            if (voiceDragRef.current === drag) clearFloatingVoiceDragClick(drag);
            voiceDragClickResetRef.current = null;
          }, FLOATING_VOICE_CLICK_SUPPRESSION_MS);
        }
        if (releaseCapture) {
          try {
            if (target && target.hasPointerCapture(pointerId)) target.releasePointerCapture(pointerId);
          } catch (_) {}
        }
        drag.target = null;
        return true;
      }

      function handleFloatingVoiceClick(event) {
        const nativeEvent = event.nativeEvent || event;
        if (consumeFloatingVoiceDragClick(voiceDragRef.current, {
          detail: nativeEvent.detail,
          pointerId: nativeEvent.pointerId,
          pointerType: nativeEvent.pointerType,
        })) {
          event.preventDefault();
          if (voiceDragClickResetRef.current) {
            window.clearTimeout(voiceDragClickResetRef.current);
            voiceDragClickResetRef.current = null;
          }
          return;
        }
        handleVoiceClick();
      }

      function handleVoiceClick() {
        if (!bridge.available) return;
        if (voiceInput.status === 'requesting_permission') {
          bridge.voice.cancelVoiceInput();
          return;
        }
        if (voiceBusy) return;
        if (voiceInput.status === 'recording') {
          bridge.voice.startVoiceInput(inputText, (text) => setInputText(prev => bridge.voice.appendVoiceText(prev, text)));
          return;
        }
        bridge.voice.startVoiceInput(inputText, (text) => setInputText(prev => bridge.voice.appendVoiceText(prev, text)));
      }

      function handleFloatingVoicePointerDown(e) {
        if (!tabletVoiceMode) return;
        const activeDrag = voiceDragRef.current;
        if (activeDrag && activeDrag.pointerId !== null) return;
        if (!canStartFloatingVoiceDrag(e)) return;
        const root = chatRootRef.current;
        const floater = floatingVoiceRef.current;
        if (!root || !floater) return;
        if (voiceDragClickResetRef.current) {
          window.clearTimeout(voiceDragClickResetRef.current);
          voiceDragClickResetRef.current = null;
        }
        const floatRect = floater.getBoundingClientRect();
        const target = e.currentTarget;
        const drag = createFloatingVoiceDragSession({
          pointerId: e.pointerId,
          pointerType: e.pointerType,
          clientX: e.clientX,
          clientY: e.clientY,
          offsetX: e.clientX - floatRect.left,
          offsetY: e.clientY - floatRect.top,
        });
        drag.target = target;
        voiceDragRef.current = drag;
        setFloatingVoicePressed(true);
        try { target.setPointerCapture(e.pointerId); } catch (_) {}
      }

      function handleFloatingVoicePointerMove(e) {
        const drag = voiceDragRef.current;
        const movement = moveFloatingVoiceDrag(drag, {
          pointerId: e.pointerId,
          clientX: e.clientX,
          clientY: e.clientY,
          buttons: e.buttons,
        });
        if (movement.kind === 'released') {
          finishFloatingVoicePointer(e.pointerId, e, true, 'buttons-released');
          return;
        }
        if (movement.kind !== 'move') return;
        const root = chatRootRef.current;
        if (!root) return;
        e.preventDefault();
        const rootRect = root.getBoundingClientRect();
        setFloatingVoicePos(clampFloatingVoicePos(
          movement.x - rootRect.left,
          movement.y - rootRect.top
        ));
      }

      function handleFloatingVoicePointerEnd(e) {
        finishFloatingVoicePointer(e.pointerId, e, true, e.type);
      }

      function handleFloatingVoiceLostPointerCapture(e) {
        finishFloatingVoicePointer(e.pointerId, e, false, 'lostpointercapture');
      }

      function handleClearInput() {
        if (!canClearInput) return;
        setInputText('');
        personalWorkbenchTemplateIdRef.current = null;
        setPersonalWorkbenchTemplateId(null);
      }

      function handleVoiceCancel() {
        if (bridge.available) bridge.voice.cancelVoiceInput();
      }

      function handleVoiceClose() {
        if (bridge.available) bridge.voice.clearVoiceInput();
      }

      function handlePaste(e) {
        if (isWeb) return;
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
              if (bridge.available) bridge.attachments.addPasteImage(`paste-${Date.now()}.${ext}`, bytes);
            };
            reader.readAsArrayBuffer(file);
          }
        }
      }

      return (
        <div ref={rootRef} className="flex-1 flex flex-row w-full h-full min-h-0 relative z-10 animate-in fade-in duration-300">
          <div ref={chatRootRef} className="flex-1 flex flex-col min-w-0 relative h-full">
            <AttachmentDropOverlay
              active={attachmentDragActive}
              dark={isDark}
              variant={isWeb ? 'web' : 'desktop'}
              releaseLabel={t.uiAttachments.dropRelease}
              webTitle={t.uiAttachments.dropWebTitle}
              webHint={t.uiAttachments.dropWebHint}
            />

          {/* Top Header (浮动) */}
          <div className="absolute top-0 left-0 right-0 p-4 flex justify-between items-center z-20 pointer-events-none">
            <div className="flex items-center gap-2 min-w-0">
              {scheduledRunContext && (
                <button type="button" onClick={onBackScheduledRun}
                  data-testid="scheduled-run-back"
                  aria-label={chatCopy.backRuns}
                  title={chatCopy.backRuns}
                  className={`pointer-events-auto h-10 max-w-[520px] max-sm:max-w-[55vw] px-3 rounded-full flex items-center gap-2 border text-[14px] font-medium transition-colors ${isDark ? 'bg-[#1E1F20] border-[#333537] text-[#E3E3E3] hover:bg-[#2B2C2F]' : 'bg-white border-[#E3E5E8] text-[#1F1F1F] hover:bg-[#F5F5F6] shadow-sm'}`}>
                  <ArrowLeft size={16} className="shrink-0" />
                  <span className="truncate">{scheduledRunContext.taskName || chatCopy.scheduledRun}</span>
                  <span className={`shrink-0 text-[12px] max-sm:hidden ${isDark ? 'text-[#9AA0A6]' : 'text-[#85888D]'}`}>{chatCopy.runRecords}</span>
                </button>
              )}
            </div>
            <div className="flex items-center gap-2">
              {/* 窄屏：只留图标+计数（避免被左侧返回按钮挤到换行），文字标签 ≥sm 才显示 */}
              {activeSessionId && (
                <button
                  data-testid="chat-artifacts-entry"
                  onClick={openArtifactsPreview}
                  className={`pointer-events-auto px-4 max-sm:px-3 py-2 rounded-full text-[14px] font-medium flex items-center gap-2 whitespace-nowrap shrink-0 ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3] hover:bg-[#333537]' : 'bg-white text-[#1F1F1F] hover:bg-[#F0F4F9] shadow-sm'}`}>
                  <Package size={16} /> <span className="max-sm:hidden">{t.artifacts}</span>
                  {artifactCount > 0 && <span className={`text-[11px] px-1.5 rounded-full ${isDark ? 'bg-[#A8C7FA] text-[#062E6F]' : 'bg-[#0B57D0] text-white'}`}>{artifactCount}</span>}
                </button>
              )}
            </div>
          </div>


          {/* Main Chat Area */}
          {/* 有消息时底部留白由列表内的实体 spacer 承担，避免 WebKitGTK/Safari
              不把 overflow flex 容器的尾部 padding 完整计入 scrollHeight。
              空态不滚动，仍需 paddingBottom 让欢迎语在悬浮输入框上方居中。 */}
          <div ref={scrollRef} data-testid="chat-scroll"
            style={hasMessages ? undefined : { paddingBottom: (composerH ? composerH + 48 : 160) + 'px' }}
            className={`flex-1 min-h-0 min-w-0 overflow-y-auto ${(artifactsVisible && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'} custom-scrollbar flex flex-col ${hasSkill ? 'pt-3' : 'pt-20'} max-sm:pt-16 ${hasMessages ? 'justify-start' : 'items-center justify-center'}`}>

            {!hasMessages && !welcomeToolId && (
              /* Gemini Style Centered Empty State */
              <div className="w-full max-w-[760px] px-4 text-center mb-12 animate-in slide-in-from-bottom-4 duration-500">
                <h1 data-testid="chat-greeting" className={`${isWeb ? 'text-[28px] leading-[1.35] px-2 [text-wrap:balance] sm:text-[44px] sm:leading-normal sm:px-0' : 'text-[34px] md:text-[44px] leading-tight whitespace-normal break-words'} font-normal mb-2 ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>
                  {t.chatGreeting}
                </h1>
              </div>
            )}

            {!hasMessages && welcomeToolId && (
              <div className="max-w-[800px] w-full mx-auto mt-8">
                <ToolWelcomeCard
                  toolId={welcomeToolId}
                  theme={theme}
                  t={t}
                  onSend={(q) => {
                    setWelcomeToolId(null);
                    sendChatMessage(q);
                  }}
                />
              </div>
            )}

            {hasMessages && (
              <div className="max-w-[800px] w-full min-w-0 mx-auto space-y-4">
                {useUnifiedConversationUi ? (
                  <ConversationTimeline
                    turns={conversationProjection.turns}
                    now={conversationNow}
                    copy={t.uiConversation}
                    agentLabel={chatViewCopy.agentName}
                    assistantAvatar={(
                      <div className="mt-1 flex h-7 w-7 shrink-0 items-center justify-center">
                        <PinvouLogo className="h-5 w-5" title={chatViewCopy.agentName} />
                      </div>
                    )}
                    renderUser={(item) => (
                      <ChatBubble
                        item={item}
                        sessionId={activeSessionId}
                        theme={theme}
                        t={t}
                        editable={!busy && !isMultiAgentReadOnly && item.id === lastUserId}
                        conversationVariant="unified"
                      />
                    )}
                    renderItem={(item) => {
                      // reasoning 由 ConversationTimeline 的 ReasoningItem 负责，
                      // 不能交给旧 ChatBubble；后者不认识该类型，会返回 null，
                      // 导致后端已收到的实时 thinking 被静默吞掉。
                      if (item.type === 'reasoning') return undefined;
                      if (!item.legacyItem) return undefined;
                      return (
                        <ChatBubble
                          item={item.legacyItem}
                          sessionId={activeSessionId}
                          theme={theme}
                          t={t}
                          onPrefill={(text) => setInputText(text)}
                          onSend={sendChatMessage}
                          onOpenEditor={onOpenEditor}
                          isLatestArtifact={latestArtifactIds.has(item.legacyItem.id)}
                          allowScheduledTaskDraft={isScheduledTaskCreationChat} showAssistantActions={false}
                        />
                      );
                    }}
                    renderToolItem={(item) => item.legacyItem
                      && !isSearchTool(item.tool)
                      && !isFetchTool(item.tool)
                      ? <ToolCard item={item.legacyItem} sessionId={activeSessionId} theme={theme} t={t} variant="timeline" />
                      : undefined}
                    onOpenExternal={openChatExternalUrl}
                  />
                ) : (
                  <>
                    {visibleChatItems.map((item) => (
                      <ChatBubble
                        key={item.id}
                        item={item}
                        sessionId={activeSessionId}
                        theme={theme}
                        t={t}
                        onPrefill={(text) => setInputText(text)}
                        onSend={sendChatMessage}
                        editable={!busy && !isMultiAgentReadOnly && item.id === lastUserId}
                        onOpenEditor={onOpenEditor}
                        isLatestArtifact={latestArtifactIds.has(item.id)}
                        allowScheduledTaskDraft={isScheduledTaskCreationChat}
                      />
                    ))}
                    {busy && <ThinkingBubble thinking={bs && bs.thinking} theme={theme} t={t} isLocal={activeModelLocal} />}
                  </>
                )}
                {/* 实体占位必须覆盖输入框和其上方渐变区，保证滚到底时最后一张卡
                    完整停在渐变之外，而不是虽然能滚到却被遮罩淡化。 */}
                <div data-testid="chat-bottom-spacer" aria-hidden="true" className="w-full shrink-0"
                  style={{ height: (composerH ? composerH + 64 : 176) + 'px' }} />
              </div>
            )}

          </div>

          {/* 底部渐变蒙层:内容滚到底时在输入框上方柔和淡出(pointer-events-none 不挡滑动/点击;高度跟随输入框 auto-grow)。 */}
          <div className={`pointer-events-none absolute bottom-0 inset-x-0 z-[15] bg-gradient-to-t to-transparent from-30% via-70% ${isDark ? 'from-[#131314] via-[#131314]/95' : 'from-white via-white/95'}`}
            style={{ height: (composerH ? composerH + 48 : 172) + 'px' }} />
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
            <div className={`pointer-events-none absolute inset-x-0 z-[24] ${(artifactsVisible && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'}`}
              style={{ bottom: (composerH ? composerH + 28 : 148) + 'px' }}>
              <div className="max-w-[800px] w-full mx-auto flex flex-col items-end gap-3">
                {chatItems
                  .filter((item) => item.type === 'memory_candidate' && !item.resolved)
                  .slice(-2)
                  .map((item) => (
                    <div key={item.id} className="pointer-events-auto w-full flex justify-end">
                      <ChatBubble item={item} sessionId={activeSessionId} theme={theme} t={t} onPrefill={(txt) => setInputText(txt)} onSend={sendChatMessage} editable={false} onOpenEditor={onOpenEditor} isLatestArtifact={false} />
                    </div>
                  ))}
              </div>
            </div>
          )}
          {tabletVoiceMode && (
            <div ref={floatingVoiceRef} style={floatingVoiceStyle} className="absolute z-30 flex items-center gap-2">
              <button
                onClick={handleFloatingVoiceClick}
                onPointerDown={handleFloatingVoicePointerDown}
                onPointerMove={handleFloatingVoicePointerMove}
                onPointerUp={handleFloatingVoicePointerEnd}
                onPointerCancel={handleFloatingVoicePointerEnd}
                onLostPointerCapture={handleFloatingVoiceLostPointerCapture}
                disabled={primaryVoiceDisabled}
                data-testid="floating-voice-button"
                data-pressed={floatingVoicePressed ? 'true' : 'false'}
                aria-label={primaryVoiceLabel}
                title={primaryVoiceLabel}
                className={`w-16 h-16 rounded-full flex items-center justify-center transition-all shadow-xl backdrop-blur-2xl touch-none select-none ${
                  voiceRecording
                    ? 'bg-[#C5221F] text-white shadow-red-500/25 hover:bg-[#A50E0E]'
                    : voiceBusy
                      ? (isDark ? 'bg-[#1E2B3A] text-[#A8C7FA] cursor-wait' : 'bg-[#E8F0FE] text-[#174EA6] cursor-wait')
                      : voiceInput.status === 'failed'
                        ? (isDark ? 'bg-[#3A1F1F] text-[#F28B82] hover:bg-[#4A2525]' : 'bg-[#FCE8E6] text-[#C5221F] hover:bg-[#FAD2CF]')
                        : (isDark ? 'bg-[#A8C7FA] text-[#062E6F] hover:bg-[#D3E3FD]' : 'bg-[#0B57D0] text-white hover:bg-[#0842A0] shadow-blue-500/25')
                } ${primaryVoiceDisabled ? 'opacity-80' : ''} ${floatingVoicePressed ? 'scale-95' : ''}`}>
                {voiceRecording ? <StopCircle size={26} /> : <Mic size={26} />}
              </button>
              {hasDraftText && (
                <button onClick={handleClearInput} disabled={!canClearInput} aria-label={t.clearInput} title={t.clearInput}
                  className={`w-12 h-12 rounded-full flex items-center justify-center border shadow-lg backdrop-blur-2xl transition-all ${
                    canClearInput
                      ? (isDark ? 'bg-[#161618]/90 border-white/10 text-[#C4C7C5] hover:bg-[#252629]' : 'bg-white/90 border-black/[0.06] text-[#5F6368] hover:bg-[#F1F3F4]')
                      : 'bg-black/5 dark:bg-white/10 text-gray-400 cursor-not-allowed opacity-60'
                  }`}>
                  <Trash2 size={20} />
                </button>
              )}
              {(hasDraftText || hasReadyAttachment) && (
                <button onClick={handleSend} disabled={!canFloatingSend} aria-label={t.sendMsg} title={t.sendMsg}
                  className={`w-12 h-12 rounded-full flex items-center justify-center shadow-lg transition-all ${
                    canFloatingSend
                      ? 'bg-gradient-to-b from-[#47A1FF] to-[#007AFF] text-white hover:-translate-y-0.5 active:translate-y-0'
                      : 'bg-black/5 dark:bg-white/10 text-gray-400 cursor-not-allowed'
                  }`}>
                  <Send size={19} className="translate-x-[1px]" />
                </button>
              )}
            </div>
          )}
          {/* Floating Input Area */}
          <div ref={composerWrapRef} data-testid="chat-composer-wrap" className={`absolute ${isWeb ? 'bottom-2 sm:bottom-8' : 'bottom-8'} inset-x-0 z-20 ${(artifactsVisible && isWide) ? 'px-4 md:px-8' : 'px-4 md:px-20 lg:px-40'}`}>
            <div className="max-w-[800px] w-full mx-auto">
              {!scheduledRunContext && !conversationStarted && (
                <HomeModeSwitcher
                  mode={pinvouMode}
                  codeSupported={codeModeAvailable}
                  isDark={isDark}
                  onChange={handleHomeModeChange}
                  copy={t.uiHomeMode}
                />
              )}
              {pinvouMode === 'work' && !conversationStarted && (
                <SubModePicker
                  isDark={isDark}
                  value={workSubtab}
                  onChange={handleWorkSubtabChange}
                  items={workModeSubtabs}
                  testId="work-subtab-picker"
                  comingSoonLabel={chatViewCopy.comingSoon}
                />
              )}
              {personalWorkbenchSceneActive && !conversationStarted && (
                <PersonalWorkbenchTemplatePicker
                  isDark={isDark}
                  selectedIndex={PERSONAL_WORKBENCH_TEMPLATES.findIndex(template => template.id === personalWorkbenchTemplateId)}
                  onSelect={handlePersonalWorkbenchTemplateSelect}
                  templates={PERSONAL_WORKBENCH_TEMPLATES}
                />
              )}
              {pinvouMode === 'design' && !conversationStarted && (
                <SubModePicker
                  isDark={isDark}
                  value={designSubtab}
                  onChange={handleDesignSubtabChange}
                  items={designModeSubtabs}
                  testId="design-subtab-picker"
                  comingSoonLabel={chatViewCopy.comingSoon}
                />
              )}
            {/* 排队待发消息 chips（生成中继续输入会积压到这里，本轮跑完自动发） */}
            {queued.length > 0 && (
              <div className="flex flex-col gap-1 mb-2 px-2">
                {queued.map((q) => (
                  <div key={q.id} className={`flex items-center gap-1.5 pl-3 pr-1.5 py-1 rounded-full text-[12px] self-start max-w-full ${isDark ? 'bg-[#2A2B2D] text-[#C4C7C5]' : 'bg-[#EAEDF1] text-[#444746]'}`}>
                    <span className="opacity-60">{t.queuedTag}</span>
                    <span className="max-w-[480px] truncate">{q.displayText}</span>
                    <button onClick={() => bridge.chat.removeQueued(q.id)} title={t.queuedCancel} className={`w-5 h-5 rounded-full flex items-center justify-center ${isDark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'}`}>×</button>
                  </div>
                ))}
              </div>
            )}
            {/* 模型选择器/知识库挂载已挪进下方底栏(ComposerModelSelector/ComposerKbSelector) */}
            {/* 附件 chips */}
            <AttachmentChips
              attachments={attachments}
              onRemove={id => bridge.attachments.removeAttachment(id)}
              dark={isDark}
              parsingLabel={t.attachParsing}
              uploadingLabel={t.attachUploading}
              failedLabel={t.attachFailed}
              removeLabel={t.uiAttachments.remove}
              formatError={formatAttachmentError}
              className="mb-2 px-2"
            />
            {imageInputWarning && (
              <div data-testid="image-capability-warning"
                className="flex items-center gap-2 mb-2 px-3 py-2 rounded-2xl text-[12px] leading-5 bg-amber-500/10 text-amber-700 dark:text-amber-300">
                <AlertTriangle size={14} className="shrink-0 text-amber-500" />
                <span className="min-w-0">{imageInputWarning}</span>
              </div>
            )}
            {imagePrivacyHint && (
              <div data-testid="image-privacy-hint"
                className="mb-2 px-3 text-[11px] leading-4 text-black/45 dark:text-white/45">
                {imagePrivacyHint}
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
                  {voiceInput.status === 'failed' && voiceInput.category === 'recognition_failed' && canInstallLocalAsr && onGotoSettings && (
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
            {voiceAsrSetup.open && !canInstallLocalAsr && (
              <div className={`flex items-center justify-between gap-3 mb-2 px-3 py-2 rounded-2xl text-[12px] ${isDark ? 'bg-[#1E2B3A] text-[#A8C7FA]' : 'bg-[#E8F0FE] text-[#174EA6]'}`}>
                <span>{chatCopy.asrUnavailable}</span>
                <button onClick={() => bridge.voice.closeVoiceAsrSetup()} className={`shrink-0 px-2 py-1 rounded-full font-medium ${isDark ? 'hover:bg-white/10' : 'hover:bg-black/5'}`}>{chatCopy.gotIt}</button>
              </div>
            )}
            {voiceAsrSetup.open && canInstallLocalAsr && (() => {
              const su = voiceAsrSetup;
              const prog = su.progress || {};
              const pct = (prog.stage === 'model' && prog.total) ? Math.floor(prog.downloaded / prog.total * 100) : null;
              const missing = (su.status && su.status.missing) || [];
              const needFfmpeg = missing.indexOf('ffmpeg') >= 0;
              const needModel = missing.indexOf('model') >= 0;
              const needEngine = missing.indexOf('engine') >= 0 || missing.indexOf('runtime') >= 0;
              const modelSizeText = (su.status && su.status.engine && needModel && !needFfmpeg) ? chatCopy.sizeModelOnly : chatCopy.sizeFull;
              return (
                <div className="fixed inset-0 z-[80] flex items-center justify-center p-4 bg-black/45"
                  onClick={() => { if (!su.installing) bridge.voice.closeVoiceAsrSetup(); }}>
                  <div className={`w-full max-w-[440px] rounded-[20px] shadow-2xl p-6 ${isDark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}
                    onClick={e => e.stopPropagation()}>
                    <h3 className="text-[16px] font-semibold mb-2">
                      {su.installing ? chatCopy.asrDownloadTitle : chatCopy.asrEnableTitle}
                    </h3>
                    {!su.installing && (
                      <p className="text-[13px] leading-relaxed opacity-80 mb-4">
                        {needEngine
                          ? chatCopy.asrRuntimeMissing
                          : chatCopy.asrFirstUse(modelSizeText, needFfmpeg)}
                      </p>
                    )}
                    {su.installing && (
                      <div className="mb-4">
                        <div className="text-[12px] opacity-70 mb-1">
                          {prog.stage === 'model'
                            ? chatCopy.downloadingModel(pct != null ? pct + '%' : '…')
                            : (chatCopy.asrStages[prog.stage] || chatCopy.asrStages.preparing)}
                        </div>
                        <div className={`h-2 rounded-full overflow-hidden ${isDark ? 'bg-white/10' : 'bg-black/10'}`}>
                          <div className="h-full bg-[#0B57D0] transition-all" style={{ width: (pct != null ? pct : 30) + '%' }} />
                        </div>
                      </div>
                    )}
                    {su.error && <div className="text-[13px] text-[#EA4335] mb-3">❌ {su.error}</div>}
                    <div className="flex items-center justify-end gap-2">
                      <button onClick={() => bridge.voice.cancelVoiceAsrSetup()} disabled={su.cancelling}
                        className={`text-[13px] px-4 py-2 rounded-full ${isDark ? 'bg-[#333537] hover:bg-[#444746]' : 'bg-[#E1E5EA] hover:bg-[#D3D9E0]'} ${su.cancelling ? 'opacity-50' : ''}`}>
                        {su.installing ? (su.cancelling ? chatCopy.cancelling : chatCopy.cancelDownload) : chatCopy.cancel}</button>
                      {!su.installing && (
                        <button onClick={() => bridge.voice.installVoiceAsr()} disabled={!su.status?.installable}
                          className={`text-[13px] font-medium px-4 py-2 rounded-full ${isDark ? 'bg-[#A8C7FA] text-[#041E49] hover:bg-[#C2D7FB]' : 'bg-[#0B57D0] text-white hover:bg-[#1967D2]'} ${!su.status?.installable ? 'opacity-50' : ''}`}>
                          {!su.status?.installable ? chatCopy.repairInstall : (needModel ? chatCopy.downloadModel : chatCopy.install)}</button>
                      )}
                    </div>
                  </div>
                </div>
              );
            })()}
            <div className="bg-white/80 dark:bg-[#161618]/85 backdrop-blur-2xl border border-black/[0.06] dark:border-white/10 rounded-[28px] shadow-lg focus-within:border-blue-400/50 dark:focus-within:border-blue-500/50 transition-colors px-4 pt-3 pb-2.5">
              {sceneCapabilityStatus && (
                <div
                  data-testid="scene-capability-status"
                  className={`mb-2 flex items-center gap-2 rounded-2xl px-3 py-2 text-[13px] ${
                    sceneCapabilityStatus.kind === 'error'
                      ? (isDark ? 'bg-[#3A1F1F] text-[#F28B82]' : 'bg-[#FCE8E6] text-[#C5221F]')
                      : sceneCapabilityStatus.kind === 'ready'
                        ? (isDark ? 'bg-[#10281D] text-[#81C995]' : 'bg-[#E6F4EA] text-[#137333]')
                        : (isDark ? 'bg-[#1E2B3A] text-[#A8C7FA]' : 'bg-[#E8F0FE] text-[#174EA6]')
                  }`}
                >
                  <span className={`h-2 w-2 shrink-0 rounded-full ${
                    sceneCapabilityStatus.kind === 'error'
                      ? 'bg-[#EA4335]'
                      : sceneCapabilityStatus.kind === 'ready'
                        ? 'bg-[#34A853]'
                        : 'bg-[#1A73E8] animate-pulse'
                  }`} />
                  <span className="min-w-0 truncate">{sceneCapabilityStatus.text}</span>
                </div>
              )}
              {!scheduledRunContext && !conversationStarted && activeScene && (
                <SceneModeTag
                  isDark={isDark}
                  scene={activeScene}
                  onClear={handleClearActiveScene}
                  clearLabel={sceneCopy.clear(activeScene.label)}
                />
              )}
              <ConversationActivityIndicator
                turn={activeConversationTurn}
                now={conversationNow}
                onRequestAttention={scrollChatToBottom}
                className="mb-0.5"
                copy={t.uiConversation}
              />
              {isMultiAgentReadOnly ? (
                <div
                  role="note"
                  data-testid="multiagent-desktop-only"
                  className={`min-h-[48px] px-1 py-3 text-[13px] leading-5 ${
                    isDark ? 'text-[#9AA0A6]' : 'text-[#5F6368]'
                  }`}
                >
                  {t.multiAgentDesktopOnly}
                </div>
              ) : (
                <>
              <textarea
                ref={composerRef}
                data-testid="chat-composer-input"
                value={inputText}
                onChange={e => handleComposerInputChange(e.target.value)}
                onKeyDown={handleKeyDown}
                onPaste={handlePaste}
                maxLength={CHAT_INPUT_MAX_LENGTH}
                placeholder={composerPlaceholder}
                rows={1}
                className="w-full bg-transparent resize-none outline-none text-gray-800 dark:text-gray-100 text-[16px] leading-relaxed min-h-[48px] overflow-y-auto hide-scrollbar placeholder:text-gray-400 dark:placeholder:text-gray-500"
              />
              <TextareaContextMenu inputRef={composerRef} setValue={setInputText} theme={theme} t={t} />
              {inputLimitReached && (
                <div role="status" aria-live="polite" data-testid="chat-input-limit-notice"
                  className={`px-1 pb-1 text-[12px] ${isDark ? 'text-[#F28B82]' : 'text-[#C5221F]'}`}>
                  {t.chatInputLimitReached(CHAT_INPUT_MAX_LENGTH.toLocaleString())}
                </div>
              )}
              <div className="flex items-center justify-between mt-1.5 gap-2">
                <div className="flex items-center gap-1.5 min-w-0 flex-1">
                  <ComposerAttachButton t={t} compact={composerCompact} />
                  <button onClick={handleVoiceClick} disabled={primaryVoiceDisabled} data-testid="composer-voice-button" aria-label={primaryVoiceLabel} title={primaryVoiceLabel}
                    className={`${
                      voiceRecording
                        ? 'w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-colors bg-[#C5221F] text-white hover:bg-[#A50E0E] border border-transparent'
                        : voiceActive
                          ? `${COMPOSER_ICON_BUTTON_CLASS} text-[#174EA6] dark:text-[#A8C7FA]`
                          : COMPOSER_ICON_BUTTON_CLASS
                    } ${primaryVoiceDisabled ? 'opacity-70 cursor-wait' : ''}`}>
                    <Mic size={18} />
                  </button>
                  <ComposerModeChip t={t} bs={bs} compact={composerCompact} />
                  <ComposerModelSelector t={t} bs={bs} onGotoSettings={onGotoModelSettings || onGotoSettings} compact={composerCompact} />
                  <ComposerToolMenu t={t} onGotoTools={onGotoTools} sessionId={bs && bs.activeSessionId} compact={composerCompact} activeSkill={bs && bs.activeSkill} />
                  <ComposerKbSelector t={t} bs={bs} compact={composerCompact} />
                </div>
                {!tabletVoiceMode && hasDraftText && (
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
                  const ready = canSend && !sceneCapabilityPreparing;
                  return (
                    <button onClick={handleSend} disabled={!ready} aria-label={t.sendMsg} title={t.sendMsg}
                      className={`w-9 h-9 shrink-0 rounded-full flex items-center justify-center transition-all ${ready ? 'bg-gradient-to-b from-[#47A1FF] to-[#007AFF] text-white shadow-md hover:-translate-y-0.5 active:translate-y-0' : 'bg-black/5 dark:bg-white/10 text-gray-400 cursor-not-allowed'}`}>
                      <Send size={17} className="translate-x-[1px]" />
                    </button>
                  );
                })()}
              </div>
                </>
              )}
            </div>
            {ctxTokens && ctxTokens.max > 0 && (
              <div className={`mt-1.5 px-5 text-[11px] font-mono ${
                ctxPct >= 0.9 ? 'text-[#C5221F] dark:text-[#F28B82]'
                : ctxPct >= 0.75 ? 'text-[#B06000] dark:text-[#F9AB00]'
                : 'text-[#9AA0A6] dark:text-[#5F6368]'}`}>
                {t.ctxUsage} {ctxTokens.input > 0 ? fmtCtxTok(ctxTokens.input) : '—'} / {fmtCtxTok(ctxTokens.max)} · {Math.round(ctxPct * 100)}%
              </div>
            )}
            <div className="flex items-center justify-center mt-3">
               <p data-testid="chat-disclaimer" className={`text-[12px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{t.disclaimer}</p>
            </div>
            </div>
          </div>
          </div>{/* /对话列 */}

          {artifactsVisible && artifactsFullscreen && createPortal(
            <div
              ref={artColRef}
              className="fixed left-0 right-0 bottom-0 z-[1000] pointer-events-auto"
              style={{ top: can('desktopChrome') ? '36px' : 0 }}
              data-testid="artifact-fullscreen-panel">
              <ArtifactsPanel
                bs={bs}
                theme={theme}
                t={t}
                onClose={closeArtifactsPanel}
                isWide={true}
                isFullscreen={true}
                onToggleFullscreen={() => setArtifactsFullscreen(false)}
                preferredArtifactPath={activeArtifactPath}
                onPreviewArtifact={handlePreviewArtifact}
                onGotoSettings={onGotoSettings}
                designMode={pinvouMode === 'design'}
                designCommand={designCommand}
                selectedDesignElement={selectedDesignElement}
                designChanges={visibleDesignChanges}
                onDesignRuntimeStatus={handleDesignRuntimeStatus}
                onDesignElementSelected={handleDesignElementSelected}
                onDesignChangeApplied={handleDesignChangeApplied}
                onDesignMutation={handleDesignMutation}
                onDesignApplyChange={handleApplyDesignChange}
                onDesignClearChanges={handleClearDesignChanges}
                onDesignAiSubmit={handleDesignAiSubmit}
                designAiState={designAiState}
                onDesignAiStateChange={updateDesignAiState}
              />
            </div>,
            document.body
          )}

          {artifactsVisible && isWide && !artifactsFullscreen && (
            <>
              <div onMouseDown={startArtifactDrag} onDoubleClick={resetArtifactW} role="separator" aria-orientation="vertical"
                className={`shrink-0 w-1.5 h-full cursor-col-resize transition-colors ${isDark ? 'bg-white/10 hover:bg-[#A8C7FA]/60' : 'bg-black/10 hover:bg-[#0B57D0]/50'}`} />
              <div
                ref={artColRef}
                className="shrink-0 h-full relative"
                style={{ width: artifactW + 'px' }}>
                <ArtifactsPanel
                  bs={bs}
                  theme={theme}
                  t={t}
                  onClose={closeArtifactsPanel}
                  isWide={true}
                  isFullscreen={false}
                  onToggleFullscreen={() => setArtifactsFullscreen(true)}
                  preferredArtifactPath={activeArtifactPath}
                  onPreviewArtifact={handlePreviewArtifact}
                  onGotoSettings={onGotoSettings}
                  designMode={pinvouMode === 'design'}
                  designCommand={designCommand}
                  selectedDesignElement={selectedDesignElement}
                  designChanges={visibleDesignChanges}
                  onDesignRuntimeStatus={handleDesignRuntimeStatus}
                  onDesignElementSelected={handleDesignElementSelected}
                  onDesignChangeApplied={handleDesignChangeApplied}
                  onDesignMutation={handleDesignMutation}
                  onDesignApplyChange={handleApplyDesignChange}
                  onDesignClearChanges={handleClearDesignChanges}
                  onDesignAiSubmit={handleDesignAiSubmit}
                  designAiState={designAiState}
                  onDesignAiStateChange={updateDesignAiState}
                />
              </div>
            </>
          )}
          {artifactsVisible && !isWide && !artifactsFullscreen && (
            <ArtifactsPanel
              bs={bs}
              theme={theme}
              t={t}
              onClose={closeArtifactsPanel}
              isWide={false}
              isFullscreen={false}
              onToggleFullscreen={() => setArtifactsFullscreen(true)}
              preferredArtifactPath={activeArtifactPath}
              onPreviewArtifact={handlePreviewArtifact}
              onGotoSettings={onGotoSettings}
              designMode={pinvouMode === 'design'}
              designCommand={designCommand}
              selectedDesignElement={selectedDesignElement}
              designChanges={visibleDesignChanges}
              onDesignRuntimeStatus={handleDesignRuntimeStatus}
              onDesignElementSelected={handleDesignElementSelected}
              onDesignChangeApplied={handleDesignChangeApplied}
              onDesignMutation={handleDesignMutation}
              onDesignApplyChange={handleApplyDesignChange}
              onDesignClearChanges={handleClearDesignChanges}
              onDesignAiSubmit={handleDesignAiSubmit}
              designAiState={designAiState}
              onDesignAiStateChange={updateDesignAiState}
            />
          )}
          {subagentPanel && (
            <SubagentTranscriptPanel
              sessionId={activeSessionId}
              initialAgentId={subagentPanel.agentId}
              selectionRequestId={subagentPanel.selectionRequestId}
              t={t}
              theme={theme}
              onClose={closeSubagentPanel}
            />
          )}
        </div>
      );
    };

    // ==========================================
    // Chat Bubble (message rendering)
    // ==========================================
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
            <span className="w-4 text-center text-[12px]">A</span><span>{t.selectAllMsg}</span>
          </button>
          <button type="button" disabled={!menu.canCopy} className={menuItemCls(!menu.canCopy)} onClick={copySelected}>
            <Copy size={14} /><span>{t.copyMsg}</span>
          </button>
          <button type="button" className={menuItemCls(false)} onClick={pasteText}>
            <ClipboardList size={14} /><span>{t.pasteMsg}</span>
          </button>
        </div>
      ), document.body);
    };

    const UserBubble = ({ item, sessionId, theme, editable, t, conversationVariant }) => {
      const isDark = theme === 'dark';
      const unified = conversationVariant === 'unified';
      const deliveryState = item.deliveryState || '';
      const sceneDisplay = pinvouSceneDisplay(item.pinvouScene, t.uiChat.sceneModes);
      const SceneIcon = sceneDisplay && sceneDisplay.Icon;
      const [editing, setEditing] = useState(false);
      const [val, setVal] = useState(item.text);
      const [copied, setCopied] = useState(false);
      function commit() { const tx = val.trim(); setEditing(false); if (tx && bridge.available) bridge.interaction.editLastTurn(tx); }
      function copyText() {
        const tx = item.text || '';
        copyClipboardText(tx).then(function (ok) {
          if (!ok) return;
          setCopied(true);
          setTimeout(function () { setCopied(false); }, 1200);
        });
      }
      function retryDelivery() {
        if (!item.clientMessageId || !bridge.available || !bridge.chat.retryFirstTurn) return;
        bridge.chat.retryFirstTurn(item.clientMessageId);
      }
      if (editing) {
        return (
          <div className="flex justify-end min-w-0 max-w-full">
            <div className="max-w-[85%] w-full min-w-0">
              <textarea autoFocus value={val} onChange={e => setVal(e.target.value)}
                rows={Math.min(6, Math.max(1, val.split('\n').length))}
                onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey && !isImeComposing(e)) { e.preventDefault(); commit(); } else if (e.key === 'Escape') { setEditing(false); setVal(item.text); } }}
                className={`w-full min-w-0 max-w-full break-words [overflow-wrap:anywhere] rounded-[16px] px-4 py-2 text-[15px] outline-none ${
                  unified
                    ? (isDark ? 'bg-[#2A2B2E] text-[#E3E3E3]' : 'bg-[#E9EEF6] text-[#1F1F1F]')
                    : (isDark ? 'bg-[#004A77] text-[#E3E3E3]' : 'bg-[#D3E3FD] text-[#1F1F1F]')
                }`} />
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
          <div className="flex justify-end min-w-0 max-w-full">
            <div className="max-w-[85%] min-w-0">
              <div className="flex items-center justify-end gap-1 mb-1 text-[11px] font-medium" style={{ color: tint }}>
                <span>{isWu ? '✨' : '📋'}</span><span>{t.uiChatExtra.transferRevision(item.pinvouTransfer)}</span>
              </div>
              <div className={`min-w-0 max-w-full break-words [overflow-wrap:anywhere] px-5 py-3 rounded-[20px] text-[15px] leading-relaxed whitespace-pre-wrap ${tintBg} ${isDark ? 'text-[#E3E3E3]' : 'text-[#1F1F1F]'}`}>{item.text}</div>
            </div>
          </div>
        );
      }
      const actBtn = isDark
        ? 'text-[#8E8E8E] hover:text-[#E3E3E3] hover:bg-white/10'
        : 'text-[#9AA0A6] hover:text-[#444746] hover:bg-black/[0.06]';
      // 附件行拆出正文,附件以独立小气泡显示在正文气泡上方(纯附件消息只显示附件气泡)
      const { text: bodyText, attachments: attachmentNames } = splitAttachmentLine(item.text);
      return (
        <div className="flex justify-end group min-w-0 max-w-full">
          <div className="flex flex-col items-end max-w-[85%] min-w-0 max-w-full">
            {attachmentNames.length > 0 && (
              <div className={`flex max-w-full flex-wrap justify-end gap-1.5 ${bodyText ? 'mb-1.5' : ''}`}>
                {attachmentNames.map((name, index) => {
                  return (
                    <ConversationAttachmentBubble
                      key={`${name}-${index}`}
                      name={name}
                      displayText={item.text}
                      messageIndex={item.messageIndex}
                      attachmentIndex={index}
                      sessionId={sessionId}
                      isDark={isDark}
                      copyText={copyClipboardText}
                      labels={{
                        open: t.attachmentOpen,
                        download: t.attachmentDownload,
                        copyAddress: t.attachmentCopyAddress,
                        copyName: t.attachmentCopyName,
                        reveal: t.attachmentReveal,
                      }}
                    />
                  );
                })}
              </div>
            )}
            {bodyText && <div className={`min-w-0 max-w-full break-words [overflow-wrap:anywhere] px-4 py-3 rounded-[20px] rounded-br-md text-[14px] leading-6 whitespace-pre-wrap ${
              unified
                ? (isDark ? 'bg-[#2A2B2E] text-[#E3E3E3]' : 'bg-[#E9EEF6] text-[#1F1F1F]')
                : (isDark ? 'bg-[#004A77] text-[#E3E3E3]' : 'bg-[#D3E3FD] text-[#1F1F1F]')
            }`}>
              {sceneDisplay && (
                <span
                  data-testid="user-message-scene-tag"
                  className={`mr-2 inline-flex align-middle items-center gap-1 rounded-full px-2 py-0.5 text-[12px] font-semibold leading-5 ${
                    isDark ? 'bg-black/35 text-white' : 'bg-black text-white'
                  }`}
                >
                  {SceneIcon && <SceneIcon size={14} className="shrink-0" />}
                  <span>{sceneDisplay.label}</span>
                </span>
              )}
              {bodyText}
            </div>}
            {deliveryState && (
              <div data-testid={`message-delivery-${deliveryState}`} title={item.deliveryError || undefined} className={`mt-1 flex items-center gap-1.5 pr-1 text-[11px] ${
                deliveryState === 'failed' || deliveryState === 'unknown'
                  ? (isDark ? 'text-[#F28B82]' : 'text-[#C5221F]')
                  : deliveryState === 'accepted'
                    ? (isDark ? 'text-[#81C995]' : 'text-[#188038]')
                    : (isDark ? 'text-[#9AA0A6]' : 'text-[#747775]')
              }`}>
                {deliveryState === 'sending' && <span className="h-1.5 w-1.5 rounded-full bg-current animate-pulse" />}
                <span>
                  {deliveryState === 'sending'
                    ? t.messageSending
                    : deliveryState === 'accepted'
                      ? t.messageAccepted
                      : deliveryState === 'unknown'
                        ? t.messageOutcomeUnknown
                        : t.messageFailed}
                </span>
                {deliveryState === 'failed' && (
                  <button type="button" onClick={retryDelivery} className="font-medium underline underline-offset-2">
                    {t.resend}
                  </button>
                )}
              </div>
            )}
            {/* iOS 风操作条：hover 气泡时下方浮现；窄屏无 hover，常显保证触屏可达。复制=所有 query；编辑重发=仅最新(editable)。 */}
            <div className="flex items-center gap-0.5 mt-1 pr-1 opacity-0 group-hover:opacity-100 max-sm:opacity-100 transition-opacity duration-150">
              <button title={copied ? t.copied : t.copyMsg} onClick={copyText}
                className={`w-7 h-7 rounded-lg flex items-center justify-center transition-colors ${actBtn}`}>
                {copied ? <Check size={14} className="text-[#34C759]" /> : <Copy size={14} />}
              </button>
              {editable && !deliveryState && (
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
    const ThinkingBubble = ({ thinking, theme, t, isLocal }) => {
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
        const suffix = elapsed >= 120 ? ` · ${t.hintSlow120(isLocal)}` : elapsed >= 30 ? ` · ${t.hintSlow30(isLocal)}` : '';
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
      return String(s).replace(/&lt;/g,'<').replace(/&gt;/g,'>').replace(/&quot;/g,'"').replace(/&#(?:39|x27);/gi,"'").replace(/&amp;/g,'&');
    }
    function highlightedCodeText(s) {
      return htmlUnescape(String(s).replace(/<\/?span\b[^>]*>/gi, ''));
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
        mode: 'yolo',
        paused: !!d.paused,
      };
    }
    // 扫所有 ```代码块,任何能解析成「含 name+body 的 JSON」的就当卡牌草稿。
    // 不强求 ```persona-card 标签 —— 小模型常打 ```json 或不打标签,放宽识别更鲁棒。
    // 形状校验(name+body)避免把别的 JSON 误判成草稿。明确 persona-card 标签的优先。
    // 返回 { draft, html }:html 是把那段原始 JSON 块抹掉后的版本(用户只看友好草稿卡,不看机器载荷)。
    function parsePersonaDraft(html) {
      if (!html || html.indexOf('{') < 0) return { draft: null, html: html };
      var re = /<pre([^>]*)>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m, chosen = null, chosenDraft = null;
      while ((m = re.exec(html))) {
        var raw = highlightedCodeText(m[3]).trim();
        if (raw.charAt(0) !== '{') continue;
        try {
          var draft = asDraft(parseLooseJson(raw));
          if (!draft) continue;
          if (/persona-card/i.test(m[1] + m[2])) { chosen = m[0]; chosenDraft = draft; break; } // 明确标签优先
          if (!chosenDraft) { chosen = m[0]; chosenDraft = draft; }
        } catch (e) { /* 非 JSON 块,跳过 */ }
      }
      if (!chosenDraft) return { draft: null, html: html };
      return { draft: chosenDraft, html: html.replace(chosen, '') };
    }
    function parseScheduledTaskDraft(html) {
      if (!html || html.indexOf('{') < 0) return { draft: null, html: html };
      var re = /<pre([^>]*)>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m, chosen = null, chosenDraft = null;
      while ((m = re.exec(html))) {
        var raw = highlightedCodeText(m[3]).trim();
        if (raw.charAt(0) !== '{') continue;
        var draft = asScheduledTaskDraft(parseLooseJson(raw));
        if (!draft) continue;
        if (/scheduled-task-draft/i.test(m[1] + m[2])) { chosen = m[0]; chosenDraft = draft; break; }
        if (!chosenDraft) { chosen = m[0]; chosenDraft = draft; }
      }
      if (!chosenDraft) return { draft: null, html: html };
      return { draft: chosenDraft, html: html.replace(chosen, '') };
    }
    // 卡牌制造专家追问时,若问题有可选项,会输出一个 ```card-question 块 {question, options[]}。
    // 抠出来 → 渲染成可点击的 iOS 选项卡;点选项即把它作为回答发送。返回 { q, html(抹掉块) }。
    function parseCardQuestion(html) {
      if (!html || !/card-question/i.test(html)) return { q: null, html: html };
      var re = /<pre([^>]*)>\s*<code([^>]*)>([\s\S]*?)<\/code>\s*<\/pre>/g, m;
      while ((m = re.exec(html))) {
        if (!/card-question/i.test(m[1] + m[2])) continue;
        var raw = highlightedCodeText(m[3]).trim();
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
      return html.slice(0, m.index) + '<div style="margin-top:.5em;opacity:.7;font-size:13px">' + (label || '…') + '</div>';
    }

    const ChatBubble = ({ item, sessionId, theme, onPrefill, onSend, editable, onOpenEditor, t, isLatestArtifact, allowScheduledTaskDraft, conversationVariant, showAssistantActions = true }) => {
      const isDark = theme === 'dark';
      const chatCopy = t.uiChat;
      const chatViewCopy = t.uiChatView;
      // 后端持久化的记忆状态值是固定中文数据，仅在 UI 边界映射为当前语言；未识别值原样透传
      const memoryStatusLabels = {
        '已忽略': chatCopy.ignoreOnce,
        '不再提示': chatCopy.neverAsk,
        '已记住': chatViewCopy.memStatusRemembered,
        '已归档': chatViewCopy.memStatusArchived,
        '已删除': chatViewCopy.memStatusDeleted,
        '记忆已更新': chatCopy.memoryUpdated,
        '记忆已归档': chatViewCopy.memStatusArchivedNotice,
        '记忆已删除': chatViewCopy.memStatusDeletedNotice,
      };
      const localizedMemoryStatus = (label) => memoryStatusLabels[label] || label;
      const assistantSelectionHostRef = useRef(null);
      const assistantSelectionTargetRef = useRef(null);

      if (item.type === 'artifact_card') return <ArtifactCard item={item} theme={theme} t={t} isLatest={isLatestArtifact} />;
      if (item.type === 'plan_card') return <PlanCard item={item} theme={theme} t={t} onPrefill={onPrefill} />;
      if (item.type === 'plan_stuck') return <PlanStuckCard item={item} theme={theme} t={t} />;
      if (item.type === 'careful_blocked') return <CarefulBlockedCard item={item} theme={theme} t={t} />;
      if (item.type === 'user_input') return <UserInputCard item={item} theme={theme} t={t} />;
      if (item.type === 'user') {
        return <UserBubble item={item} sessionId={sessionId} theme={theme} editable={editable} t={t} conversationVariant={conversationVariant} />;
      }

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
        const streamingDraftLabel = /scheduled-task-draft/.test(html) ? t.uiChatExtra.draftingScheduled : (t && t.cpDesigning);
        const pd = item.streaming ? { draft: null, html: hideStreamingDraft(html, streamingDraftLabel) } : parsePersonaDraft(html);
        const sd = (item.streaming || !allowScheduledTaskDraft) ? { draft: null, html: pd.html } : parseScheduledTaskDraft(pd.html);
        const cq = item.streaming ? { q: null, html: sd.html } : parseCardQuestion(sd.html);
        const assistantCopyAvailable = !item.streaming
          && [item.text, item.html].some(value => String(value || '').trim());
        // 草稿是否已存入(按名字在已加载的卡池里找同名自制卡 → 派生"已存入",免单独持久化)
        const draftSaved = pd.draft && bridge.available && bridge.personas.getPersonas
          && bridge.personas.getPersonas().some(function(c){ return c && c.source === 'user' && c.name === pd.draft.name; });
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
                    invokeTauri('open_user_external_url', { url: href }).catch(() => {});
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
              {showAssistantActions && assistantCopyAvailable && <AssistantMessageFooter>
                <AssistantMessageActions
                  resolveText={() => assistantItemCopyText(item, { allowScheduledTaskDraft })}
                  copy={t.uiConversation}
                />
                {item.time && <span className={`text-[11px] ${isDark ? 'text-[#8E8E8E]' : 'text-[#757575]'}`}>{item.time}</span>}
              </AssistantMessageFooter>}
            </div>
          </div>
        );
      }

      if (item.type === 'tool') {
        return <ToolCard item={item} sessionId={sessionId} theme={theme} t={t} />;
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
        const memoryKind = item.kind === 'recent_work' ? 'recent_activity' : item.kind;
        const meta = chatCopy.memoryMeta[memoryKind] || chatCopy.memoryMeta.preference;
        if (quietNotice) {
          return (
            <div className="flex justify-center">
              <div
                className="inline-flex items-center gap-1.5 max-w-[360px] px-3 py-1.5 rounded-full text-[12px] text-[#AEB4BC]"
                title={text}
                style={{ background: 'rgba(32, 34, 38, 0.54)', border: '1px solid rgba(255,255,255,0.06)' }}
              >
                <Check size={12} className="shrink-0 text-[#30D158]" />
                <span className="font-medium text-[#D5D9DE]">{chatCopy.recordedRecent}</span>
                <span className="truncate">{chatCopy.viewMemory}</span>
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
                    <span className="text-[13px] font-semibold leading-tight">{localizedMemoryStatus(item.statusLabel) || chatCopy.memoryUpdated}</span>
                    <span className="text-[11px] px-2 py-0.5 rounded-full bg-white/[0.07] text-[#AEB4BC]">{meta.label}</span>
                  </div>
                  <div className="mt-1 text-[12px] leading-relaxed text-[#AEB4BC]">{meta.notice}</div>
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
        const memoryKind = item.kind === 'recent_work' ? 'recent_activity' : item.kind;
        const meta = chatCopy.memoryMeta[memoryKind] || chatCopy.memoryMeta.preference;
        const localizedStatus = localizedMemoryStatus(item.statusLabel);
        return (
          <div className="flex justify-end">
            <div
              data-testid="memory-candidate-card"
              data-memory-id={item.memoryId || ''}
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
                    <div className="text-[13px] font-semibold leading-tight">{resolved ? (localizedStatus || chatCopy.processed) : chatCopy.candidate}</div>
                    {!resolved && <div className="text-[12px] leading-tight mt-1 text-[#AEB4BC]">{meta.prompt}</div>}
                  </div>
                </div>
                <div className="shrink-0 flex items-center gap-2">
                  <span className="text-[11px] px-2 py-1 rounded-full bg-white/[0.07] text-[#AEB4BC]">{meta.label}</span>
                  {!resolved && (
                    <button
                      className="w-6 h-6 rounded-full flex items-center justify-center text-[#8E8E93] hover:text-[#F2F3F5] hover:bg-white/[0.08] transition-colors"
                      title={chatCopy.ignoreOnce}
                      data-testid="memory-candidate-dismiss"
                      onClick={() => bridge.available && bridge.memory.ignoreMemoryCandidate && bridge.memory.ignoreMemoryCandidate(item.memoryId, item.id)}
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
                  <button data-testid="memory-candidate-confirm" className="inline-flex items-center gap-1.5 text-[13px] font-medium px-3.5 py-1.5 rounded-full bg-[#0A84FF] text-white hover:bg-[#1677D2] transition-colors" onClick={() => bridge.available && bridge.memory.confirmMemoryCandidate && bridge.memory.confirmMemoryCandidate(item.memoryId, item.id)}><Check size={14} />{chatCopy.remember}</button>
                  <button data-testid="memory-candidate-ignore" className="inline-flex items-center gap-1.5 text-[13px] px-3.5 py-1.5 rounded-full bg-white/[0.08] text-[#E8EAED] hover:bg-white/[0.12] transition-colors" onClick={() => bridge.available && bridge.memory.ignoreMemoryCandidate && bridge.memory.ignoreMemoryCandidate(item.memoryId, item.id)}><X size={14} />{chatCopy.ignoreOnce}</button>
                  <button data-testid="memory-candidate-never" className="text-[13px] px-2 py-1.5 rounded-full text-[#AEB4BC] hover:text-[#F2F3F5] hover:bg-white/[0.08] transition-colors" onClick={() => bridge.available && bridge.memory.neverMemoryCandidate && bridge.memory.neverMemoryCandidate(item.memoryId, item.id)}>{chatCopy.neverAsk}</button>
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
