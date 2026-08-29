import React, { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { CodexAcpView as LazyCodexAcpView } from '../features/codex/LazyCodexAcpView.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { VIEW_LOADERS } from './view-loaders.js';
// 撕离窗与主窗口共用同一批懒加载视图 chunk(rolldown 自动共享),工厂统一走
// view-loaders.js 的 VIEW_LOADERS。静态 import 会让对应视图被钉回主 chunk,
// 这里全部走 lazy;每个窗只实际加载自己 kind 的 chunk。两个例外:
// - codex:复用 #159 的 LazyCodexAcpView 包装(web 能力门控 + checking 兜底,
//   内部同样 lazy import CodexAcpView,共享同一 chunk);
// - chat:主窗口在 main.jsx 静态 import ChatView(启动即渲染),撕离窗加载同一
//   index.html,主 chunk 必然已就绪 —— 动态 import 不会产生独立 chunk,直接用
//   静态 import 复用主 chunk 里的同一模块。
const LazyKnowledgeView = lazy(() => VIEW_LOADERS.knowledge().then(m => ({ default: m.KnowledgeView })));
const LazyMonitorView = lazy(() => VIEW_LOADERS.monitor().then(m => ({ default: m.MonitorView })));
const LazyToolStoreView = lazy(() => VIEW_LOADERS.toolStore().then(m => ({ default: m.ToolStoreView })));
const LazyCardPoolView = lazy(() => VIEW_LOADERS.cardpool().then(m => ({ default: m.CardPoolView })));
const DetachedViewFallback = () => <div className="p-6 text-sm opacity-60">…</div>;
import { useBridgeState } from '../hooks/useBridge.js';
import { emitTauri, isTauriAvailable, listenTauri } from '../platform/tauri/client.js';
import { listAcpSessions } from '../features/codex/acpClient.js';
import { dict, ensureLanguage, initialSystemLanguage, TAG_TO_LANG } from '../shared/i18n.js';
import { ensurePersonaI18nOverlay } from './personas-overlay.js';

function useDetachedBase() {
  const bs = useBridgeState([
    'platform', 'sessions', 'chat', 'voice', 'knowledge', 'scheduled', 'monitor',
    'settings', 'personas',
  ]);
  const [language, setLanguage] = useState(initialSystemLanguage);
  const [activeTheme, setActiveTheme] = useState('dark');
  const [, setPersonaI18nTick] = useState(0);
  const initRef = useRef(false);

  useEffect(() => {
    if (initRef.current || !bs || !bs.settings) return;
    const lang = TAG_TO_LANG[bs.settings.language];
    // One-shot bootstrap once bridge state is ready (language/theme only take
    // initial values at detached-window startup, then this window manages them);
    // initRef blocks repeat writes from later settings changes, keeping the
    // original one-shot semantics.
    // en/ja lazy dictionaries: the entry only bootstraps the system language;
    // the persisted language may not be loaded yet, so ensure it before switching.
    if (lang) ensureLanguage(lang).then((ok) => { if (ok) setLanguage(lang); }).catch(() => {});
    setActiveTheme(bs.settings.theme === 'liquid-light' ? 'light' : 'dark');
    initRef.current = true;
  }, [bs]);
  useEffect(() => {
    document.documentElement.classList.toggle('dark', activeTheme === 'dark');
  }, [activeTheme]);
  // 与主窗 App 同一兜底:撕离窗加载同一 index.html,「系统中文 + 英/日 UI」时
  // index.html 快速路径跳过注入,卡池撕离窗卡名会停在中文。UI 语言为 en/ja 时
  // 兜底注入 overlay,加载完成 bump 一次让卡名重渲染。
  useEffect(() => {
    if (language === 'en' || language === 'ja') {
      ensurePersonaI18nOverlay(() => setPersonaI18nTick(v => v + 1));
    }
  }, [language]);

  // 兜底 zh:词典 chunk 装载失败时按 zh 渲染而非白屏(与 PetWindow/ReaderApp 同口径)。
  return { bs, activeTheme, t: dict[language] || dict.zh };
}

class DetachedErrorBoundary extends React.Component {
  constructor(props) {
    super(props);
    this.state = { err: null };
  }

  static getDerivedStateFromError(err) {
    return { err };
  }

  componentDidCatch(err, info) {
    console.error('[detached] panel render failed', err && err.stack ? err.stack : err,
      info && info.componentStack ? info.componentStack : info);
  }

  render() {
    if (this.state.err) {
      const message = String((this.state.err && this.state.err.message) || this.state.err);
      return <div className="p-6 text-sm opacity-70">{this.props.t.uiMainApp.panelLoadFailed(message)}</div>;
    }
    return this.props.children;
  }
}

function DetachedCodexSessionView({ id, theme, t, bs }) {
  const [sessions, setSessions] = useState(null);
  const [loadFailed, setLoadFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    const refresh = async () => {
      try {
        const next = await listAcpSessions();
        if (!disposed) {
          setSessions(Array.isArray(next) ? next : []);
          setLoadFailed(false);
        }
      } catch (error) {
        console.warn('[detached-codex] list sessions failed', error);
        if (!disposed) setLoadFailed(true);
      }
    };
    refresh();
    listenTauri('session:deleted', refresh).then(fn => {
      if (disposed) fn();
      else unlisten = fn;
    }).catch(() => {});
    return () => {
      disposed = true;
      if (unlisten) unlisten();
    };
  }, [id]);

  if (!id || loadFailed) {
    return <div className="p-6 text-sm opacity-70">{t.uiMainApp.detachedSessionLoadFailed}</div>;
  }
  if (sessions === null) {
    return <div className="p-6 text-sm opacity-60">…</div>;
  }
  if (sessions.every(session => !(session && session.id === id))) {
    return <div className="p-6 text-sm opacity-70">{t.uiMainApp.detachedSessionMissing}</div>;
  }
  return (
    <Suspense fallback={<DetachedViewFallback />}>
    <LazyCodexAcpView
      theme={theme}
      t={t}
      sessions={sessions}
      activeId={id}
      onActiveSessionChange={() => {}}
      onSessionsChange={next => setSessions(Array.isArray(next) ? next : [])}
      onSwitchHomeMode={() => {}}
      bs={bs}
      onGotoTools={() => {}}
      fixedSession
    />
    </Suspense>
  );
}

// Reuse the same feature views as the main window. Cross-view navigation is a
// no-op because a detached window intentionally owns one view only.
const DETACHED_VIEWS = {
  session: ({ theme, t, bs }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={() => {}} justInstalledTool={null} setJustInstalledTool={() => {}} onGotoSettings={() => {}} onGotoTools={() => {}} />,
  'codex-session': ({ id, theme, t, bs }) => <DetachedCodexSessionView id={id} theme={theme} t={t} bs={bs} />,
  monitor: ({ theme, t, bs }) => <LazyMonitorView theme={theme} t={t} bs={bs} />,
  cardpool: ({ theme, t, bs }) => <LazyCardPoolView theme={theme} t={t} bs={bs} onEquipped={() => {}} onAICreate={() => {}} initialMyOnly={false} />,
  toolstore: ({ theme, t }) => <LazyToolStoreView theme={theme} t={t} onNewChat={() => {}} />,
  knowledge: ({ theme, t }) => <LazyKnowledgeView theme={theme} t={t} />,
  outputs: ({ theme, t }) => <LazyKnowledgeView theme={theme} t={t} mode="outputs" />,
};

export function DetachedShell({ kind, id }) {
  const { bs, activeTheme, t } = useDetachedBase();

  useEffect(() => {
    const key = `${kind}:${id || ''}`;
    const onUnload = () => {
      if (isTauriAvailable()) void emitTauri('detach:closed', key).catch(() => {});
    };
    window.addEventListener('beforeunload', onUnload);
    return () => window.removeEventListener('beforeunload', onUnload);
  }, [kind, id]);

  const View = DETACHED_VIEWS[kind] || DETACHED_VIEWS.monitor;
  return (
    <div className={`h-screen w-screen flex flex-col bg-white text-[#1F1F1F] dark:bg-[#1B1C1D] dark:text-[#E3E3E3]`}>
      <div
        data-tauri-drag-region
        className="h-9 shrink-0 flex items-center px-3 text-[13px] font-medium select-none"
        style={{ borderBottom: '1px solid rgba(128,128,128,.2)' }}
      >
        <span data-tauri-drag-region className="pointer-events-none">{t.tearoffTitle} · {kind}</span>
      </div>
      <div className="flex-1 min-h-0 overflow-auto">
        {bs
          ? <DetachedErrorBoundary t={t}><Suspense fallback={<DetachedViewFallback />}><View id={id} theme={activeTheme} t={t} bs={bs} /></Suspense></DetachedErrorBoundary>
          : <div className="p-6 text-sm opacity-60">…</div>}
      </div>
    </div>
  );
}
