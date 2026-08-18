import React, { useEffect, useRef, useState } from 'react';
import { KnowledgeView } from '../features/knowledge/KnowledgeView.jsx';
import { MonitorView } from '../features/monitor/MonitorView.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { ToolStoreView } from '../features/tools/ToolStoreView.jsx';
import { CardPoolView } from '../features/personas/Personas.jsx';
import { CodexAcpView } from '../features/codex/CodexAcpView.jsx';
import { useBridgeState } from '../hooks/useBridge.js';
import { emitTauri, invokeTauri, isTauriAvailable, listenTauri } from '../platform/tauri/client.js';
import { dict, initialSystemLanguage, TAG_TO_LANG } from '../shared/i18n.js';

function useDetachedBase() {
  const bs = useBridgeState([
    'platform', 'sessions', 'chat', 'voice', 'knowledge', 'scheduled', 'monitor',
    'settings', 'personas',
  ]);
  const [language, setLanguage] = useState(initialSystemLanguage);
  const [activeTheme, setActiveTheme] = useState('dark');
  const initRef = useRef(false);

  useEffect(() => {
    if (initRef.current || !bs || !bs.settings) return;
    const lang = TAG_TO_LANG[bs.settings.language];
    if (lang) setLanguage(lang);
    setActiveTheme(bs.settings.theme === 'liquid-light' ? 'light' : 'dark');
    initRef.current = true;
  }, [bs]);
  useEffect(() => {
    document.documentElement.classList.toggle('dark', activeTheme === 'dark');
  }, [activeTheme]);

  return { bs, activeTheme, t: dict[language] };
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
        const next = await invokeTauri('list_codex_acp_sessions');
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
  if (!sessions.some(session => session && session.id === id)) {
    return <div className="p-6 text-sm opacity-70">{t.uiMainApp.detachedSessionMissing}</div>;
  }
  return (
    <CodexAcpView
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
  );
}

// Reuse the same feature views as the main window. Cross-view navigation is a
// no-op because a detached window intentionally owns one view only.
const DETACHED_VIEWS = {
  session: ({ theme, t, bs }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={() => {}} justInstalledTool={null} setJustInstalledTool={() => {}} onGotoSettings={() => {}} onGotoTools={() => {}} />,
  'codex-session': ({ id, theme, t, bs }) => <DetachedCodexSessionView id={id} theme={theme} t={t} bs={bs} />,
  monitor: ({ theme, t, bs }) => <MonitorView theme={theme} t={t} bs={bs} />,
  cardpool: ({ theme, t, bs }) => <CardPoolView theme={theme} t={t} bs={bs} onEquipped={() => {}} onAICreate={() => {}} initialMyOnly={false} />,
  toolstore: ({ theme, t }) => <ToolStoreView theme={theme} t={t} onNewChat={() => {}} />,
  knowledge: ({ theme, t }) => <KnowledgeView theme={theme} t={t} />,
  outputs: ({ theme, t }) => <KnowledgeView theme={theme} t={t} mode="outputs" />,
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
          ? <DetachedErrorBoundary t={t}><View id={id} theme={activeTheme} t={t} bs={bs} /></DetachedErrorBoundary>
          : <div className="p-6 text-sm opacity-60">…</div>}
      </div>
    </div>
  );
}
