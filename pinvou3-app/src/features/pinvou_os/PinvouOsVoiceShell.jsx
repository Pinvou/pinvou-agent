import React, { useEffect, useMemo, useRef, useState } from 'react';

import { AlertTriangle, Mic, Sparkles, StopCircle, X } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { ArtifactBrowser } from '../artifacts/FilePreviewModal.jsx';
import { ArtifactCard } from '../tools/tool-common.jsx';
import { UserInputCard } from '../tools/tool-renderers.jsx';
import { PinvouOsAgentDock } from './PinvouOsAgentDock.jsx';
import { PinvouOsProjectionSurface } from './PinvouOsProjectionSurface.jsx';
import {
  activateVoiceControl,
  getVoiceControlState,
  isVoiceCaptureActive,
} from './pinvou-os-voice-control.js';
import {
  getNextTurnFeedbackDelay,
  getTurnFeedback,
  latestOpenTurnStart,
  queuedMessagePresentations,
  visibleUnqueuedUtterance,
} from './pinvou-os-interjection.js';
import './pinvou-os-voice-shell.css';

function readableText(value) {
  if (value == null) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) return value.map(readableText).filter(Boolean).join('\n');
  if (typeof value === 'object') {
    for (const key of ['text', 'content', 'markdown', 'message']) {
      if (typeof value[key] === 'string') return value[key];
    }
  }
  return '';
}

function latestAssistantPresentation(items) {
  const rows = Array.isArray(items) ? items : [];
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const item = rows[index];
    if (!item) continue;
    const role = String(item.role || item.speaker || item.type || '').toLowerCase();
    if (role.includes('user') || role.includes('tool') || role.includes('system')) continue;
    if (role.includes('reasoning') || role.includes('thinking') || role.includes('debug')) continue;
    const rawText = readableText(item);
    // Do not trim/scan the complete growing answer on every hot snapshot. The
    // bounded preview is the streaming visibility signal; terminal text is
    // normalized once after streaming ends.
    const text = item.streaming ? rawText : rawText.trim();
    if (text) {
      return {
        text,
        html: typeof item.html === 'string' ? item.html : '',
        streaming: Boolean(item.streaming),
        streamingPreviewText: typeof item.streamingPreviewText === 'string'
          ? item.streamingPreviewText
          : '',
        streamingPreviewOmitted: Boolean(item.streamingPreviewOmitted),
        streamingStructuredDraft: typeof item.streamingStructuredDraft === 'string'
          ? item.streamingStructuredDraft
          : item.streamingStructuredDraft ? 'structured' : '',
      };
    }
  }
  return {
    text: '', html: '', streaming: false,
    streamingPreviewText: '', streamingPreviewOmitted: false, streamingStructuredDraft: '',
  };
}

function latestItem(items, predicate) {
  const rows = Array.isArray(items) ? items : [];
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    if (predicate(rows[index])) return rows[index];
  }
  return null;
}

function itemIdentity(item) {
  if (!item) return '';
  return String(item.id || item.toolCallId || item.path || '');
}

function voiceStatusLabel(voiceInput, t) {
  if (voiceInput.status === 'requesting_permission') return t.voiceRequestingCancelable;
  if (voiceInput.status === 'recording') return t.voiceRecording;
  if (voiceInput.status === 'transcribing') return t.voiceTranscribingCancelable;
  if (voiceInput.status === 'failed') return voiceInput.message || t.voiceInputFailed;
  return '';
}

export function PinvouOsVoiceShell({ theme, t, bs, onSubmitPrompt }) {
  const voiceInput = (bs && bs.voiceInput) || { status: 'idle' };
  const busy = Boolean(bs && bs.busy);
  const chatItems = (bs && bs.chatItems) || [];
  const queued = (bs && bs.queued) || [];
  const turnTimeline = (bs && bs.turnTimeline) || [];
  const currentAssistant = useMemo(() => latestAssistantPresentation(chatItems), [chatItems]);
  const currentAssistantText = currentAssistant.text;
  const pendingUserInput = useMemo(
    () => latestItem(chatItems, item => item && item.type === 'user_input' && !item.resolved),
    [chatItems],
  );
  const latestArtifact = useMemo(
    () => latestItem(chatItems, item => item && item.type === 'artifact_card' && item.path),
    [chatItems],
  );
  const [interactionStarted, setInteractionStarted] = useState(false);
  const [baselineAssistantText, setBaselineAssistantText] = useState('');
  const [lastUtterance, setLastUtterance] = useState('');
  const [submitError, setSubmitError] = useState('');
  const [baselineArtifactId, setBaselineArtifactId] = useState('');
  const [artifactBrowser, setArtifactBrowser] = useState(null);
  const [turnFeedbackNow, setTurnFeedbackNow] = useState(() => Date.now());
  const submissionRef = useRef('');

  const requesting = voiceInput.status === 'requesting_permission';
  const recording = voiceInput.status === 'recording';
  const transcribing = voiceInput.status === 'transcribing';
  const voiceFailed = voiceInput.status === 'failed';
  // Agent work and voice capture are independent. In particular, a completion
  // event must never take away the user's stop/cancel control or ability to interject.
  const voiceBridge = bridge.available && bridge.voice ? bridge.voice : null;
  const voiceControl = getVoiceControlState(voiceInput.status, voiceBridge);
  const captureActive = isVoiceCaptureActive(voiceInput.status);
  const visibleAssistantText = !captureActive
    && interactionStarted
    && currentAssistantText
    && currentAssistantText !== baselineAssistantText
    ? currentAssistantText
    : '';
  const visibleAssistant = visibleAssistantText ? currentAssistant : null;
  const visibleArtifact = !captureActive
    && !busy
    && latestArtifact
    && (!interactionStarted || itemIdentity(latestArtifact) !== baselineArtifactId)
    ? latestArtifact
    : null;
  const openTurnStart = useMemo(() => latestOpenTurnStart(turnTimeline), [turnTimeline]);
  const queuedMessages = useMemo(() => queuedMessagePresentations(queued), [queued]);
  const visibleLastUtterance = useMemo(
    () => visibleUnqueuedUtterance(lastUtterance, queued),
    [lastUtterance, queued],
  );
  const turnFeedback = busy ? getTurnFeedback(turnTimeline, turnFeedbackNow) : null;

  useEffect(() => {
    if (!busy || !openTurnStart) return undefined;

    let timerId;
    const refreshAtNextBoundary = () => {
      const now = Date.now();
      setTurnFeedbackNow(now);
      const delay = getNextTurnFeedbackDelay(turnTimeline, now);
      if (delay != null) {
        timerId = window.setTimeout(refreshAtNextBoundary, delay + 25);
      }
    };

    refreshAtNextBoundary();
    return () => window.clearTimeout(timerId);
  }, [busy, openTurnStart && openTurnStart.turn_id, openTurnStart && openTurnStart.timestamp]);

  async function submitRecognizedText(text) {
    const prompt = String(text || '').trim();
    if (!prompt || submissionRef.current === prompt) return;
    submissionRef.current = prompt;
    setLastUtterance(prompt);
    setSubmitError('');
    try {
      await onSubmitPrompt(prompt);
    } catch (error) {
      submissionRef.current = '';
      setSubmitError(String(error || t.messageFailed));
    }
  }

  function beginVoiceInput() {
    if (!voiceBridge || !voiceBridge.startVoiceInput) return;
    setInteractionStarted(true);
    setBaselineAssistantText(currentAssistantText);
    setBaselineArtifactId(itemIdentity(latestArtifact));
    setLastUtterance('');
    setSubmitError('');
    submissionRef.current = '';
    if (voiceFailed && voiceBridge.clearVoiceInput) voiceBridge.clearVoiceInput();
    voiceBridge.startVoiceInput('', text => {
      void submitRecognizedText(text);
    });
  }

  function handleMicrophoneClick() {
    activateVoiceControl(voiceControl, voiceBridge, beginVoiceInput);
  }

  const statusLabel = voiceStatusLabel(voiceInput, t);
  const turnFeedbackLabel = turnFeedback
    ? turnFeedback.phase === 'extended'
      ? t.voiceTurnFeedbackExtended
      : turnFeedback.phase === 'long'
        ? t.voiceTurnFeedbackLong
        : t.voiceTurnFeedbackReady
    : '';
  const openArtifactBrowser = request => {
    setArtifactBrowser(current => current || request);
  };

  const removeQueuedMessage = id => {
    if (!bridge.available || !bridge.chat || typeof bridge.chat.removeQueued !== 'function') return;
    const removed = queued.find(item => item && item.id === id);
    if (removed) {
      setLastUtterance(current => visibleUnqueuedUtterance(current, [removed]));
    }
    bridge.chat.removeQueued(id);
  };

  return (
    <div
      className={`pinvou-os-voice-shell ${theme === 'dark' ? 'dark' : ''} ${transcribing ? 'is-voice-transcribing' : ''}`}
      data-testid="app-root"
      data-current-view="pinvou-os-voice"
    >
      <div className="pinvou-os-voice-veil" aria-hidden="true" />

      <div className="pinvou-os-canvas-stage">
        <PinvouOsProjectionSurface
          t={t}
          compact={Boolean(visibleAssistantText || (busy && !captureActive) || submitError)}
        />
      </div>

      {(pendingUserInput || visibleArtifact || visibleAssistantText || (busy && !captureActive) || submitError) && (
        <main className="pinvou-os-answer-stage" aria-live="polite">
          {submitError ? (
            <div className="pinvou-os-error-card">
              <AlertTriangle size={18} />
              <span>{submitError}</span>
            </div>
          ) : pendingUserInput ? (
            <div className="pinvou-os-system-card" data-testid="pinvou-os-user-input-card">
              <UserInputCard item={pendingUserInput} t={t} />
            </div>
          ) : visibleAssistantText || visibleArtifact ? (
            <div className="pinvou-os-canvas-content">
              {visibleAssistantText && (
                visibleAssistant.streaming && visibleAssistant.streamingStructuredDraft ? (
                  <article className="pinvou-os-answer-markdown pinvou-os-answer-stream-text">
                    {visibleAssistant.streamingStructuredDraft === 'scheduled-task-draft'
                      ? t.uiChatExtra.draftingScheduled
                      : t.cpDesigning}
                  </article>
                ) : visibleAssistant.streaming && visibleAssistant.streamingPreviewText ? (
                  <article className="pinvou-os-answer-markdown pinvou-os-answer-stream-text">
                    {visibleAssistant.streamingPreviewOmitted && (
                      <span className="pinvou-os-stream-omitted" aria-hidden="true">…</span>
                    )}
                    {visibleAssistant.streamingPreviewText}
                  </article>
                ) : (
                  <article
                    className="pinvou-os-answer-markdown"
                    dangerouslySetInnerHTML={{ __html: visibleAssistant.html }}
                  />
                )
              )}
              {visibleArtifact && (
                <div data-testid="pinvou-os-artifact-card">
                  <ArtifactCard item={visibleArtifact} theme={theme} t={t} onOpen={openArtifactBrowser} />
                </div>
              )}
              {busy && turnFeedbackLabel && (
                <p
                  className="pinvou-os-turn-feedback is-inline"
                  data-feedback-phase={turnFeedback.phase}
                  data-testid="pinvou-os-turn-feedback"
                >
                  {turnFeedbackLabel}
                </p>
              )}
            </div>
          ) : (
            <div className="pinvou-os-thinking" data-feedback-phase={turnFeedback ? turnFeedback.phase : 'initial'}>
              <div className="pinvou-os-thinking-dots" aria-hidden="true">
                <span />
                <span />
                <span />
              </div>
              {turnFeedbackLabel && (
                <p className="pinvou-os-turn-feedback" data-testid="pinvou-os-turn-feedback">
                  {turnFeedbackLabel}
                </p>
              )}
            </div>
          )}
        </main>
      )}

      <div className="pinvou-os-voice-dock">
        {(queuedMessages.length > 0 || (visibleLastUtterance && busy)) && (
          <div className="pinvou-os-interjection-tray">
            {queuedMessages.length > 0 && (
              <div
                className="pinvou-os-queued-inputs"
                role="list"
                aria-label={t.voiceQueuedRegion}
                aria-live="polite"
                data-testid="pinvou-os-queued-inputs"
              >
                {queuedMessages.map(message => (
                  <div
                    className="pinvou-os-queued-input"
                    key={message.id}
                    role="listitem"
                    data-queue-id={message.id}
                    data-testid="pinvou-os-queued-input"
                  >
                    <span className="pinvou-os-queued-state">
                      <span className="pinvou-os-queued-dot" aria-hidden="true" />
                      {t.voiceQueuedStatus}
                    </span>
                    <span className="pinvou-os-queued-text" title={message.text}>{message.text}</span>
                    <button
                      type="button"
                      className="pinvou-os-queued-cancel"
                      onClick={() => removeQueuedMessage(message.id)}
                      aria-label={t.voiceQueuedCancel(message.text)}
                      data-testid="pinvou-os-queued-cancel"
                    >
                      <span aria-hidden="true"><X size={17} /></span>
                    </button>
                  </div>
                ))}
              </div>
            )}
            {visibleLastUtterance && busy && (
              <div className="pinvou-os-utterance" aria-live="polite">{visibleLastUtterance}</div>
            )}
          </div>
        )}
        <button
          type="button"
          className={`pinvou-os-mic ${recording ? 'is-recording' : ''} ${transcribing ? 'is-transcribing' : ''} ${voiceFailed ? 'is-failed' : ''}`}
          onClick={handleMicrophoneClick}
          disabled={voiceControl.disabled}
          aria-label={recording ? t.voiceStop : requesting || transcribing ? t.voiceCancel : voiceFailed ? t.voiceRetry : t.voiceStart}
          data-testid="pinvou-os-microphone"
        >
          <span className="pinvou-os-mic-ripple" aria-hidden="true" />
          {transcribing
            ? <Sparkles size={30} className="pinvou-os-mic-processing" />
            : recording
              ? <StopCircle size={31} />
              : <Mic size={31} />}
        </button>
        {statusLabel && (
          <div className={`pinvou-os-voice-status ${voiceFailed ? 'is-failed' : ''}`} aria-live="polite">
            {statusLabel}
          </div>
        )}
      </div>

      <PinvouOsAgentDock theme={theme} t={t} />

      {artifactBrowser && (
        <ArtifactBrowser
          path={artifactBrowser.path}
          sessionId={artifactBrowser.sessionId}
          title={artifactBrowser.title}
          originRect={artifactBrowser.originRect}
          returnFocus={artifactBrowser.returnFocus}
          theme={theme}
          t={t}
          onClose={() => setArtifactBrowser(null)}
        />
      )}
    </div>
  );
}
