import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MessageSquare, RotateCcw, Send, X } from '../../components/icons.jsx';
import { RightDockPanel } from '../../components/layout/RightDock.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { isImeComposing } from '../../shared/ime-guard.mjs';
import { ConversationTimeline } from '../conversation/ConversationTimeline.jsx';
import {
  auxChatBusy,
  auxChatHasContent,
  normalizeAuxSnapshot,
  projectAuxChatTurns,
} from './aux-chat-state.mjs';

/**
 * 辅助对话面板（右侧 RightDock）：每个主任务挂一条独立的纯问答会话，
 * 不参与主任务的执行与上下文（桥侧 send 已内置 restrictTools），不经子
 * 代理体系、不产生第二任务入口（ADR-0006 约束）。
 *
 * 数据流：换绑/首开后 ensure(sessionId) 幂等拿到 auxId → 本地只存 auxId
 * 与 snapshot 快照；chat 域 notify（后台会话事件已路由进 per-session
 * buffer）时重拉同步快照。App 不维护任何会话状态机。
 */

const RESTART_CONFIRM_MS = 4000;

export function AuxChatPanel({ sessionId, activationKey, t, theme, onClose }) {
  const copy = t.uiAuxChat;
  const conversationCopy = t.uiConversation;
  const auxChat = bridge.available ? bridge.auxChat : null;

  const [auxId, setAuxId] = useState(null);
  const [snapshot, setSnapshot] = useState(() => normalizeAuxSnapshot(null));
  const [draft, setDraft] = useState('');
  const [sendFailed, setSendFailed] = useState(false);
  const [restartArmed, setRestartArmed] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const generationRef = useRef(0);
  const auxIdRef = useRef(null);
  const scrollRef = useRef(null);

  const pullSnapshot = useCallback((id) => {
    setSnapshot(normalizeAuxSnapshot(id && auxChat ? auxChat.snapshot(id) : null));
  }, [auxChat]);

  // 首开与主会话换绑：丢弃旧绑定并幂等 ensure 新任务的辅助会话。generation
  // 守卫挡住晚到的 ensure 结果把面板绑回上一个任务。
  useEffect(() => {
    const generation = generationRef.current + 1;
    generationRef.current = generation;
    auxIdRef.current = null;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset binding state on session switch; one-shot mirror, same pattern as SubagentTranscriptPanel
    setAuxId(null);
    setSnapshot(normalizeAuxSnapshot(null));
    setSendFailed(false);
    setRestartArmed(false);
    setDraft('');
    if (!auxChat || !sessionId) return;
    let disposed = false;
    auxChat.ensure(sessionId)
      .then((nextAuxId) => {
        if (disposed || generationRef.current !== generation) return;
        auxIdRef.current = nextAuxId;
        setAuxId(nextAuxId);
        pullSnapshot(nextAuxId);
      })
      .catch((error) => {
        console.warn('[pinvou3][aux-chat] ensure failed', error);
      });
    return () => { disposed = true; };
  }, [auxChat, sessionId, pullSnapshot]);

  // 后台辅助会话的回合事件已自动进 per-session buffer 并触发 notify；
  // 订阅 chat 域重拉同步快照即可，不新增事件监听。
  useEffect(() => {
    if (!auxChat || !bridge.state) return;
    return bridge.state.subscribeMany(['chat'], () => {
      if (auxIdRef.current) pullSnapshot(auxIdRef.current);
    });
  }, [auxChat, pullSnapshot]);

  const busy = auxChatBusy(snapshot);
  const hasContent = auxChatHasContent(snapshot);
  const turns = useMemo(
    () => (auxId ? projectAuxChatTurns(snapshot, auxId) : []),
    [snapshot, auxId],
  );

  // 新回合出现时贴底；流式增量不强制滚动，避免打断用户回看历史。
  const itemCount = snapshot.chatItems.length;
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [auxId, itemCount]);

  useEffect(() => {
    if (!restartArmed) return;
    const timer = setTimeout(() => setRestartArmed(false), RESTART_CONFIRM_MS);
    return () => clearTimeout(timer);
  }, [restartArmed]);

  const handleSend = useCallback(async () => {
    const text = draft.trim();
    if (!auxChat || !auxIdRef.current || !text || busy) return;
    setSendFailed(false);
    try {
      await auxChat.send(auxIdRef.current, text);
      setDraft('');
      pullSnapshot(auxIdRef.current);
    } catch (error) {
      console.warn('[pinvou3][aux-chat] send failed', error);
      setSendFailed(true);
    }
  }, [auxChat, draft, busy, pullSnapshot]);

  const handleComposerKeyDown = useCallback((event) => {
    if (event.key !== 'Enter' || event.shiftKey || isImeComposing(event)) return;
    event.preventDefault();
    void handleSend();
  }, [handleSend]);

  // 重开话题：两段式轻量确认（Tauri WebView2 下系统 window.confirm 不弹，
  // 仓内先例为自绘确认）→ discard 旧辅助会话 → ensure 重建 → 清空本地快照。
  const handleRestart = useCallback(async () => {
    if (!auxChat || !sessionId || restarting) return;
    if (!restartArmed) {
      setRestartArmed(true);
      return;
    }
    setRestartArmed(false);
    setRestarting(true);
    const generation = generationRef.current;
    try {
      await auxChat.discard(sessionId);
      const nextAuxId = await auxChat.ensure(sessionId);
      if (generationRef.current !== generation) return;
      auxIdRef.current = nextAuxId;
      setAuxId(nextAuxId);
      setSnapshot(normalizeAuxSnapshot(nextAuxId ? auxChat.snapshot(nextAuxId) : null));
      setDraft('');
      setSendFailed(false);
    } catch (error) {
      console.warn('[pinvou3][aux-chat] restart failed', error);
      setSendFailed(true);
    } finally {
      setRestarting(false);
    }
  }, [auxChat, sessionId, restartArmed, restarting]);

  const composerDisabled = !auxChat || !auxId || busy || restarting;

  return (
    <RightDockPanel
      panelId="aux-chat"
      activationKey={activationKey}
      className="border-l border-black/[0.06] bg-white/92 backdrop-blur-xl dark:border-white/[0.07] dark:bg-[#17181A]/96"
      dataTestId="aux-chat-panel"
    >
      <div className="h-14 shrink-0 px-3 flex items-center gap-2 border-b border-black/[0.05] dark:border-white/[0.06]">
        <MessageSquare size={15} className="shrink-0 text-gray-400" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-semibold">{copy.panelTitle}</span>
        <button
          type="button"
          data-testid="aux-chat-new-topic"
          onClick={() => { void handleRestart(); }}
          disabled={!auxChat || restarting}
          className={`shrink-0 h-7 rounded-lg px-2 flex items-center gap-1 text-[11px] transition-colors ${
            restartArmed
              ? 'bg-red-500/10 text-red-600 dark:text-red-400'
              : 'text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]'
          }`}
          aria-label={copy.newTopic}
          title={restartArmed ? copy.newTopicConfirm : copy.newTopic}
        >
          <RotateCcw size={13} />
          <span>{restartArmed ? copy.newTopicConfirm : copy.newTopic}</span>
        </button>
        <button
          type="button"
          onClick={onClose}
          className="w-7 h-7 shrink-0 rounded-lg flex items-center justify-center text-gray-400 hover:bg-black/[0.05] dark:hover:bg-white/[0.07]"
          aria-label={copy.close}
        >
          <X size={14} />
        </button>
      </div>

      <div ref={scrollRef} className="custom-scrollbar flex-1 min-h-0 overflow-y-auto px-4 py-4">
        {!hasContent && (
          <div className="space-y-2">
            <div className="rounded-xl border border-black/[0.05] bg-black/[0.02] px-3 py-2.5 text-[12px] leading-5 text-gray-500 dark:border-white/[0.07] dark:bg-white/[0.03] dark:text-gray-400">
              {copy.landingHint}
            </div>
            <div className="px-1 text-[12px] text-gray-400">{copy.emptyState}</div>
          </div>
        )}
        {hasContent && (
          <ConversationTimeline
            turns={turns}
            now={0}
            copy={conversationCopy}
          />
        )}
      </div>

      <div className="shrink-0 border-t border-black/[0.05] px-3 py-3 dark:border-white/[0.06]">
        {busy && (
          <div className="mb-2 text-[11px] text-gray-400" role="status">{copy.busyHint}</div>
        )}
        {sendFailed && (
          <div className="mb-2 text-[11px] text-red-600 dark:text-red-400" role="alert">{copy.sendFailed}</div>
        )}
        <div className={`flex items-end gap-2 rounded-xl border px-3 py-2 ${
          theme === 'dark' ? 'border-white/[0.08] bg-white/[0.03]' : 'border-black/[0.08] bg-white/60'
        }`}>
          <textarea
            rows={1}
            value={draft}
            data-testid="aux-chat-input"
            onChange={(event) => {
              setDraft(event.target.value);
              if (sendFailed) setSendFailed(false);
            }}
            onKeyDown={handleComposerKeyDown}
            placeholder={copy.inputPlaceholder}
            aria-label={copy.inputPlaceholder}
            className="custom-scrollbar max-h-32 flex-1 resize-none bg-transparent text-[13px] leading-5 outline-none placeholder:text-gray-400"
          />
          <button
            type="button"
            data-testid="aux-chat-send"
            onClick={() => { void handleSend(); }}
            disabled={composerDisabled || !draft.trim()}
            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg bg-[#0A84FF] text-white transition-opacity hover:bg-[#1677D2] disabled:opacity-40"
            aria-label={copy.send}
            title={busy ? copy.busyHint : copy.send}
          >
            <Send size={13} />
          </button>
        </div>
      </div>
    </RightDockPanel>
  );
}
