// 聊天/代码页共用的输入框底栏控件。
//
// 从 ChatView 提取（2026-08）：原定义在 ChatView 组件体内。为支持代码模块原生
// （品悟）车道复用，三个控件都接受可选的“显式会话态驱动”props：传入时绕开
// bridge 聊天 active 绑定（bridge 的 models/knowledge/interaction 方法都绑聊天
// activeSession 且 ensureSession 会物化聊天会话，代码车道必须直调 invoke 显式
// 传 sessionId）；不传时走原 bs/bridge 路径，聊天页行为不变。

import React, { useEffect, useRef, useState } from 'react';
import { BookOpen, Check, ChevronDown, ClipboardList, X, Zap } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ComposerPopover } from '../../components/ComposerPopover.jsx';
import { invokeTauri, isTauriAvailable } from '../../platform/tauri/client.js';

const COMPOSER_ICON_BUTTON_CLASS = 'w-9 h-9 shrink-0 rounded-full flex items-center justify-center bg-transparent text-gray-700 hover:text-gray-900 dark:text-gray-200 dark:hover:text-white hover:bg-black/5 dark:hover:bg-white/10 transition-colors border border-transparent';

const ComposerKbSelector = ({
  t,
  bs,
  compact,
  mountedId: mountedIdProp,
  mountedCollections: mountedCollectionsProp,
  onMount,
  onUnmount,
  onSetCollectionEnabled,
  onRemoveCollection,
}) => {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef(null);
  const [collections, setCollections] = useState(null); // null=未加载
  const [modelStatus, setModelStatus] = useState(null); // null=未知；新后端同时返回 installed/ready/loading
  // 显式会话态驱动（代码车道）优先；否则读 bridge 聊天 active 的挂载态。
  // 代码车道当前仍可只传 mountedId/onMount/onUnmount，保持原单库契约兼容。
  const explicitMountState = mountedIdProp !== undefined || mountedCollectionsProp !== undefined;
  const localMountedSource = mountedCollectionsProp !== undefined
    ? mountedCollectionsProp
    : (mountedIdProp !== undefined
      ? (mountedIdProp == null ? [] : [{ collectionId: mountedIdProp, enabled: true }])
      : ((bs && Array.isArray(bs.mountedCollections))
        ? bs.mountedCollections
        : ((bs && bs.mountedCollection != null) ? [{ collectionId: bs.mountedCollection, enabled: true }] : [])));
  const remoteMountedSource = explicitMountState ? [] : ((bs && Array.isArray(bs.mountedRemoteCollections)) ? bs.mountedRemoteCollections : []);
  const mountedSource = localMountedSource.map(entry => (typeof entry === 'object' ? { ...entry, source: 'local' } : { collectionId: entry, enabled: true, source: 'local' }))
    .concat(remoteMountedSource.map(entry => ({ ...entry, source: 'remote' })));
  const seenMounted = new Set();
  const mountedEntries = mountedSource.map(entry => {
    const collectionId = typeof entry === 'object'
      ? (entry.collectionId != null ? entry.collectionId : entry.collection_id)
      : entry;
    const source = entry && entry.source === 'remote' ? 'remote' : 'local';
    const serverId = source === 'remote' ? (entry.serverId || entry.server_id) : null;
    const key = source === 'remote' ? `remote:${serverId}:${collectionId}` : `local:${collectionId}`;
    if (collectionId == null || (source === 'remote' && !serverId) || seenMounted.has(key)) return null;
    seenMounted.add(key);
    return { key, source, serverId, collectionId, enabled: typeof entry === 'object' ? entry.enabled !== false : true };
  }).filter(Boolean);
  const mountedEntry = collection => {
    const key = typeof collection === 'string' ? collection : (collection.key || `local:${collection.id}`);
    return mountedEntries.find(entry => entry.key === key) || null;
  };

  const loadList = async () => {
    if (!bridge.available || !bridge.knowledge.listCollections) { setCollections([]); return; }
    let local;
    try {
      local = ((await bridge.knowledge.listCollections()) || []).map(c => ({ ...c, key: `local:${c.id}`, source: 'local', serverId: null, serverName: null, ready: true }));
    }
    catch (e) { setCollections([]); return; }
    let remoteCollections = [];
    // 显式会话态（原生 Code 车道）当前只实现了本地知识集命令；不要把远程条目
    // 交给它的 onMount/onRemove 回调，否则相同数字 id 可能被当成本地知识集挂错。
    if (isTauriAvailable() && !explicitMountState) {
      // 远程加载单独兜底：remote_kb_connections 或单个 server 失败只跳过远程
      // 集合，保留已加载的本地列表。
      try {
        const servers = (await invokeTauri('remote_kb_connections')) || [];
        const pages = await Promise.all(servers.map(async server => {
          try {
            const list = await invokeTauri('remote_kb_collections', { serverId: server.serverId, includeDeleted: false });
            return (list || []).map(c => ({ ...c, key: `remote:${server.serverId}:${c.id}`, source: 'remote', serverId: server.serverId, serverName: server.name, ready: server.ready && server.online }));
          } catch (_) { return []; }
        }));
        remoteCollections = pages.flat();
      } catch (_) { /* 远程不可用时不影响本地列表 */ }
    }
    setCollections(local.concat(remoteCollections));
  };
  const refreshModelStatus = async () => {
    if (!bridge.available || !bridge.knowledge.kbModelStatus) { setModelStatus({ installed: true }); return; } // mock/旧后端不 gate
    try { const m = await bridge.knowledge.kbModelStatus(); setModelStatus(m || { installed: true }); }
    catch (e) { setModelStatus({ installed: true }); }
  };
  useEffect(() => { refreshModelStatus(); }, []);
  // 首帧后台加载/下载完成后由 bridge 推送真实进程态，免重开菜单。
  const modelSetup = (bs && bs.kbModelSetup) || {};
  const setupStatus = modelSetup.status || null;
  useEffect(() => {
    if (setupStatus) setModelStatus(setupStatus);
    else if (modelSetup.startupReady === true) {
      setModelStatus(status => Object.assign({}, status || { installed: true }, { ready: true, loading: false }));
    }
  }, [setupStatus, modelSetup.startupReady]);
  // 已挂载但还没列表 → 拉一次用于显示名字。
  const mountedKey = mountedEntries.map(entry => `${entry.key}:${entry.enabled}`).join('|');
  useEffect(() => {
    if (mountedEntries.length > 0 && collections === null) loadList();
  }, [mountedKey]);

  const mountedNames = mountedEntries.map(entry => {
    const collection = (collections || []).find(c => c.key === entry.key);
    return collection ? (collection.source === 'remote' ? `${collection.serverName} / ${collection.name}` : collection.name) : ((entry.source === 'remote' ? `${t.remoteKnowledge} / ` : '') + '#' + entry.collectionId);
  });
  const displayedCollections = (collections || []).concat(
    mountedEntries
      .filter(entry => !(collections || []).some(c => c.key === entry.key))
      .map(entry => ({ id: entry.collectionId, key: entry.key, source: entry.source, serverId: entry.serverId, serverName: entry.source === 'remote' ? t.remoteKnowledge : null, name: '#' + entry.collectionId, docCount: 0, ready: true })),
  );
  const mountedName = mountedEntries.length === 1
    ? mountedNames[0]
    : (mountedEntries.length > 1 ? t.kbMountCount(mountedEntries.length) : null);
  const mountedTitle = mountedEntries.map((entry, index) => (
    `${mountedNames[index]} (${entry.enabled ? t.kbMountEnabled : t.kbMountDisabled})`
  )).join(' · ');
  const active = mountedEntries.length > 0;
  const modelMissing = modelStatus && modelStatus.installed === false;
  const runtimeReadyKnown = modelStatus && typeof modelStatus.ready === 'boolean';
  const modelNotReady = !modelMissing && (
    modelSetup.startupLoading === true
    || (runtimeReadyKnown && modelStatus.ready === false && modelSetup.startupReady !== true)
  );
  const modelBlocked = modelMissing || modelNotReady;
  const triggerBlocked = modelBlocked
    && !mountedEntries.some(entry => entry.source === 'remote')
    && !(collections || []).some(collection => collection.source === 'remote' && collection.ready !== false);
  const modelBlockedCopy = modelMissing ? t.kbMountNoModel : t.kbMountNotReady;

  function toggle() { const next = !open; setOpen(next); if (next) { refreshModelStatus(); if (collections === null) loadList(); } }
  function pick(collection) {
    if (collection.source === 'local' && modelBlocked) return;
    if (collection.source === 'remote' && collection.ready === false) return;
    if (mountedEntry(collection)) return;
    if (collection.source === 'remote') {
      if (explicitMountState) return;
      if (bridge.available && bridge.knowledge.mountRemoteCollection) {
        bridge.knowledge.mountRemoteCollection(collection.serverId, collection.id);
      }
      return;
    }
    if (explicitMountState) setOpen(false);
    if (onMount) { onMount(collection.id); return; }
    if (!bridge.available) return;
    bridge.knowledge.mountCollection(collection.id);
  }
  function toggleEnabled(collection) {
    const entry = mountedEntry(collection);
    if (!entry || (entry.source === 'local' && modelBlocked && !entry.enabled) || (entry.source === 'remote' && collection.ready === false && !entry.enabled)) return;
    if (entry.source === 'remote') {
      if (explicitMountState) return;
      if (bridge.available && bridge.knowledge.setRemoteCollectionEnabled) {
        bridge.knowledge.setRemoteCollectionEnabled(entry.serverId, entry.collectionId, !entry.enabled);
      }
      return;
    }
    if (onSetCollectionEnabled) { onSetCollectionEnabled(collection.id, !entry.enabled); return; }
    if (!explicitMountState && bridge.available) {
      bridge.knowledge.setCollectionEnabled(entry.collectionId, !entry.enabled);
    }
  }
  function remove(collection) {
    const entry = mountedEntry(collection);
    if (!entry) return;
    if (entry.source === 'remote') {
      if (explicitMountState) return;
      if (bridge.available && bridge.knowledge.removeRemoteCollection) {
        bridge.knowledge.removeRemoteCollection(entry.serverId, entry.collectionId);
      }
      return;
    }
    if (onRemoveCollection) { onRemoveCollection(collection.id); return; }
    if (explicitMountState) { if (onUnmount) onUnmount(); return; }
    if (bridge.available) {
      bridge.knowledge.removeCollection(entry.collectionId);
    }
  }
  function unmount() {
    setOpen(false);
    if (onUnmount) { onUnmount(); return; }
    if (bridge.available) bridge.knowledge.unmountCollection();
  }

  return (
    <div className="relative">
      <button ref={triggerRef} onClick={toggle} data-testid="kb-mount-trigger" title={active ? mountedTitle : (triggerBlocked ? modelBlockedCopy : t.kbMountTitle)}
        className={`relative shrink-0 flex items-center justify-center transition-colors border ${compact ? 'w-9 h-9 rounded-full' : 'h-8 gap-1.5 rounded-[12px] px-2.5 text-[12px] font-semibold'} ${active
          ? (compact ? 'bg-transparent text-[#1A73E8] dark:text-[#A8C7FA] border-transparent' : 'bg-[#007AFF]/10 dark:bg-[#0A84FF]/18 text-[#007AFF] dark:text-[#5AC8FA] border-[#007AFF]/20 dark:border-[#0A84FF]/25')
          : triggerBlocked
            ? 'bg-transparent text-gray-400 dark:text-gray-600 border-transparent opacity-70'
            : (compact ? 'bg-transparent hover:bg-black/5 dark:hover:bg-white/10 text-gray-700 dark:text-gray-200 border-transparent' : 'bg-black/[0.045] dark:bg-white/[0.055] hover:bg-black/[0.07] dark:hover:bg-white/[0.09] text-gray-700 dark:text-gray-200 border-black/[0.045] dark:border-white/[0.06]')}`}>
        <BookOpen size={compact ? 18 : 13} className="opacity-70 shrink-0" />
        {!compact && <span className="max-w-[116px] truncate">{active ? mountedName : t.kbMount}</span>}
        {!compact && <ChevronDown size={13} className="opacity-50 shrink-0" />}
        {compact && active && <span className="absolute top-1 right-1 w-1.5 h-1.5 rounded-full bg-[#1A73E8] dark:bg-[#A8C7FA] ring-2 ring-white dark:ring-[#161618]"></span>}
      </button>
      <ComposerPopover open={open} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
        desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-64 max-h-[340px] overflow-y-auto bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
            {modelBlocked && displayedCollections.some(c => c.source === 'local') && (
              <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">{modelBlockedCopy}</div>
            )}
            {collections === null ? (
              <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">…</div>
            ) : displayedCollections.length === 0 ? (
              <div className="px-3 py-2.5 text-[13px] text-gray-400 dark:text-gray-500">{t.kbMountNone}</div>
            ) : displayedCollections.map(c => {
              const entry = mountedEntry(c);
              const disabled = (c.source === 'local' && modelBlocked && (!entry || !entry.enabled))
                || (c.source === 'remote' && c.ready === false && (!entry || !entry.enabled));
              return (
                <div key={c.key} data-testid="kb-mount-row"
                  className="flex items-center rounded-xl text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white transition-colors group">
                  <button onClick={() => entry ? toggleEnabled(c) : pick(c)} disabled={disabled} data-testid="kb-mount-toggle"
                    title={entry ? (entry.enabled ? t.kbMountDisable : t.kbMountEnable) : t.kbMountPick}
                    className="min-w-0 flex-1 flex items-center justify-between gap-2.5 px-3 py-2.5 text-[13px] text-left disabled:cursor-not-allowed disabled:opacity-50">
                    <span className="flex items-center gap-2.5 min-w-0">
                      <BookOpen size={15} className="shrink-0 text-gray-400 group-hover:text-white/90" />
                      <span className="min-w-0 flex flex-col">
                        <span className="truncate">{c.source === 'remote' ? `${c.serverName} / ${c.name}` : c.name}</span>
                        {entry && <span className="text-[10px] text-gray-400 group-hover:text-white/80">{entry.enabled ? t.kbMountEnabled : t.kbMountDisabled}</span>}
                      </span>
                    </span>
                    {entry
                      ? (entry.enabled
                        ? <Check size={15} className="shrink-0 text-[#007AFF] group-hover:text-white" />
                        : <span className="shrink-0 w-3.5 h-3.5 rounded border border-gray-300 dark:border-gray-600 group-hover:border-white/80" />)
                      : <span className="text-[11px] text-gray-400 group-hover:text-white/80 shrink-0">{c.docCount}</span>}
                  </button>
                  {entry && (
                    <button onClick={() => remove(c)} title={t.kbMountRemoveOne} aria-label={`${t.kbMountRemoveOne}: ${c.name}`}
                      data-testid="kb-mount-remove" className="shrink-0 p-2.5 mr-0.5 rounded-lg text-gray-400 hover:text-white hover:bg-white/15">
                      <X size={14} />
                    </button>
                  )}
                </div>
              );
            })}
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
      </ComposerPopover>
    </div>
  );
};

// [plan/yolo] composer 模式 chip:默认 Yolo,下拉手切 Plan。进 Plan=只读调研
// (底座 ReadOnly+只读工具集),调 update_plan 出方案卡决策。切换逻辑搬自旧 ModeHeader。
const ComposerModeChip = ({ t, bs, compact, mode: modeProp, busy: busyProp, onSwitch }) => {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef(null);
  // 显式会话态驱动（代码车道）优先；否则读 bridge 聊天 active 的 mode/busy。
  const ms = modeProp != null ? { mode: modeProp } : ((bs && bs.modeState) || { mode: 'yolo' });
  const isPlan = ms.mode === 'plan';
  const busy = busyProp !== undefined ? busyProp : (bs && bs.busy);
  async function switchTo(target) {
    setOpen(false);
    // 点击已激活模式：无状态变更，早退避免冗余刷新（代码车道 onSwitch 路径
    // 会触发一次 refreshNativeControls 3 次 invoke；ChatView bridge 路径同样受益）。
    if ((target === 'plan' && isPlan) || (target === 'yolo' && !isPlan)) return;
    if (onSwitch) { onSwitch(target, { isPlan, busy }); return; }
    if (!bridge.available) return;
    if (target === 'plan' && !isPlan) {
      await bridge.interaction.setPlanModeNext();
    } else if (target === 'yolo' && isPlan) {
      if (busy) await bridge.chat.cancelGeneration();
      await bridge.interaction.exitPlanToYolo();
    }
  }
  const optCls = "w-full flex items-center justify-between px-3 py-2.5 text-[13px] text-gray-700 dark:text-gray-200 hover:bg-[#007AFF] hover:text-white rounded-xl transition-colors group";
  return (
    <div className="relative">
      <button ref={triggerRef} onClick={() => setOpen(!open)} title={t.modeSwitchTitle + ' · ' + (isPlan ? t.modePlan : t.modeYolo)}
        className={`${COMPOSER_ICON_BUTTON_CLASS} font-semibold ${isPlan ? 'text-[#1A73E8] dark:text-[#A8C7FA]' : ''}`}>
        {isPlan
          ? <ClipboardList size={18} className="shrink-0" />
          : <Zap size={18} className="shrink-0" />}
      </button>
      <ComposerPopover open={open} onClose={() => setOpen(false)} triggerRef={triggerRef} compact={compact}
        desktopClassName="absolute bottom-full left-0 mb-2 z-50 w-60 bg-white dark:bg-[#1E1E20] border border-black/5 dark:border-white/10 rounded-2xl shadow-xl p-1.5">
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
      </ComposerPopover>
    </div>
  );
};

export { COMPOSER_ICON_BUTTON_CLASS, ComposerKbSelector, ComposerModeChip };
