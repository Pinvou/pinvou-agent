import React, { useEffect, useRef, useState } from 'react';
import { KnowledgeView } from '../features/knowledge/KnowledgeView.jsx';
import { MonitorView } from '../features/monitor/MonitorView.jsx';
import { ChatView } from '../features/chat/ChatView.jsx';
import { ToolStoreView } from '../features/tools/ToolStoreView.jsx';
import { CardPoolView } from '../features/personas/Personas.jsx';
import { WorkflowView } from '../features/workflow/WorkflowView.jsx';
import { useBridgeState } from '../hooks/useBridge.js';
import { emitTauri, isTauriAvailable } from '../platform/tauri/client.js';
import { dict, TAG_TO_LANG } from '../shared/i18n.js';

function useDetachedBase() {
  const bs = useBridgeState([
    'platform', 'sessions', 'chat', 'voice', 'knowledge', 'scheduled', 'monitor',
    'settings', 'personas',
    'workflow',
  ]);
  const [language, setLanguage] = useState('zh');
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

  render() {
    if (this.state.err) {
      const message = String((this.state.err && this.state.err.message) || this.state.err);
      return <div className="p-6 text-sm opacity-70">{this.props.t.uiMainApp.panelLoadFailed(message)}</div>;
    }
    return this.props.children;
  }
}

// Reuse the same feature views as the main window. Cross-view navigation is a
// no-op because a detached window intentionally owns one view only.
const DETACHED_VIEWS = {
  session: ({ theme, t, bs }) => <ChatView theme={theme} t={t} bs={bs} prefill="" onPrefillConsumed={() => {}} onOpenEditor={() => {}} justInstalledTool={null} setJustInstalledTool={() => {}} onGotoSettings={() => {}} onGotoTools={() => {}} />,
  workflow: ({ theme, t, bs }) => <WorkflowView theme={theme} t={t} bs={bs} />,
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
  const isDark = activeTheme === 'dark';
  return (
    <div className={`h-screen w-screen flex flex-col ${isDark ? 'bg-[#1B1C1D] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F]'}`}>
      <div
        data-tauri-drag-region
        className="h-9 shrink-0 flex items-center px-3 text-[13px] font-medium select-none"
        style={{ borderBottom: '1px solid rgba(128,128,128,.2)' }}
      >
        <span data-tauri-drag-region className="pointer-events-none">{t.tearoffTitle} · {kind}</span>
      </div>
      <div className="flex-1 min-h-0 overflow-auto">
        {bs
          ? <DetachedErrorBoundary t={t}><View theme={activeTheme} t={t} bs={bs} /></DetachedErrorBoundary>
          : <div className="p-6 text-sm opacity-60">…</div>}
      </div>
    </div>
  );
}
