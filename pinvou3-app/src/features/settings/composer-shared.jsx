// 输入框共享组件:模型选择器/工具菜单/技能菜单/产物 HTML 缩放预览。原生于
// SettingsView.jsx,但 ChatView(启动视图)与 CodexAcpView 也静态引用——只要
// 留在 SettingsView.jsx 里,SettingsView(3389 行)就永远进不了独立懒加载
// chunk。抽到本模块后 SettingsView 可整体懒加载,共享件随主 chunk 常驻。
// 组件实现与 SettingsView.jsx 原版逐字节一致。
import React, { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Cpu, Plus, Store, Users, Wrench, X, Zap } from '../../components/icons.jsx';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import { bridge, useBridgeState } from '../../hooks/useBridge.js';
import { visibleUserModels } from '../../shared/model-options.js';
import { can } from '../../shared/platform.js';
import { buildComposerToolMenuState } from './composer-tool-menu-logic.js';
import { invokeTauri } from '../../platform/tauri/client.js';
import {
  artifactPreviewExternalUrlFromMessage,
  buildArtifactPreviewDocument,
} from '../artifacts/artifact-preview-navigation.js';
import {
  groupModelsForSelector, selectorMainLabel, selectorSubLabel,
  reasoningEffortTiersForModel, normalizeStoredReasoningEffort,
  localProbeTiersForKind, baseUrlUsesLocalOrPrivate,
} from './model-catalog.js';

    // 输入框底栏:模型选择器(iOS 化;darkMode:'class' 故用 dark: 变体)。
    // 可选“显式会话态驱动”props（代码模块原生车道用）：sessionId/sessionModelId/
    // busy/onSwitchModel 传入时绕开 bridge 聊天 active 绑定；不传走原 bs/bridge 路径。
    const ComposerModelSelector = ({
      t,
      bs,
      onGotoSettings,
      compact,
      sessionId: sessionIdProp,
      sessionModelId: sessionModelIdProp,
      busy: busyProp,
      onSwitchModel,
      multiAgentEnabled: multiAgentEnabledProp,
      multiAgentAvailable: multiAgentAvailableProp,
      onToggleMultiAgent,
    }) => {
      const [open, setOpen] = useState(false);
      const triggerRef = useRef(null);
      const canManageModels = can('modelManagement');
      const canSwitchModels = can('sessionModelSwitch');
      // 多智能体模式 = 模型列表下方的会话级开关（ADR-0006）。状态权威在
      // 后端 mode_state，这里只读 bs 镜像；翻转后 bridge 回写权威状态。
      const canMultiAgent = can('multiAgent') && multiAgentAvailableProp !== false;
      const multiAgentOn = multiAgentEnabledProp !== undefined
        ? Boolean(multiAgentEnabledProp)
        : !!(bs && bs.modeState && bs.modeState.multiAgent);
      const multiAgentCopy = (t && t.uiMultiAgent) || {};
      // 防重入（复核点名）：后端事务完成前再点会带着旧状态重复提交，
      // 其中一次名册推送失败的回滚还会覆盖另一次已开启的状态。切换期间
      // 禁用按钮；bridge 侧另有 in-flight 丢弃兜底（双入口防线）。
      const [multiAgentBusy, setMultiAgentBusy] = useState(false);
      // 揭幕动效只在用户点击开启这一刻播放：会话切换/重启同步出现的
      // 开启态、弹层关了再开，都不重播揭幕（真机点名），但光晕持续漂移
      // （真机点名"动画不能停"，挂在常态类上）。触发源是点击处理器而
      // 不是状态上升沿；点击关闭或弹层关闭时清掉标记，避免误续播。
      const [multiAgentRevealing, setMultiAgentRevealing] = useState(false);
      useEffect(() => {
        if (!open) setMultiAgentRevealing(false);
      }, [open]);
      async function toggleMultiAgent() {
        if (multiAgentBusy || busy) return;
        if (!onToggleMultiAgent
          && !(bridge.available && bridge.interaction && bridge.interaction.setMultiAgentMode)) return;
        setMultiAgentRevealing(!multiAgentOn);
        setMultiAgentBusy(true);
        try {
          if (onToggleMultiAgent) await onToggleMultiAgent(!multiAgentOn);
          else await bridge.interaction.setMultiAgentMode(!multiAgentOn);
        } finally {
          setMultiAgentBusy(false);
        }
      }
      const savedModels = visibleUserModels((bs && bs.savedModels) || []);
      const activeSessionId = sessionIdProp !== undefined ? sessionIdProp : (bs ? bs.activeSessionId : null);
      const activeModelId = bs && bs.activeModelId;
      const currentSessionModelId = sessionModelIdProp !== undefined ? sessionModelIdProp : (bs && bs.currentSessionModelId);
      const busy = busyProp !== undefined ? busyProp : (bs ? bs.busy : false);
      const effectiveId = currentSessionModelId || activeModelId;
      const current = savedModels.find(m => m.id === effectiveId);
      const [currentProbedKind, setCurrentProbedKind] = useState(null);
      const [currentProbePending, setCurrentProbePending] = useState(false);
      // 本地/私网 openai_compatible 端点：探测服务类型，按探测结果下发真实档位
      // （vllm→四档、ollama→off/high、lmstudio/generic→不支持提示）。
      const isLocalCompatible = current && current.preset === 'openai_compatible' && baseUrlUsesLocalOrPrivate(current.base_url || '');
      useEffect(() => {
        if (!isLocalCompatible) {
          setCurrentProbedKind(null);
          setCurrentProbePending(false);
          return;
        }
        let cancelled = false;
        setCurrentProbePending(true);
        setCurrentProbedKind(null);
        if (bridge.available && bridge.models && bridge.models.probeLocalServerKind) {
          bridge.models.probeLocalServerKind(current.base_url)
            .then((kind) => { if (!cancelled) setCurrentProbedKind(kind || 'generic'); })
            .catch(() => { if (!cancelled) setCurrentProbedKind('generic'); })
            .finally(() => { if (!cancelled) setCurrentProbePending(false); });
        } else {
          if (!cancelled) setCurrentProbePending(false);
        }
        return () => { cancelled = true; };
      }, [isLocalCompatible, current && current.base_url]);
      const reasoningEffortTiers = isLocalCompatible
        ? (currentProbePending ? [] : (localProbeTiersForKind(currentProbedKind) || []))
        : (current ? (reasoningEffortTiersForModel(current) || []) : []);
      // 存量档位（可能保存过底座归一前的旧值，如 deepseek 的 medium）先归一到
      // 档位表内等价档位再高亮，避免「档位表不含该值 → 下拉无高亮」。
      const reasoningEffortValue = current ? normalizeStoredReasoningEffort(current, current.reasoning_effort) : null;
      const [effortSaveError, setEffortSaveError] = useState('');
      function setReasoningEffortForCurrent(tier) {
        if (!current) return;
        if (tier === reasoningEffortValue) return;
        setEffortSaveError('');
        const next = { ...current, reasoning_effort: tier };
        if (!bridge.available) { setOpen(false); return; }
        // 保存成功才收弹层；失败保留弹层以便展示 effortSaveError（否则错误渲染
        // 在已关闭的 popover 内不可达）。
        bridge.models.saveModel(next)
          .then(() => { setOpen(false); setEffortSaveError(''); })
          .catch((error) => {
            const message = (error && error.message)
              ? error.message
              : ((t && t.saveModelFailed) || '保存失败');
            setEffortSaveError(message);
          });
      }
      if (!savedModels.length) return null;
      function pick(id) {
        setOpen(false);
        setEffortSaveError('');
        if (id === effectiveId) return;
        if (onSwitchModel) { onSwitchModel(activeSessionId, id); return; }
        if (bridge.available) bridge.models.switchModel(activeSessionId, id);
      }
      return (
        <div className="relative min-w-0">
          <button ref={triggerRef} onClick={() => { if (!busy && canSwitchModels) setOpen(o => !o); }} disabled={busy || !canSwitchModels}
            title={(current ? selectorMainLabel(current, t) : t.modelNonePick) + (busy ? ' · ' + t.modelSwitchBusy : '')}
            className={`relative shrink-0 flex items-center justify-center ${multiAgentOn ? 'text-[#6d28d9] dark:text-[#c4b5fd]' : 'text-gray-700 dark:text-gray-200'} transition-colors border disabled:opacity-50 ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] font-semibold min-w-0 max-w-full bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] border-black/[0.045] dark:border-white/[0.06]'}`}>
            {compact ? (
              <>
                <Cpu size={18} className="opacity-80" />
                <span className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[#34C759] ring-2 ring-white dark:ring-[#161618]"></span>
              </>
            ) : (
              <>
                <span className="w-1.5 h-1.5 shrink-0 rounded-full bg-[#34C759]"></span>
                <span className="max-w-[116px] truncate">{t.composerModelLabel(current ? selectorMainLabel(current, t) : t.modelNonePick)}</span>
                <ChevronDown size={13} className="opacity-50 shrink-0" />
              </>
            )}
          </button>
          <ComposerPopover open={open && canSwitchModels} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
            desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-64 max-h-[340px] overflow-y-auto bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {(() => {
                  const { preset, custom } = groupModelsForSelector(savedModels);
                  const renderGroup = (label, items, withDivider) => items.length > 0 && (
                    <>
                      {withDivider && <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />}
                      <div className="px-3 pt-1.5 pb-1 text-[11px] font-semibold text-gray-400 dark:text-gray-500">{label}</div>
                      {items.map(m => (
                        <button key={m.id} onClick={() => pick(m.id)}
                          className="w-full flex items-center justify-between gap-2 px-3 py-2 text-left rounded-xl transition-colors group hover:bg-[#007AFF] hover:text-white">
                          <span className="flex items-center gap-2.5 min-w-0">
                            <Cpu size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                            <span className="min-w-0">
                              <span className="block text-[13px] truncate text-gray-700 dark:text-gray-200 group-hover:text-white">{selectorMainLabel(m, t)}</span>
                              <span className="block text-[11px] truncate text-gray-400 dark:text-gray-500 group-hover:text-white/80">{selectorSubLabel(m, t)}</span>
                            </span>
                          </span>
                          {m.id === effectiveId && <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />}
                        </button>
                      ))}
                    </>
                  );
                  return (
                    <>
                      {renderGroup(t.modelGroupPreset, preset, false)}
                      {renderGroup(t.modelGroupCustom, custom, preset.length > 0)}
                    </>
                  );
                })()}
                {current && (reasoningEffortTiers.length > 0 || isLocalCompatible) && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <div className="px-3 pt-1 pb-1">
                      <div className="text-[11px] font-semibold text-gray-400 dark:text-gray-500 mb-1.5">{t.thinkingDepth}</div>
                      {reasoningEffortTiers.length > 0 ? (
                        <div className="flex flex-wrap gap-1">
                          {reasoningEffortTiers.map(tier => (
                            <button key={tier} onClick={() => setReasoningEffortForCurrent(tier)}
                              className={`h-7 min-w-[48px] px-2.5 rounded-full text-[12px] font-medium transition-colors ${
                                reasoningEffortValue === tier
                                  ? 'bg-[#007AFF] text-white'
                                  : 'bg-black/[0.05] dark:bg-white/[0.08] text-gray-600 dark:text-gray-300 hover:bg-black/[0.09] dark:hover:bg-white/[0.13]'
                            }`}>
                              {t.thinkingDepthTiers[tier] || tier}
                            </button>
                          ))}
                        </div>
                      ) : (
                        <div className={`text-[11px] leading-4 ${currentProbePending ? 'text-gray-400 dark:text-gray-500' : 'text-[#FF9500] dark:text-[#FFB340]'}`}>
                          {currentProbePending
                            ? ((t.uiSettingsDetail && t.uiSettingsDetail.reasoningProbePending) || '正在探测服务类型…')
                            : ((t.uiSettingsDetail && t.uiSettingsDetail.reasoningProbeUnsupported) || '该端点不支持思考档位调节')}
                        </div>
                      )}
                      {effortSaveError && (
                        <div className="mt-1.5 text-[11px] leading-4 text-[#FF3B30] dark:text-[#FF6B6B]">{effortSaveError}</div>
                      )}
                    </div>
                  </>
                )}
                {canMultiAgent && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <button type="button" data-testid="multiagent-toggle" onClick={toggleMultiAgent}
                      disabled={multiAgentBusy || busy}
                      title={multiAgentCopy.toggleHint || ''}
                      onAnimationEnd={(event) => {
                        if (event.animationName === 'pinvou-ultra-reveal') setMultiAgentRevealing(false);
                      }}
                      className={`w-full flex items-center justify-between px-3 py-2.5 text-[13px] rounded-xl ${
                        multiAgentOn
                          ? 'pinvou-ultra-row'
                          : 'text-gray-700 dark:text-gray-200 hover:bg-black/[0.045] dark:hover:bg-white/[0.07]'
                      } ${multiAgentRevealing ? 'pinvou-ultra-row-reveal' : ''}`}>
                      <span className="flex items-center gap-2.5 min-w-0">
                        <Users size={15} className={`shrink-0 ${multiAgentOn ? 'text-current' : 'text-gray-400'}`} />
                        <span className={`truncate ${multiAgentOn ? 'font-medium' : ''}`}>{multiAgentCopy.toggleLabel || ''}</span>
                      </span>
                      <span aria-hidden="true" className={`relative shrink-0 w-8 h-[18px] rounded-full transition-colors ${multiAgentOn ? 'bg-white/30' : 'bg-black/20 dark:bg-white/25'}`}>
                        <span className={`absolute top-[2px] left-0 w-[14px] h-[14px] rounded-full bg-white shadow transition-transform ${multiAgentOn ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
                      </span>
                    </button>
                  </>
                )}
                {canManageModels && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <button onClick={() => { setOpen(false); if (onGotoSettings) onGotoSettings(); }}
                      className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                      <Plus size={15} className="text-gray-400 group-hover:text-white/90" />
                      {t.manageModels}
                    </button>
                  </>
                )}
          </ComposerPopover>
        </div>
      );
    };

    // 产物 HTML 预览：测内容自然尺寸，比面板宽就整体等比缩小铺满（只缩不放）。
    // 治"固定尺寸 banner 在窄预览面板里溢出、出滚动条、只露一角"。响应式整页缩放比≈1、不受影响。
    const clampPreviewScale = value => Math.max(0.1, Math.min(3, Number(value) || 1));
    const ScaledHtmlPreview = ({ html, onFrameLoad, onOpenExternal, zoomMode = 'auto-width', customScale = 1, onScaleChange, onCustomScaleChange }) => {
      const wrapRef = useRef(null);
      const frameRef = useRef(null);
      const naturalRef = useRef(null); // 当前 html 的内容自然尺寸缓存(在面板参考宽度下测得)
      const [box, setBox] = useState(null); // { w, h, scale }
      const [ready, setReady] = useState(false);
      const managedZoom = zoomMode !== 'auto-width';
      const canvasW = managedZoom ? 1440 : null;
      const measure = () => {
        try {
          const fr = frameRef.current, wrap = wrapRef.current;
          if (!fr || !wrap || !fr.contentWindow) return;
          const doc = fr.contentWindow.document;
          const de = doc.documentElement, bd = doc.body;
          const panelW = wrap.clientWidth;
          const panelH = wrap.clientHeight;
          // 内容自然尺寸只在面板参考宽度下测量一次并缓存；后续仅依据面板尺寸重算缩放。
          // 若把「已按自然宽度撑开的 iframe 视口」每次再喂回测量，弹层里的 vw/vh 与
          // 溢出内容（如 right:-120px 的绝对定位元素）会让 scrollWidth/scrollHeight 随 iframe
          // 被撑大而继续放大，触发 ResizeObserver 无限反馈 → 预览无限放大。
          let nat = naturalRef.current;
          if (!nat) {
            const cw = Math.max(de ? de.scrollWidth : 0, bd ? bd.scrollWidth : 0);
            const ch = Math.max(de ? de.scrollHeight : 0, bd ? bd.scrollHeight : 0);
            nat = { w: cw, h: ch };
            // iframe 内容可能尚未真正加载(scrollWidth=0 或 scrollHeight=0)：这种空测量不写入缓存，
            // 等 onLoad 后测到真实尺寸再缓存，避免把首轮空值固化导致正常页面缩放出错。
            if (cw > 0 && ch > 0) naturalRef.current = nat;
          }
          const w = managedZoom ? Math.max(canvasW, nat.w || 0) : nat.w;
          const h = nat.h;
          let scale = 1;
          if (zoomMode === 'fit') {
            const widthScale = w > 0 && panelW > 0 ? panelW / w : 1;
            const heightScale = h > 0 && panelH > 0 ? panelH / h : 1;
            scale = Math.min(widthScale, heightScale);
          } else if (zoomMode === 'custom') {
            scale = clampPreviewScale(customScale);
          } else if (zoomMode === 'fit-width' || zoomMode === 'auto-width') {
            scale = (w > panelW && w > 0) ? panelW / w : 1;
          }
          scale = clampPreviewScale(scale);
          var nextBox = { w, h, scale, panelW, panelH };
          setBox(prev => (
            prev &&
            Math.abs(prev.w - nextBox.w) < 0.5 &&
            Math.abs(prev.h - nextBox.h) < 0.5 &&
            Math.abs(prev.scale - nextBox.scale) < 0.001 &&
            Math.abs(prev.panelW - nextBox.panelW) < 0.5 &&
            Math.abs(prev.panelH - nextBox.panelH) < 0.5
              ? prev
              : nextBox
          ));
          if (onScaleChange) onScaleChange(scale);
        } catch (e) { /* 未就绪/跨域，忽略 */ }
      };
      useEffect(() => { setReady(false); setBox(null); naturalRef.current = null; }, [html]);
      useEffect(() => { measure(); }, [zoomMode, customScale]);
      useEffect(() => {
        if (!wrapRef.current || typeof ResizeObserver === 'undefined') return;
        const ro = new ResizeObserver(() => measure());
        ro.observe(wrapRef.current);
        return () => ro.disconnect();
      }, [zoomMode, customScale]);
      const applyWheelZoom = deltaY => {
        const base = box ? box.scale : customScale;
        const next = clampPreviewScale(base + (deltaY < 0 ? 0.1 : -0.1));
        if (onCustomScaleChange) onCustomScaleChange(next);
      };
      const handleWheel = event => {
        if (!managedZoom || !event.ctrlKey) return;
        event.preventDefault();
        event.stopPropagation();
        applyWheelZoom(event.deltaY);
      };
      useEffect(() => {
        const fr = frameRef.current;
        if (!managedZoom || !fr || !fr.contentWindow) return undefined;
        let doc = null;
        try {
          doc = fr.contentWindow.document;
        } catch (e) {
          return undefined;
        }
        if (!doc) return undefined;
        const handleFrameWheel = event => {
          if (!event.ctrlKey) return;
          event.preventDefault();
          event.stopPropagation();
          applyWheelZoom(event.deltaY);
        };
        doc.addEventListener('wheel', handleFrameWheel, { passive: false, capture: true });
        return () => doc.removeEventListener('wheel', handleFrameWheel, { capture: true });
      }, [managedZoom, ready, box && box.scale, customScale, onCustomScaleChange]);
      useEffect(() => {
        const handlePreviewMessage = event => {
          const frameWindow = frameRef.current && frameRef.current.contentWindow;
          if (!frameWindow || event.source !== frameWindow) return;
          const url = artifactPreviewExternalUrlFromMessage(event.data);
          if (url && onOpenExternal) onOpenExternal(url);
        };
        window.addEventListener('message', handlePreviewMessage);
        return () => window.removeEventListener('message', handlePreviewMessage);
      }, [onOpenExternal]);
      const scaled = box && box.scale !== 1;
      const scaledW = box ? Math.max(1, Math.ceil(box.w * box.scale)) : 0;
      const scaledH = box ? Math.max(1, Math.ceil(box.h * box.scale)) : 0;
      const stageStyle = box
        ? {
          minWidth: Math.max(box.panelW || 0, scaledW || box.w) + 'px',
          minHeight: Math.max(box.panelH || 0, scaledH || box.h) + 'px',
          display: 'flex',
          justifyContent: (zoomMode === 'fit' || zoomMode === 'custom') && scaledW <= (box.panelW || 0) ? 'center' : 'flex-start',
          alignItems: (zoomMode === 'fit' || zoomMode === 'custom') && scaledH <= (box.panelH || 0) ? 'center' : 'flex-start',
        }
        : { minWidth: '100%', minHeight: '100%' };
      const frameStyle = () => {
        if (box && scaled) {
          return { position: 'absolute', left: 0, top: 0, width: box.w + 'px', height: box.h + 'px', transform: 'scale(' + box.scale + ')', transformOrigin: 'top left', colorScheme: 'dark' };
        }
        if (managedZoom && box) return { width: box.w + 'px', height: box.h + 'px', minHeight: '480px', colorScheme: 'dark' };
        return { width: '100%', height: '100%', minHeight: '480px', colorScheme: 'dark' };
      };
      const wrapStyle = managedZoom
        ? { minHeight: 0, height: '100%', overflow: zoomMode === 'fit' ? 'hidden' : 'auto' }
        : (scaled ? { height: scaledH } : { minHeight: 480, height: '100%' });
      return (
        <div ref={wrapRef} data-testid="artifact-html-preview-scroll" onWheel={handleWheel} className="relative w-full bg-[#15171a]" style={wrapStyle}>
          {!ready && <div className="h-[480px] bg-[#15171a]"></div>}
          <div data-testid="artifact-html-preview-stage" style={managedZoom ? stageStyle : (box && scaled ? { width: scaledW + 'px', height: scaledH + 'px', position: 'relative' } : { width: '100%', height: '100%' })}>
            <div style={box && scaled ? { width: scaledW + 'px', height: scaledH + 'px', position: 'relative', flex: '0 0 auto' } : (managedZoom && box ? { width: box.w + 'px', height: box.h + 'px', flex: '0 0 auto' } : { width: '100%', height: '100%' })}>
              <iframe ref={frameRef} sandbox="allow-same-origin allow-scripts" data-testid="artifact-html-preview-frame" onLoad={() => { measure(); if (onFrameLoad) onFrameLoad(frameRef.current); setTimeout(() => setReady(true), 80); }}
                className={`border-0 block bg-[#15171a] transition-opacity duration-300 ${ready ? 'opacity-100' : 'opacity-0 absolute pointer-events-none'}`}
                data-zoom-mode={zoomMode}
                data-zoom-scale={box ? String(box.scale) : ''}
                style={frameStyle()}
                srcDoc={buildArtifactPreviewDocument(html)} />
            </div>
          </div>
        </div>
      );
    };

    // 输入框「技能」入口：⚡ 药丸 + popover。视觉设计=内置自动技能（只读，模型 load_skill 时显"使用中"
    // 并高亮药丸）。activeSkill 由 bridge 检测 load_skill 设，纯只读指示。
    const ComposerModeMenu = ({ t, bs, compact }) => {
      const [open, setOpen] = useState(false);
      const SKILLS = [
        { id: 'visual-design', name: t.uiSettingsView.visualDesignSkillName, desc: t.uiSettingsView.visualDesignSkillDesc, kind: 'auto' },
      ];
      const activeId = bs && bs.activeSkill;
      const cur = SKILLS.find(s => s.id === activeId && s.kind === 'auto');
      return (
        <div className="relative">
          <button onClick={() => setOpen(o => !o)} title={cur ? cur.name : t.composerMode}
            className={`flex items-center shrink-0 font-semibold transition-colors border ${compact ? 'justify-center w-9 h-9 rounded-full' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] whitespace-nowrap'} ${cur
              ? 'bg-[#007AFF]/[0.1] dark:bg-[#0A84FF]/20 text-[#007AFF] dark:text-[#5AC8FA] border-[#007AFF]/20 dark:border-[#0A84FF]/30'
              : 'bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] text-gray-700 dark:text-gray-200 border-black/[0.045] dark:border-white/[0.06]'}`}>
            <Zap size={compact ? 14 : 13} className={cur ? '' : 'opacity-70'} />
            {!compact && (cur ? cur.name : t.composerMode)}
            {!compact && <ChevronDown size={13} className="opacity-50 shrink-0" />}
          </button>
          {open && (
            <>
              <div className="fixed inset-0 z-40" onClick={() => setOpen(false)}></div>
              <div className="absolute bottom-full left-0 mb-2 z-50 w-64 bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                <div className="px-3 py-2 text-[11px] font-bold text-gray-400 dark:text-gray-500 uppercase tracking-wider">{t.composerModeTitle}</div>
                {SKILLS.map(s => {
                  const soon = s.kind === 'soon';
                  const inUse = s.kind === 'auto' && activeId === s.id;
                  return (
                    <div key={s.id} className={`flex items-start justify-between gap-2 px-3 py-2.5 rounded-xl ${soon ? 'opacity-50' : ''}`}>
                      <span className="min-w-0">
                        <span className="block text-[13px] font-medium text-gray-800 dark:text-gray-100 truncate">{s.name}</span>
                        <span className="block text-[11px] text-gray-400 dark:text-gray-500 truncate">{s.desc}</span>
                      </span>
                      {soon
                        ? <span className="shrink-0 text-[10px] font-semibold text-gray-400 dark:text-gray-500 bg-black/[0.04] dark:bg-white/10 px-2 py-0.5 rounded-full leading-none mt-0.5">{t.composerSkillSoon}</span>
                        : inUse
                          ? <span className="shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold text-[#34C759] bg-[#34C759]/10 px-2 py-0.5 rounded-full leading-none mt-0.5"><span className="w-1.5 h-1.5 rounded-full bg-[#34C759]" />{t.composerSkillInUse}</span>
                          : <span className="shrink-0 text-[10px] font-semibold text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15 px-2 py-0.5 rounded-full leading-none mt-0.5">{t.composerSkillAuto}</span>}
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      );
    };
    // 输入框底栏:工具菜单(只展示已装工具 + 跳工具商店;无会话级开关——后端无此概念)。
    // 可选触发器变体：triggerVariant='pill' 时触发器渲染为代码页配置组同款 pill
    //（triggerLabel 为可选 10px 前缀文案；triggerTestId 覆盖默认 testid），
    // 下拉内容不变；不传变体时聊天页外观逐字节不变。
    const ComposerToolMenu = ({ t, onGotoTools, compact, activeSkill, triggerVariant, triggerLabel, triggerTestId, scope, activeSessionId: activeSessionIdProp }) => {
      const [open, setOpen] = useState(false);
      const triggerRef = useRef(null);
      const canMutateToolStore = can('toolStoreMutations');
      // 只增不减：有活动会话时只阻隔「关闭」——已进入上下文的工具撤不回，
      // 关闭只能在会话开始前做；「打开」仍允许（后端热重载保留给「增」即时生效）。
      const sessionsState = useBridgeState(['sessions']);
      // scope='code'（原生代码车道）时由调用方传入该车道的活动会话 id——显式会话态
      // 驱动，绕开 bridge 聊天 active 绑定（二轮评审：code 门控不得读聊天域
      // activeSessionId）；plain 缺省沿用聊天侧。
      const hasActiveSession = scope === 'code'
        ? !!activeSessionIdProp
        : !!(sessionsState && sessionsState.activeSessionId);
      // 无权限才全局禁用；会话中的「关闭」阻隔是逐行判断（只有已开 = enabled 才禁）。
      const toolSwitchDisabled = !canMutateToolStore;
      // scope: 'code' = 原生代码会话(独立开关,默认全关),缺省 = 普通会话(plain)。
      const toolScope = scope === 'code' ? 'code' : 'plain';
      const [marketplaceTools, setMarketplaceTools] = useState([]);
      const [marketplaceSkills, setMarketplaceSkills] = useState([]);
      const [disabled, setDisabled] = useState(() => new Set()); // 被关掉的包 id(开关 off，按 scope 持久)
      const [hidden, setHidden] = useState(() => new Set()); // 被不可见的包 id(可见性预过滤，按 scope 持久)
      const [projectSkillsEnabled, setProjectSkillsEnabled] = useState(false); // 项目级 skills(仅 code scope 生效)
      const [projectSkillsHelp, setProjectSkillsHelp] = useState(false); // 项目技能帮助弹窗(功能说明+扫描目录)
      const [feishuOn, setFeishuOn] = useState(false); // 飞书是否已连接(CLI 路线)
      const [feishuEnabled, setFeishuEnabled] = useState(true); // 飞书技能是否启用(未手动停用)
      const [wecomOn, setWecomOn] = useState(false); // 企微是否已连接(CLI 路线)
      const [wecomEnabled, setWecomEnabled] = useState(true); // 企微技能是否启用(未手动停用)
      const [dingtalkOn, setDingtalkOn] = useState(false); // 钉钉是否已连接(CLI 路线)
      const [dingtalkEnabled, setDingtalkEnabled] = useState(true); // 钉钉技能是否启用(未手动停用)
      const [tmeetOn, setTmeetOn] = useState(false); // 腾讯会议是否已连接(CLI 路线)
      const [tmeetEnabled, setTmeetEnabled] = useState(true); // 腾讯会议技能是否启用(未手动停用)
      // 启动时加载已装工具 + 全局持久的禁用列表(持久语义:新窗口/新对话都继承)
      async function refreshToolsMenu(isAlive) {
        try {
          const list = await invokeTauri('list_marketplace_tools');
          if (isAlive()) setMarketplaceTools(Array.isArray(list) ? list : []);
        } catch (e) { /* ignore */ }
        try {
          const skills = await invokeTauri('list_marketplace_skills');
          if (isAlive()) setMarketplaceSkills(Array.isArray(skills) ? skills : []);
        } catch (e) { /* ignore */ }
        try {
          const dis = await invokeTauri('get_disabled_connectors', { scope: toolScope });
          if (isAlive()) setDisabled(new Set(dis || []));
        } catch (e) { /* ignore */ }
        try {
          const hid = await invokeTauri('get_bundle_visibility', { scope: toolScope });
          if (isAlive()) setHidden(new Set(hid || []));
        } catch (e) { /* ignore */ }
        try {
          const proj = await invokeTauri('get_project_skills_enabled');
          if (isAlive()) setProjectSkillsEnabled(!!proj);
        } catch (e) { /* ignore */ }
        try {
          const fs = await invokeTauri('feishu_skills_state');
          if (isAlive()) { setFeishuOn(!!(fs && fs.connected)); setFeishuEnabled(!fs || fs.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ws = await invokeTauri('wecom_skills_state');
          if (isAlive()) { setWecomOn(!!(ws && ws.connected)); setWecomEnabled(!ws || ws.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ds = await invokeTauri('dingtalk_skills_state');
          if (isAlive()) { setDingtalkOn(!!(ds && ds.connected)); setDingtalkEnabled(!ds || ds.enabled !== false); }
        } catch (e) { /* ignore */ }
        try {
          const ts = await invokeTauri('tmeet_skills_state');
          if (isAlive()) { setTmeetOn(!!(ts && ts.connected)); setTmeetEnabled(!ts || ts.enabled !== false); }
        } catch (e) { /* ignore */ }
      }
      useEffect(() => {
        let alive = true;
        const isAlive = () => alive;
        const onChanged = () => refreshToolsMenu(isAlive);
        refreshToolsMenu(isAlive);
        window.addEventListener('pinvou:tools-changed', onChanged);
        return () => { alive = false; window.removeEventListener('pinvou:tools-changed', onChanged); };
      }, []);
      // 项目技能帮助弹窗 Esc 关闭（与项目其他 modal 惯例一致，仅弹窗打开时挂监听）
      useEffect(() => {
        if (!projectSkillsHelp) return undefined;
        const onKey = event => { if (event.key === 'Escape') setProjectSkillsHelp(false); };
        window.addEventListener('keydown', onKey);
        return () => window.removeEventListener('keydown', onKey);
      }, [projectSkillsHelp]);
      function toggleTool(id, enabled) {
        // 只增不减：会话中允许「打开」（enabled=false），只阻隔「关闭」（enabled=true）。
        if (toolSwitchDisabled || (hasActiveSession && enabled)) return;
        // scope 收敛后：工具/技能/CLI 开关统一为包 id 单一禁用集（后端
        // disabled_bundles.json），技能行 id 即包 id，不再带 `skill:` 前缀。
        const next = new Set(disabled);
        next.has(id) ? next.delete(id) : next.add(id);
        setDisabled(next);
        // 按 scope 持久:落盘 + 广播给所有在跑引擎,关一次该 scope 所有新对话/新窗口都继承。
        if (bridge.available) {
          invokeTauri('set_disabled_connectors',
            { connectorIds: Array.from(next), scope: toolScope }).catch(() => {});
        }
      }
      function toggleProjectSkills() {
        if (toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)) return;
        const next = !projectSkillsEnabled;
        setProjectSkillsEnabled(next);
        if (bridge.available) {
          invokeTauri('set_project_skills_enabled', { enabled: next }).catch(() => {});
        }
      }
      const menuState = buildComposerToolMenuState({
        marketplaceTools,
        marketplaceSkills,
        disabledIds: Array.from(disabled),
        hiddenIds: Array.from(hidden),
        activeSkill,
        scope: toolScope,
        serviceStates: [
          { id: 'feishu', title: t.uiSettingsView.serviceFeishu, connected: feishuOn, enabled: feishuEnabled },
          { id: 'wecom', title: t.uiSettingsView.serviceWecom, connected: wecomOn, enabled: wecomEnabled },
          { id: 'dingtalk', title: t.uiSettingsView.serviceDingtalk, connected: dingtalkOn, enabled: dingtalkEnabled },
          { id: 'tmeet', title: t.uiSettingsView.serviceTmeet, connected: tmeetOn, enabled: tmeetEnabled },
        ],
      });
      const { connectedServices, toolRows, skillRows, enabledCount, allSkillsDisabled } = menuState;
      // 内置技能名称/描述由 composer-tool-menu-logic.js 数据提供，在 UI 边界按当前语言覆盖
      const localizedSkillRows = skillRows.map(row => (row.kind === 'builtin-skill' && row.skillId === 'visual-design')
        ? { ...row, title: t.uiSettingsView.visualDesignSkillName, description: t.uiSettingsView.visualDesignSkillDesc }
        : row);
      const statusBadge = (label, tone = 'green') => {
        const cls = tone === 'blue'
          ? 'text-[#007AFF] dark:text-[#5AC8FA] bg-[#007AFF]/10 dark:bg-[#0A84FF]/15'
          : 'text-[#34C759] bg-[#34C759]/10';
        return <span className={`shrink-0 inline-flex items-center gap-1 text-[10px] font-semibold ${cls} px-2 py-0.5 rounded-full leading-none`}><span className={`w-1.5 h-1.5 rounded-full ${tone === 'blue' ? 'bg-[#007AFF] dark:bg-[#5AC8FA]' : 'bg-[#34C759]'}`} />{label}</span>;
      };
      const switchRow = (row) => {
        const rowDisabled = toolSwitchDisabled || (hasActiveSession && row.enabled);
        return (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0 flex items-center gap-1.5">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
            {row.kind === 'service' && statusBadge(t.composerConnected, 'green')}
          </span>
          <button onClick={() => toggleTool(row.id, row.enabled)} aria-label={row.id} disabled={rowDisabled}
            className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors disabled:cursor-default ${rowDisabled ? 'opacity-70' : ''} ${row.enabled ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'}`}>
            <span className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform ${row.enabled ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
          </button>
        </div>
        );
      };
      const readonlyRow = (row, label, tone = 'green') => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
          </span>
          {statusBadge(label, tone)}
        </div>
      );
      // 权限只读开关：显示开关状态（受静态表控制），但不可手动切换；保留「内置」标识。
      const readonlySwitchRow = (row) => (
        <div key={row.id} className="flex items-center justify-between gap-2 px-3 py-2.5 rounded-xl font-medium">
          <span className="min-w-0 flex items-center gap-1.5">
            <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">{row.title}</span>
            {statusBadge(row.active ? t.composerSkillInUse : t.composerBuiltinAuto, row.active ? 'green' : 'blue')}
          </span>
          <button disabled aria-label={row.id} title={t.composerReadonlySwitch}
            className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors cursor-not-allowed bg-[#34C759]/60`}>
            <span className="inline-block h-4 w-4 rounded-full bg-white shadow translate-x-[16px]" />
          </button>
        </div>
      );
      return (
        <div className="relative shrink-0">
          {triggerVariant === 'pill' ? (
            <button
              ref={triggerRef}
              type="button"
              data-testid={triggerTestId || 'composer-tool-menu-trigger'}
              onClick={() => setOpen(o => !o)}
              title={t.composerTools}
              aria-expanded={open}
              className="inline-flex h-8 min-w-0 max-w-[220px] items-center gap-1.5 overflow-hidden rounded-xl border px-2.5 transition-all cursor-pointer hover:-translate-y-px hover:shadow-sm focus-within:border-[#007AFF]/45 focus-within:ring-2 focus-within:ring-[#007AFF]/10 border-black/[0.07] bg-black/[0.025] text-[#1F1F1F] dark:border-white/[0.09] dark:bg-white/[0.055] dark:text-[#E8EAED]"
            >
              {triggerLabel && (
                <span className="pointer-events-none shrink-0 text-[10px] font-medium text-gray-400 dark:text-gray-500">
                  {triggerLabel}
                </span>
              )}
              <span className="pointer-events-none min-w-0 truncate text-[11px] font-semibold">
                {t.composerTools}
              </span>
              {enabledCount > 0 && (
                <span className="min-w-4 h-4 rounded-full bg-[#007AFF] px-1 text-center text-[10px] font-bold leading-4 text-white shrink-0">{enabledCount}</span>
              )}
              <ChevronDown
                size={12}
                aria-hidden="true"
                className={`pointer-events-none ml-auto shrink-0 text-gray-400 transition-transform ${open ? 'rotate-180' : ''}`}
              />
            </button>
          ) : (
          <button ref={triggerRef} data-testid={triggerTestId || 'composer-tool-menu-trigger'} onClick={() => setOpen(o => !o)} title={t.composerTools}
            className={`relative shrink-0 flex items-center justify-center text-gray-700 dark:text-gray-200 transition-colors border ${compact ? 'w-9 h-9 rounded-full bg-transparent hover:bg-black/5 dark:hover:bg-white/10 border-transparent' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] font-semibold whitespace-nowrap bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] border-black/[0.045] dark:border-white/[0.06]'}`}>
            <Wrench size={compact ? 18 : 13} className="opacity-80" />
            {!compact && t.composerTools}
            {enabledCount > 0 && (compact
              ? <span className="absolute -top-1 -right-1 min-w-[16px] h-4 px-1 text-[10px] leading-4 text-center font-bold bg-[#007AFF] text-white rounded-full">{enabledCount}</span>
              : <span className="min-w-4 h-4 rounded-full bg-[#007AFF] px-1 text-center text-[10px] font-bold leading-4 text-white shrink-0">{enabledCount}</span>)}
            {!compact && <ChevronDown size={13} className="opacity-50 shrink-0" />}
          </button>
          )}
          <ComposerPopover open={open} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
            menuProps={{ 'data-testid': 'composer-tool-menu' }}
            desktopClassName="absolute bottom-full left-0 mb-2 w-72 max-h-[420px] z-50 overflow-y-auto custom-scrollbar bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
                {connectedServices.map(switchRow)}
                {toolRows.map(switchRow)}
                {localizedSkillRows.length === 0 ? (
                  (connectedServices.length === 0 && toolRows.length === 0) ? (
                    <div className="px-3 py-2 text-[13px] text-gray-400 dark:text-gray-500">{t.composerModeNone}</div>
                  ) : null
                ) : (
                  <>
                    {localizedSkillRows.map(row => row.switchable
                      ? switchRow(row)
                      : row.readonly
                        ? readonlySwitchRow(row)
                        : readonlyRow(row, row.active ? t.composerSkillInUse : t.composerBuiltinAuto, row.active ? 'green' : 'blue'))}
                    {/* 该 scope 全部技能被关：空态提示（组合目录为空 → 模型看不到任何技能） */}
                    {allSkillsDisabled && (
                      <div className="px-3 pt-1 pb-1 text-[11px] text-gray-400 dark:text-gray-500">{t.composerSkillAllDisabled}</div>
                    )}
                  </>
                )}
                {toolScope === 'code' && (
                  <>
                    <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                    <div className="px-3 py-2">
                      <div className="flex items-center justify-between gap-2">
                        <span className="min-w-0">
                          <span className="block text-[13px] text-gray-700 dark:text-gray-200 truncate">
                            {t.composerProjectSkills}
                            <button onClick={() => setProjectSkillsHelp(true)} aria-label={t.composerProjectSkillsHelpTitle}
                              className="inline-flex items-center justify-center w-[15px] h-[15px] ml-1 rounded-full text-[10px] font-semibold leading-none text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 hover:bg-black/5 dark:hover:bg-white/10 align-middle">?</button>
                          </span>
                          <span className="block text-[10px] text-gray-400 dark:text-gray-500">{t.composerProjectSkillsDesc}</span>
                        </span>
                        <button onClick={toggleProjectSkills} aria-label="project-skills" disabled={toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)}
                          className={`relative inline-flex h-5 w-[34px] shrink-0 items-center rounded-full transition-colors disabled:cursor-default ${(toolSwitchDisabled || (hasActiveSession && projectSkillsEnabled)) ? 'opacity-70' : ''} ${projectSkillsEnabled ? 'bg-[#34C759]' : 'bg-[#E5E5EA] dark:bg-[#39393D]'}`}>
                          <span className={`inline-block h-4 w-4 rounded-full bg-white shadow transition-transform ${projectSkillsEnabled ? 'translate-x-[16px]' : 'translate-x-[2px]'}`} />
                        </button>
                      </div>
                      {projectSkillsEnabled && (
                        <div className="mt-1.5 text-[11px] leading-snug text-amber-600 dark:text-amber-400">{t.composerProjectSkillsWarning}</div>
                      )}
                    </div>
                  </>
                )}
                <div className="h-px bg-black/5 dark:bg-white/10 my-1.5 mx-2" />
                <button onClick={() => { setOpen(false); if (onGotoTools) onGotoTools(); }}
                  className="w-full flex items-center gap-2.5 px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group">
                  <Store size={15} className="text-gray-400 group-hover:text-white/90" />
                  {t.composerManageTools}
                </button>
          </ComposerPopover>
          {projectSkillsHelp && createPortal(
            <div className="fixed inset-0 z-[90] flex items-center justify-center p-4 bg-black/45" onClick={() => setProjectSkillsHelp(false)}>
              <div onClick={e => e.stopPropagation()} className="relative w-full max-w-[380px] rounded-[22px] shadow-2xl p-5 bg-white text-[#1F1F1F] dark:bg-[#1E1F20] dark:text-[#E3E3E3]">
                <div className="flex items-start justify-between gap-3 mb-4">
                  <div className="text-[16px] font-semibold">{t.composerProjectSkillsHelpTitle}</div>
                  <button onClick={() => setProjectSkillsHelp(false)} className="w-8 h-8 rounded-full flex items-center justify-center hover:bg-black/5 dark:hover:bg-white/10"><X size={17} /></button>
                </div>
                <div className="text-[12px] leading-relaxed text-[#5F6368] dark:text-[#AEB4BC]">{t.composerProjectSkillsHelpBody}</div>
                <div className="mt-3 text-[12px] font-medium">{t.composerProjectSkillsHelpDirsLabel}</div>
                <div className="mt-1.5 rounded-[14px] border p-3 border-black/10 bg-[#F8F9FA] dark:border-white/10 dark:bg-white/[0.035]">
                  {String(t.composerProjectSkillsHelpDirs).split('\n').map((dir, i) => (
                    <div key={dir} className="flex items-center gap-2 text-[11px] font-mono text-gray-600 dark:text-gray-300 py-0.5">
                      <span className="text-[10px] text-gray-400 dark:text-gray-500">{i + 1}</span>{dir}
                    </div>
                  ))}
                </div>
                <div className="mt-3 text-[11px] leading-snug text-amber-600 dark:text-amber-400">{t.composerProjectSkillsWarning}</div>
              </div>
            </div>,
            document.body
          )}
        </div>
      );
    };

export { ComposerModelSelector, ScaledHtmlPreview, ComposerModeMenu, ComposerToolMenu };
