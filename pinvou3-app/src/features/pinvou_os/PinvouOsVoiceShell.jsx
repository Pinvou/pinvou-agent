import React, { useMemo, useRef, useState } from 'react';

import { AlertTriangle, Mic, Sparkles, StopCircle } from '../../components/icons.jsx';
import { bridge } from '../../hooks/useBridge.js';
import { renderMarkdown } from '../../shared/markdown-renderer.js';
import { ArtifactBrowser } from '../artifacts/FilePreviewModal.jsx';
import { ArtifactCard } from '../tools/tool-common.jsx';
import { UserInputCard } from '../tools/tool-renderers.jsx';
import { PinvouOsAgentDock } from './PinvouOsAgentDock.jsx';
import { PinvouOsProjectionSurface } from './PinvouOsProjectionSurface.jsx';
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

function latestAssistantText(items) {
  const rows = Array.isArray(items) ? items : [];
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const item = rows[index];
    if (!item) continue;
    const role = String(item.role || item.speaker || item.type || '').toLowerCase();
    if (role.includes('user') || role.includes('tool') || role.includes('system')) continue;
    if (role.includes('reasoning') || role.includes('thinking') || role.includes('debug')) continue;
    const text = readableText(item).trim();
    if (text) return text;
  }
  return '';
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
  if (voiceInput.status === 'requesting_permission') return t.voiceRequesting;
  if (voiceInput.status === 'recording') return t.voiceRecording;
  if (voiceInput.status === 'transcribing') return t.voiceTranscribing;
  if (voiceInput.status === 'failed') return voiceInput.message || t.voiceInputFailed;
  return '';
}

export function PinvouOsVoiceShell({ theme, t, bs, onSubmitPrompt }) {
  const voiceInput = (bs && bs.voiceInput) || { status: 'idle' };
  const busy = Boolean(bs && bs.busy);
  const chatItems = (bs && bs.chatItems) || [];
  const currentAssistantText = useMemo(() => latestAssistantText(chatItems), [chatItems]);
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
  const submissionRef = useRef('');

  const requesting = voiceInput.status === 'requesting_permission';
  const recording = voiceInput.status === 'recording';
  const transcribing = voiceInput.status === 'transcribing';
  const voiceFailed = voiceInput.status === 'failed';
  const voiceLocked = transcribing || busy;
  const visibleAssistantText = interactionStarted
    && currentAssistantText
    && currentAssistantText !== baselineAssistantText
    ? currentAssistantText
    : '';
  const visibleArtifact = !busy
    && latestArtifact
    && (!interactionStarted || itemIdentity(latestArtifact) !== baselineArtifactId)
    ? latestArtifact
    : null;

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
    if (!bridge.available || !bridge.voice || !bridge.voice.startVoiceInput || voiceLocked) return;
    setInteractionStarted(true);
    setBaselineAssistantText(currentAssistantText);
    setBaselineArtifactId(itemIdentity(latestArtifact));
    setLastUtterance('');
    setSubmitError('');
    submissionRef.current = '';
    if (voiceFailed && bridge.voice.clearVoiceInput) bridge.voice.clearVoiceInput();
    bridge.voice.startVoiceInput('', text => {
      void submitRecognizedText(text);
    });
  }

  function handleMicrophoneClick() {
    if (requesting) {
      bridge.voice.cancelVoiceInput();
      return;
    }
    if (recording) {
      // 现有语音桥约定：录音中再次调用即结束录音并进入 Qwen3-ASR。
      bridge.voice.startVoiceInput('', () => {});
      return;
    }
    beginVoiceInput();
  }

  const statusLabel = voiceStatusLabel(voiceInput, t);
  const answerHtml = useMemo(
    () => visibleAssistantText ? renderMarkdown(visibleAssistantText) : '',
    [visibleAssistantText],
  );

  const openArtifactBrowser = request => {
    setArtifactBrowser(current => current || request);
  };

  return (
    <div
      className={`pinvou-os-voice-shell ${theme === 'dark' ? 'dark' : ''}`}
      data-testid="app-root"
      data-current-view="pinvou-os-voice"
    >
      <div className="pinvou-os-voice-veil" aria-hidden="true" />

      <div className="pinvou-os-canvas-stage">
        <PinvouOsProjectionSurface t={t} compact={Boolean(visibleAssistantText || busy || submitError)} />
      </div>

      {(pendingUserInput || visibleArtifact || visibleAssistantText || busy || submitError) && (
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
                <article
                  className="pinvou-os-answer-markdown"
                  dangerouslySetInnerHTML={{ __html: answerHtml }}
                />
              )}
              {visibleArtifact && (
                <div data-testid="pinvou-os-artifact-card">
                  <ArtifactCard item={visibleArtifact} theme={theme} t={t} onOpen={openArtifactBrowser} />
                </div>
              )}
            </div>
          ) : (
            <div className="pinvou-os-thinking">
              <span />
              <span />
              <span />
            </div>
          )}
        </main>
      )}

      <div className="pinvou-os-voice-dock">
        {lastUtterance && busy && (
          <div className="pinvou-os-utterance" aria-live="polite">{lastUtterance}</div>
        )}
        <button
          type="button"
          className={`pinvou-os-mic ${recording ? 'is-recording' : ''} ${transcribing ? 'is-transcribing' : ''} ${voiceFailed ? 'is-failed' : ''}`}
          onClick={handleMicrophoneClick}
          disabled={voiceLocked}
          aria-label={recording ? t.voiceStop : transcribing ? t.voiceTranscribing : voiceFailed ? t.voiceRetry : t.voiceStart}
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
