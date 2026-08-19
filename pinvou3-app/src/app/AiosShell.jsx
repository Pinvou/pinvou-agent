import React, { useEffect, useMemo, useRef, useState } from 'react';
import pinvouAppIcon from '../../src-tauri/icons/icon.png';
import { renderMarkdown } from '../shared/markdown-renderer.js';
import '../styles/aios.css';

const CARD_TONE_CLASSES = ['aios-icon-blue', 'aios-icon-green', 'aios-icon-purple', 'aios-icon-orange', 'aios-icon-pink'];
const CARD_ICONS = ['fa-wand-magic-sparkles', 'fa-bolt', 'fa-layer-group', 'fa-comment-dots'];
const VOICE_WAVE_BARS = [16, 26, 38, 46, 50, 46, 38, 26, 16];

function formatClock() {
  const now = new Date();
  let hours = now.getHours();
  let minutes = now.getMinutes();
  const ampm = hours >= 12 ? '下午' : '上午';

  hours %= 12;
  hours = hours || 12;
  minutes = minutes < 10 ? `0${minutes}` : String(minutes);

  return `${ampm} ${hours}:${minutes}`;
}

function greetingFor(copy) {
  const hour = new Date().getHours();
  if (hour < 12) return copy.goodMorning;
  if (hour < 18) return copy.goodAfternoon;
  return copy.goodEvening;
}

function truncateText(value, max = 120) {
  const text = String(value || '').replace(/\s+/g, ' ').trim();
  if (text.length <= max) return text;
  return `${text.slice(0, max - 1)}...`;
}

function readableText(value) {
  if (value == null) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) return value.map(readableText).filter(Boolean).join('\n');
  if (typeof value === 'object') {
    if (typeof value.text === 'string') return value.text;
    if (typeof value.content === 'string') return value.content;
    if (typeof value.markdown === 'string') return value.markdown;
    if (typeof value.message === 'string') return value.message;
    if (typeof value.title === 'string') return value.title;
    if (typeof value.toolName === 'string') return value.phase ? `${value.toolName}: ${value.phase}` : value.toolName;
    if (typeof value.phase === 'string') return value.phase;
    try {
      return JSON.stringify(value);
    } catch {
      return '';
    }
  }
  return '';
}

function parseJsonObject(text) {
  if (typeof text !== 'string') return null;
  const trimmed = text.trim();
  if (!trimmed || !trimmed.startsWith('{')) return null;
  try {
    const parsed = JSON.parse(trimmed);
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function normalizeUserInputItem(item) {
  if (!item) return null;
  if (item.type === 'user_input' && Array.isArray(item.questions)) return item;

  const candidates = [item.text, item.content, item.markdown, item.message, item.title];
  for (const candidate of candidates) {
    const parsed = parseJsonObject(readableText(candidate));
    if (parsed?.type === 'user_input' && Array.isArray(parsed.questions)) {
      return {
        ...parsed,
        id: item.id || parsed.id || parsed.toolCallId,
        resolved: item.resolved ?? parsed.resolved,
        cardState: item.cardState || parsed.cardState,
        submitting: item.submitting || parsed.submitting,
        error: item.error || parsed.error,
      };
    }
  }
  return null;
}

function shouldHideToolBubble(item) {
  if (!item) return false;
  const role = String(item.type || item.role || '').toLowerCase();
  const name = String(item.name || item.toolName || item.title || '').toLowerCase();
  if (!role.includes('tool') && !name) return false;
  return name === 'request_user_input' || bubbleText(item).toLowerCase().includes('request_user_input');
}

function taskPreview(task, copy) {
  if (!task) return '';
  return truncateText(task.subtitle || task.preview || task.title || copy.untitledTask);
}

function taskStatusLabel(task, copy) {
  if (task?.status === 'failed') return copy.taskFailed || '失败';
  if (task?.status === 'auth_checking') return copy.ctripStatus?.authChecking || '检查授权';
  if (task?.status === 'needs_auth') return copy.ctripStatus?.needsAuth || '需要授权';
  if (task?.status === 'authorizing') return copy.ctripStatus?.authorizing || '授权中';
  if (task?.status === 'auth_failed') return copy.ctripStatus?.authFailed || '授权失败';
  if (task?.status === 'needs_details') return copy.ctripStatus?.needsDetails || '需要选择';
  if (task?.status === 'searching') return copy.ctripStatus?.searching || '正在查询';
  if (task?.status === 'wendao_ready') return copy.ctripStatus?.wendaoReady || '已返回结果';
  if (task?.status === 'browser_prepare') return copy.ctripStatus?.browserPrepare || '准备打开';
  if (task?.status === 'browser_searching') return copy.ctripStatus?.browserSearching || '网页协助';
  if (task?.status === 'browser_order_review') return copy.ctripStatus?.browserOrderReview || '订单核对';
  if (task?.status === 'submit_confirmation_required') return copy.ctripStatus?.submitConfirmationRequired || '提交确认';
  if (task?.status === 'payment_required') return copy.ctripStatus?.paymentRequired || '待支付';
  if (task?.status === 'user_action_required') return copy.ctripStatus?.userActionRequired || '需要接管';
  if (task?.status === 'browser_blocked') return copy.ctripStatus?.browserBlocked || '协助受阻';
  if (task?.status === 'browser_cancelled') return copy.ctripStatus?.browserCancelled || '已结束';
  if (task?.status === 'handoff_done') return copy.ctripStatus?.handoffDone || '已接管';
  if (task?.status === 'needs_choice') return copy.ctripStatus?.needsChoice || '需要选择';
  if (task?.status === 'validating') return copy.ctripStatus?.validating || '验价中';
  if (task?.status === 'order_confirm') return copy.ctripStatus?.orderConfirm || '待确认订单';
  if (task?.status === 'submitting_order') return copy.ctripStatus?.submittingOrder || '提交中';
  if (task?.status === 'submit_failed') return copy.ctripStatus?.submitFailed || '提交失败';
  if (task?.status === 'order_created') return copy.ctripStatus?.orderCreated || '已完成';
  if (task?.status === 'paused') return copy.ctripStatus?.paused || '已暂停';
  if (task?.status === 'processing' || task?.optimistic) return copy.processing;
  if (task?.needsUserInput) return copy.choiceNeeded || '需要选择';
  if (task?.hasArtifact) return copy.artifactReady || '已生成产物';
  if (task?.resultPreview) return copy.resultReady || '已生成结果';
  return '';
}

function taskStatusTone(task) {
  if (task?.status === 'failed' || task?.status === 'auth_failed' || task?.status === 'submit_failed' || task?.status === 'browser_blocked') return 'red';
  if (['needs_auth', 'needs_details', 'needs_choice', 'order_confirm', 'paused', 'user_action_required', 'submit_confirmation_required', 'payment_required', 'browser_cancelled'].includes(task?.status) || task?.needsUserInput) return 'orange';
  if (task?.hasArtifact || task?.resultPreview) return 'green';
  if (['order_created', 'wendao_ready', 'handoff_done'].includes(task?.status)) return 'green';
  if (task?.status === 'processing' || task?.status === 'browser_prepare' || task?.status === 'browser_searching' || task?.status === 'browser_order_review' || task?.optimistic) return 'blue';
  return '';
}

function iconForTask(task, index) {
  if (task?.taskKind === 'scheduled') return 'fa-tasks';
  if (task?.taskKind === 'codex') return 'fa-code';
  if (task?.working) return 'fa-circle-notch fa-spin';
  return CARD_ICONS[index % CARD_ICONS.length];
}

function bubbleText(item) {
  if (!item) return '';
  return readableText(item.text || item.content || item.markdown || item.message || item.title || item);
}

function bubbleRole(item) {
  const role = String(item?.role || item?.speaker || item?.type || '').toLowerCase();
  if (role.includes('user')) return 'user';
  return 'ai';
}

function timelineMetaForItem(item, copy) {
  const role = bubbleRole(item);
  const type = String(item?.type || item?.role || '').toLowerCase();
  const name = String(item?.name || item?.toolName || item?.title || '').trim();
  const text = bubbleText(item);

  if (role === 'user') {
    return {
      icon: 'fa-arrow-up',
      title: '收到任务',
      desc: truncateText(text, 56),
      tone: 'blue',
    };
  }
  if (type.includes('tool')) {
    return {
      icon: 'fa-screwdriver-wrench',
      title: name ? `使用 ${name}` : '正在使用工具',
      desc: '正在获取信息或生成产物',
      tone: 'purple',
    };
  }
  return {
    icon: 'fa-check',
    title: '生成了结果',
    desc: truncateText(cleanMarkdownText(text), 70) || copy.processing,
    tone: 'green',
  };
}

function AiosTimelineItem({ meta, active = false }) {
  return (
    <div className={`aios-timeline-item ${active ? 'active' : ''}`}>
      <div className={`aios-timeline-dot aios-timeline-${meta.tone || 'blue'}`}>
        <i className={`fas ${meta.icon}`} />
      </div>
      <div className="aios-timeline-copy">
        <div className="aios-timeline-title">{meta.title}</div>
        {meta.desc && <div className="aios-timeline-desc">{meta.desc}</div>}
      </div>
    </div>
  );
}

function questionId(question, index) {
  return String(question?.id || question?.header || `q-${index}`);
}

function optionLabel(option) {
  return String(option?.label ?? option?.value ?? '').trim();
}

function optionValue(option) {
  return String(option?.value ?? option?.label ?? '').trim();
}

function AiosUserInputCard({ item, copy, onSubmit, onCancel }) {
  const questions = Array.isArray(item?.questions) ? item.questions : [];
  const [selected, setSelected] = useState({});
  const [freeText, setFreeText] = useState({});
  const [localSubmitting, setLocalSubmitting] = useState(false);
  const conversationCopy = copy.root?.uiConversation || {};
  const otherLabel = conversationCopy.otherAnswer || copy.root?.otherAnswer || '其他';
  const placeholder = conversationCopy.inputPlaceholder || '请输入';
  const submitting = localSubmitting || item?.submitting;
  const resolved = item?.resolved || item?.cardState === 'submitted' || item?.cardState === 'cancelled';
  const cancelled = item?.cardState === 'cancelled';

  function selectOption(qid, option, multiSelect) {
    const choice = { label: optionLabel(option), value: optionValue(option) };
    if (!choice.label && !choice.value) return;
    setSelected(prev => {
      const current = Array.isArray(prev[qid]) ? prev[qid] : [];
      if (!multiSelect) return { ...prev, [qid]: [choice] };
      const exists = current.some(entry => entry.value === choice.value && entry.label === choice.label);
      return {
        ...prev,
        [qid]: exists
          ? current.filter(entry => !(entry.value === choice.value && entry.label === choice.label))
          : [...current, choice],
      };
    });
  }

  function buildAnswers() {
    return questions.flatMap((question, index) => {
      const qid = questionId(question, index);
      const picked = Array.isArray(selected[qid]) ? selected[qid] : [];
      const typed = String(freeText[qid] || '').trim();
      const answers = picked.map(choice => ({
        id: qid,
        label: choice.label || choice.value,
        value: choice.value || choice.label,
      }));
      if (typed && question.allow_free_text !== false) {
        answers.push({ id: qid, label: otherLabel, value: typed, other: true });
      }
      return answers;
    });
  }

  async function submitCard() {
    const answers = buildAnswers();
    if (!answers.length || !onSubmit || resolved || submitting) return;
    const summaryQuestions = answers.map(answer => (
      questions.find((question, index) => questionId(question, index) === answer.id) || questions[0] || {}
    ));
    setLocalSubmitting(true);
    try {
      await onSubmit(item, answers, summaryQuestions);
    } finally {
      setLocalSubmitting(false);
    }
  }

  async function cancelCard() {
    if (!onCancel || resolved || submitting) return;
    setLocalSubmitting(true);
    try {
      await onCancel(item);
    } finally {
      setLocalSubmitting(false);
    }
  }

  return (
    <div className="aios-input-card">
      <div className="aios-input-card-title">
        <i className="fas fa-list-check" />
        <span>{copy.root?.choiceTitle || 'Agent 需要你的选择'}</span>
      </div>
      {questions.map((question, index) => {
        const qid = questionId(question, index);
        const options = (Array.isArray(question.options) ? question.options : [])
          .filter(option => {
            const label = optionLabel(option).toLowerCase();
            return label && label !== 'other' && label !== '其他' && label !== '其它';
          });
        const multiSelect = Boolean(question.multi_select);
        const picked = Array.isArray(selected[qid]) ? selected[qid] : [];

        return (
          <div key={qid} className="aios-input-question">
            {question.header && <div className="aios-input-question-header">{question.header}</div>}
            <div className="aios-input-question-text">{question.question || question.header || `Q${index + 1}`}</div>
            {options.length > 0 && (
              <div className="aios-input-options">
                {options.map((option, optionIndex) => {
                  const label = optionLabel(option);
                  const value = optionValue(option);
                  const active = picked.some(entry => entry.value === value && entry.label === label);
                  return (
                    <button
                      key={`${qid}-${value || label || optionIndex}`}
                      type="button"
                      className={`aios-input-option ${active ? 'active' : ''}`}
                      disabled={resolved || submitting}
                      onClick={() => selectOption(qid, option, multiSelect)}
                    >
                      <span>{label}</span>
                      {option.description && <small>{option.description}</small>}
                    </button>
                  );
                })}
              </div>
            )}
            {question.allow_free_text !== false && (
              <input
                className="aios-input-free-text"
                value={freeText[qid] || ''}
                disabled={resolved || submitting}
                placeholder={placeholder}
                onChange={event => setFreeText(prev => ({ ...prev, [qid]: event.target.value }))}
              />
            )}
          </div>
        );
      })}
      {item?.error && <div className="aios-input-error">{item.error}</div>}
      {resolved ? (
        <div className="aios-input-status">
          <i className={`fas ${cancelled ? 'fa-ban' : 'fa-check'}`} />
          {cancelled ? (copy.root?.canceled || '已取消') : (copy.root?.submitted || '已提交')}
        </div>
      ) : (
        <div className="aios-input-actions">
          <button type="button" className="aios-input-cancel" disabled={submitting} onClick={cancelCard}>
            {copy.root?.cancel || '取消'}
          </button>
          <button type="button" className="aios-input-submit" disabled={submitting || !buildAnswers().length} onClick={submitCard}>
            {submitting ? copy.processing : (copy.root?.submit || '提交')}
          </button>
        </div>
      )}
    </div>
  );
}

function AiosComposer({
  copy,
  placeholder,
  compact = false,
  voiceOnly = false,
  disabled = false,
  autoSubmitVoice = false,
  voiceInput,
  onVoiceClick,
  onCancelVoiceInput,
  onClearVoiceInput,
  onSubmit,
}) {
  const [value, setValue] = useState('');
  return (
    <AiosVoiceInputPill
      copy={copy}
      value={value}
      setValue={setValue}
      placeholder={placeholder}
      disabled={disabled}
      compact={compact}
      voiceOnly={voiceOnly}
      voiceInput={voiceInput}
      autoSubmitVoice={autoSubmitVoice}
      onVoiceClick={onVoiceClick}
      onCancelVoiceInput={onCancelVoiceInput}
      onClearVoiceInput={onClearVoiceInput}
      onSubmit={onSubmit}
    />
  );
}

function voiceActiveStatus(status) {
  return status === 'requesting_permission' || status === 'recording' || status === 'transcribing' || status === 'postprocessing';
}

function voiceStatusText(voiceInput, copy, mode) {
  const status = voiceInput?.status;
  if (status === 'requesting_permission') return copy.root?.voiceRequesting || '正在请求麦克风权限…';
  if (status === 'recording') return mode === 'edit' ? '正在录制修改指令' : '正在录音';
  if (status === 'transcribing') return copy.root?.voiceTranscribing || '正在识别语音…';
  if (status === 'postprocessing') return voiceInput?.message || '正在按语音编辑草稿…';
  if (status === 'failed') return voiceInput?.message || copy.root?.voiceInputFailed || '语音输入失败';
  return '';
}

function AiosVoiceWave({ muted = false }) {
  return (
    <div className={`aios-voice-wave ${muted ? 'is-silent' : ''}`} aria-hidden="true">
      {VOICE_WAVE_BARS.map((height, index) => (
        <span
          key={`${height}-${index}`}
          className={`bar-${index + 1}`}
          style={{ height: `${height}px`, '--i': index + 1 }}
        />
      ))}
    </div>
  );
}

function AiosVoiceInputPill({
  copy,
  value,
  setValue,
  placeholder,
  compact = false,
  voiceOnly = false,
  disabled = false,
  voiceInput,
  autoSubmitVoice = false,
  onVoiceClick,
  onCancelVoiceInput,
  onClearVoiceInput,
  onSubmit,
}) {
  const inputRef = useRef(null);
  const composingRef = useRef(false);
  const [voiceMode, setVoiceMode] = useState('dictation');
  const status = voiceInput?.status || 'idle';
  const active = voiceActiveStatus(status);
  const busy = status === 'requesting_permission' || status === 'transcribing';
  const failed = status === 'failed';
  const hasText = String(value || '').trim().length > 0;

  function writeVoiceResult(updater, mode, draftBeforeStart, context) {
    if (typeof setValue !== 'function') return;
    const editMode = mode === 'edit' || context?.mode === 'edit';
    if (!editMode && autoSubmitVoice && onSubmit) {
      const next = typeof updater === 'function'
        ? updater(String(draftBeforeStart ?? value ?? ''))
        : updater;
      const text = String(next || '').trim();
      if (text) onSubmit(text);
      setValue('');
      return;
    }
    setValue(prev => {
      const base = editMode ? String(draftBeforeStart ?? prev ?? '') : prev;
      const next = typeof updater === 'function' ? updater(base) : updater;
      return String(next || '').trim();
    });
  }

  function startVoice(mode = 'dictation') {
    if (disabled || !onVoiceClick) return;
    const normalizedMode = mode === 'edit' ? 'edit' : 'dictation';
    const draft = String(value || '');
    setVoiceMode(normalizedMode);
    inputRef.current?.blur();
    onVoiceClick(
      normalizedMode === 'edit' ? draft : '',
      (updater, draftBeforeStart, context) => writeVoiceResult(updater, normalizedMode, draftBeforeStart, context),
      { mode: normalizedMode },
    );
  }

  function finishRecording() {
    startVoice(voiceMode);
  }

  function cancelVoice() {
    if (active && onCancelVoiceInput) {
      onCancelVoiceInput();
      return;
    }
    if (onClearVoiceInput) onClearVoiceInput();
  }

  function submit() {
    const text = String(value || '').trim();
    if (!text || disabled || !onSubmit) return;
    onSubmit(text);
    setValue('');
    inputRef.current?.blur();
  }

  function clearDraft() {
    if (disabled) return;
    setValue('');
    inputRef.current?.focus();
  }

  function handleKeyDown(event) {
    const composing = event.isComposing || event.nativeEvent?.isComposing || composingRef.current || event.keyCode === 229;
    if (event.key === 'Enter' && !event.shiftKey && !composing) {
      event.preventDefault();
      if (hasText) submit();
    }
  }

  const rootClass = [
    'aios-voice-pill',
    compact ? 'compact' : '',
    voiceOnly && !active && !failed && !hasText ? 'voice-only' : '',
    active ? 'is-recording' : '',
    failed ? 'failed' : '',
    hasText ? 'has-text' : '',
  ].filter(Boolean).join(' ');

  if (active) {
    return (
      <div className={rootClass}>
        <button type="button" className="aios-voice-round aios-voice-cancel" onClick={cancelVoice} aria-label={copy.root?.voiceCancel || '取消语音输入'}>
          <i className="fas fa-times" />
        </button>
        <div className="aios-voice-main">
          <AiosVoiceWave muted={busy} />
          <div className="aios-voice-caption">{voiceStatusText(voiceInput, copy, voiceMode)}</div>
        </div>
        <button type="button" className="aios-voice-round aios-voice-confirm" disabled={busy} onClick={finishRecording} aria-label={copy.root?.voiceStop || '结束录音'}>
          <i className="fas fa-check" />
        </button>
      </div>
    );
  }

  if (failed) {
    return (
      <div className={rootClass}>
        <div className="aios-voice-error">{voiceStatusText(voiceInput, copy, voiceMode)}</div>
        <button type="button" className="aios-voice-retry" onClick={() => startVoice('dictation')}>{copy.root?.voiceRetry || '重试'}</button>
        <button type="button" className="aios-voice-round aios-voice-close" onClick={cancelVoice}><i className="fas fa-times" /></button>
      </div>
    );
  }

  if (voiceOnly && !hasText) {
    return (
      <div className={rootClass}>
        <button
          type="button"
          className="aios-voice-round aios-voice-mic"
          onClick={() => startVoice('dictation')}
          aria-label={copy.startVoice}
          title={copy.startVoice}
          disabled={disabled}
        >
          <i className="fas fa-microphone" />
        </button>
      </div>
    );
  }

  return (
    <div className={rootClass}>
      {hasText && (
        <button type="button" className="aios-voice-round aios-voice-clear aios-voice-close" onClick={clearDraft} aria-label="清空输入" title="清空输入" disabled={disabled}>
          <i className="fas fa-times" />
        </button>
      )}
      <textarea
        ref={inputRef}
        className="aios-voice-draft"
        value={value}
        rows={1}
        disabled={disabled}
        placeholder={placeholder || '说点什么，或点这里输入...'}
        onCompositionStart={() => { composingRef.current = true; }}
        onCompositionEnd={() => { composingRef.current = false; }}
        onChange={event => setValue(event.target.value)}
        onKeyDown={handleKeyDown}
      />
      {hasText && (
        <button type="button" className="aios-voice-round aios-voice-edit" onClick={() => startVoice('edit')} aria-label="二次语音编辑" title="二次语音编辑" disabled={disabled}>
          <i className="fas fa-wand-magic-sparkles" />
        </button>
      )}
      <button
        type="button"
        className={`aios-voice-round ${hasText ? 'aios-voice-send' : 'aios-voice-mic'}`}
        onClick={hasText ? submit : () => startVoice('dictation')}
        aria-label={hasText ? copy.send : copy.startVoice}
        title={hasText ? copy.send : copy.startVoice}
        disabled={disabled}
      >
        <i className={`fas ${hasText ? 'fa-paper-plane' : 'fa-microphone'}`} />
      </button>
    </div>
  );
}

function AiosVoiceDock({
  copy,
  voiceInput,
  voiceDraft,
  onVoiceClick,
  onCancelVoiceInput,
  onClearVoiceInput,
  onSendDraft,
  onChangeDraft,
}) {
  return (
    <AiosVoiceInputPill
      copy={copy}
      value={voiceDraft}
      setValue={onChangeDraft}
      placeholder="说点什么，或点这里输入..."
      voiceInput={voiceInput}
      autoSubmitVoice
      onVoiceClick={onVoiceClick}
      onCancelVoiceInput={onCancelVoiceInput}
      onClearVoiceInput={onClearVoiceInput}
      onSubmit={onSendDraft}
      compact
    />
  );
}

function AiosEmptyState({ copy, onQuickPrompt }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center text-center px-4 animate-fade-in pb-12">
      <div className="aios-empty-orb aios-empty-brand mb-6">
        <img src={pinvouAppIcon} alt="Pinvou" />
      </div>
      <h2 className="text-2xl font-semibold text-gray-800 dark:text-gray-100 mb-3 tracking-tight transition-colors">
        {copy.emptyTitle}
      </h2>
      <p className="text-gray-500 dark:text-gray-400 max-w-md mb-8 text-base transition-colors">
        {copy.emptyDesc}
      </p>
      <div className="flex flex-wrap justify-center gap-3 max-w-lg">
        <button
          type="button"
          onClick={() => onQuickPrompt(copy.quickActions.schedule.prompt)}
          className="aios-suggestion-chip px-5 py-2.5 rounded-full bg-white/50 dark:bg-gray-800/50 hover:bg-white/80 dark:hover:bg-gray-700/80 border border-white/60 dark:border-gray-600/60 text-gray-700 dark:text-gray-200 text-sm font-medium transition-all duration-200 shadow-sm hover:shadow hover:-translate-y-0.5 flex items-center gap-2"
        >
          <i className="fas fa-tasks text-blue-500 dark:text-blue-400 opacity-70" />
          {copy.quickActions.schedule.label}
        </button>
        <button
          type="button"
          onClick={() => onQuickPrompt(copy.quickActions.mail.prompt)}
          className="aios-suggestion-chip px-5 py-2.5 rounded-full bg-white/50 dark:bg-gray-800/50 hover:bg-white/80 dark:hover:bg-gray-700/80 border border-white/60 dark:border-gray-600/60 text-gray-700 dark:text-gray-200 text-sm font-medium transition-all duration-200 shadow-sm hover:shadow hover:-translate-y-0.5 flex items-center gap-2"
        >
          <i className="fas fa-envelope text-green-500 dark:text-green-400 opacity-70" />
          {copy.quickActions.mail.label}
        </button>
        <button
          type="button"
          onClick={() => onQuickPrompt(copy.quickActions.brainstorm.prompt)}
          className="aios-suggestion-chip px-5 py-2.5 rounded-full bg-white/50 dark:bg-gray-800/50 hover:bg-white/80 dark:hover:bg-gray-700/80 border border-white/60 dark:border-gray-600/60 text-gray-700 dark:text-gray-200 text-sm font-medium transition-all duration-200 shadow-sm hover:shadow hover:-translate-y-0.5 flex items-center gap-2"
        >
          <i className="fas fa-lightbulb text-orange-500 dark:text-orange-400 opacity-70" />
          {copy.quickActions.brainstorm.label}
        </button>
      </div>
    </div>
  );
}

function AiosTaskCard({ task, copy, onOpen, onDelete }) {
  const dragRef = useRef(null);
  const suppressClickRef = useRef(false);
  const [dragY, setDragY] = useState(0);
  const [deleting, setDeleting] = useState(false);
  const statusLabel = taskStatusLabel(task, copy);
  const statusTone = taskStatusTone(task);
  const resultPreview = truncateText(task.resultPreview || '', 118);
  const hasResult = Boolean(task.hasArtifact || resultPreview);

  function beginDrag(event) {
    if (deleting) return;
    dragRef.current = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      active: false,
      currentY: 0,
    };
    suppressClickRef.current = false;
    event.currentTarget.setPointerCapture?.(event.pointerId);
  }

  function moveDrag(event) {
    const drag = dragRef.current;
    if (!drag || drag.id !== event.pointerId || deleting) return;
    const dx = event.clientX - drag.startX;
    const dy = event.clientY - drag.startY;
    const isUpSwipe = dy < -8 && Math.abs(dy) > Math.abs(dx) * 1.15;
    if (!drag.active && !isUpSwipe) return;
    drag.active = true;
    suppressClickRef.current = true;
    event.preventDefault();
    drag.currentY = Math.max(-112, Math.min(0, dy));
    setDragY(drag.currentY);
  }

  async function endDrag(event) {
    const drag = dragRef.current;
    if (!drag || drag.id !== event.pointerId) return;
    dragRef.current = null;
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    if (drag.currentY <= -72 && onDelete) {
      setDeleting(true);
      try {
        await onDelete(task);
      } catch {
        setDeleting(false);
        setDragY(0);
      }
      return;
    }
    setDragY(0);
    window.setTimeout(() => {
      suppressClickRef.current = false;
    }, 0);
  }

  function cancelDrag() {
    dragRef.current = null;
    setDragY(0);
    window.setTimeout(() => {
      suppressClickRef.current = false;
    }, 0);
  }

  function openTask(event) {
    if (suppressClickRef.current || deleting) {
      event.preventDefault();
      suppressClickRef.current = false;
      return;
    }
    onOpen(task);
  }

  return (
    <div
      className={`aios-task-card glass-card rounded-3xl p-6 flex flex-col min-h-[240px] relative overflow-hidden transition-all duration-500 ease-out session-card-clickable ${deleting ? 'is-deleting' : ''} ${hasResult ? 'has-result' : ''} ${task.optimistic ? 'is-optimistic' : ''}`}
      onClick={openTask}
      onPointerDown={beginDrag}
      onPointerMove={moveDrag}
      onPointerUp={endDrag}
      onPointerCancel={cancelDrag}
      style={{
        transform: deleting ? 'translateY(-140px) scale(0.96)' : (dragY ? `translateY(${dragY}px)` : undefined),
        opacity: deleting ? 0 : undefined,
      }}
      role="button"
      tabIndex={0}
      aria-label={copy.openTask(task.title || copy.untitledTask)}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          onOpen(task);
        }
      }}
    >
      <div className="aios-task-card-delete-hint" aria-hidden="true">
        <i className="fas fa-trash" />
        <span>松手删除会话</span>
      </div>
      <h3 className="text-xl font-semibold text-gray-800 dark:text-gray-100 mb-3 session-title transition-colors">
        {task.title || copy.untitledTask}
      </h3>
      <p className="text-gray-600 dark:text-gray-300 text-sm line-clamp-3 mb-4 flex-1 session-preview transition-colors">
        {taskPreview(task, copy)}
      </p>
      {hasResult && (
        <div className="aios-task-result-preview">
          <div className="aios-task-result-kicker">
            <i className={`fas ${task.hasArtifact ? 'fa-cube' : 'fa-check'}`} />
            {task.hasArtifact ? (copy.artifactReady || '已生成产物') : (copy.resultReady || '已生成结果')}
          </div>
          {resultPreview && <p>{resultPreview}</p>}
        </div>
      )}
      <div className="text-xs font-medium text-gray-500 dark:text-gray-400 flex items-center session-time transition-colors">
        <i className="far fa-clock mr-1" />
        {task.date || copy.recently}
      </div>
      {statusLabel && (
        <div className={`aios-task-status aios-task-status-${statusTone}`}>
          <i className={`fas ${['processing', 'auth_checking', 'authorizing', 'searching', 'validating', 'submitting_order', 'browser_prepare', 'browser_searching'].includes(task.status) || (task.optimistic && !['needs_auth', 'needs_details', 'needs_choice', 'order_confirm', 'order_created', 'wendao_ready', 'paused', 'failed', 'auth_failed', 'submit_failed', 'user_action_required', 'browser_order_review', 'submit_confirmation_required', 'payment_required', 'browser_blocked', 'browser_cancelled', 'handoff_done'].includes(task.status)) ? 'fa-circle-notch fa-spin' : task.needsUserInput || ['needs_auth', 'needs_details', 'needs_choice', 'order_confirm', 'user_action_required', 'submit_confirmation_required', 'payment_required'].includes(task.status) ? 'fa-hand-pointer' : ['failed', 'auth_failed', 'submit_failed', 'browser_blocked'].includes(task.status) ? 'fa-triangle-exclamation' : ['browser_cancelled'].includes(task.status) ? 'fa-ban' : 'fa-check'}`} />
          <span>{statusLabel}</span>
        </div>
      )}
    </div>
  );
}

function AiosDialogue({ items, busy, thinking, copy, onSubmitUserInput, onCancelUserInput }) {
  const hasItems = items && items.length > 0;
  if (!hasItems && !busy) {
    return <div className="text-gray-400 text-sm text-center">{copy.noDialogue}</div>;
  }

  return (
    <div className="aios-timeline">
      {items.map((item, index) => {
        if (shouldHideToolBubble(item)) return null;
        const userInputItem = normalizeUserInputItem(item);
        if (userInputItem) {
          return (
            <AiosUserInputCard
              key={item.id || userInputItem.id || index}
              item={userInputItem}
              copy={copy}
              onSubmit={onSubmitUserInput}
              onCancel={onCancelUserInput}
            />
          );
        }
        const text = bubbleText(item);
        if (!text) return null;
        return <AiosTimelineItem key={item.id || index} meta={timelineMetaForItem(item, copy)} />;
      })}
      {busy && (
        <AiosTimelineItem
          active
          meta={{
            icon: 'fa-circle-notch fa-spin',
            title: copy.processing,
            desc: truncateText(readableText(thinking), 60),
            tone: 'blue',
          }}
        />
      )}
    </div>
  );
}

function cleanMarkdownText(value) {
  return String(value || '')
    .replace(/\*\*/g, '')
    .replace(/`/g, '')
    .replace(/^#{1,6}\s*/gm, '')
    .trim();
}

function isProcessText(text) {
  const value = String(text || '').trim();
  if (!value) return true;
  const lower = value.toLowerCase();
  return [
    /^搜索结果显示[:：]/,
    /^思路[:：]/,
    /^mcp registry-first/i,
    /^没有.+工具/,
    /^所以我手上可用/,
    /registry_sync/,
    /start_registry_mcp_server/,
    /\bthinking\b/i,
    /\bdebug\b/i,
    /\btool\b/i,
    /正在获取信息或生成产物/,
    /我可以尝试用 bash/i,
    /bash\s*\+\s*curl/i,
  ].some(pattern => pattern.test(value) || pattern.test(lower));
}

function hasReportSignals(text) {
  const value = String(text || '');
  return /^(#{1,3}\s+.+)$/m.test(value)
    || /\|[^|\n]+\|[^|\n]+\|/.test(value)
    || /(结论|摘要|数据来源|趋势|均价|价格|同比|环比|天气|气温|湿度|风速|降水|报告|分析)/.test(value);
}

function extractTableValue(text, label) {
  const tableMatch = text.match(new RegExp(`\\|\\s*${label}\\s*\\|\\s*([^|\\n]+)`, 'i'));
  if (tableMatch) return cleanMarkdownText(tableMatch[1]);
  const lineMatch = text.match(new RegExp(`${label}\\s*[|:：]\\s*([^|\\n]+)`, 'i'));
  if (lineMatch) return cleanMarkdownText(lineMatch[1]);
  return '';
}

function latestAssistantText(items, { busy = false } = {}) {
  const rows = Array.isArray(items) ? items : [];
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const item = rows[index];
    if (!item || shouldHideToolBubble(item) || normalizeUserInputItem(item)) continue;
    const role = String(item.role || item.speaker || item.type || '').toLowerCase();
    if (role.includes('user') || role.includes('tool') || role.includes('system')) continue;
    const type = String(item.type || '').toLowerCase();
    if (type.includes('reasoning') || type.includes('thinking') || type.includes('debug') || type.includes('tool')) continue;
    const text = bubbleText(item);
    if (!text.trim() || isProcessText(text)) continue;
    if (busy && !hasReportSignals(text)) continue;
    return text.trim();
  }
  return '';
}

function weatherIconFor(text) {
  if (/雷|暴雨/.test(text)) return 'fa-cloud-bolt';
  if (/雨/.test(text)) return 'fa-cloud-rain';
  if (/雪/.test(text)) return 'fa-snowflake';
  if (/晴/.test(text) && /云/.test(text)) return 'fa-cloud-sun';
  if (/晴/.test(text)) return 'fa-sun';
  if (/云|阴/.test(text)) return 'fa-cloud';
  return 'fa-cloud-sun';
}

function parseWeatherArtifact(text) {
  if (!/天气/.test(text) || !/(气温|温度|湿度|风速|降水|日出|日落)/.test(text)) return null;
  const heading = text.match(/^#{1,3}\s*(.+)$/m);
  const title = cleanMarkdownText(heading?.[1] || text.split('\n').find(line => line.trim()) || '天气');
  const temperature = extractTableValue(text, '气温') || extractTableValue(text, '温度');
  const condition = extractTableValue(text, '天气') || '';
  const humidity = extractTableValue(text, '湿度');
  const wind = extractTableValue(text, '风速');
  const rain = extractTableValue(text, '降水');
  const sun = extractTableValue(text, '日出 / 日落') || extractTableValue(text, '日出') || extractTableValue(text, '日落');
  const tips = text
    .split('\n')
    .map(line => cleanMarkdownText(line.replace(/^[-*]\s*/, '')))
    .filter(line => line && !line.includes('|---') && !line.startsWith('|') && !line.startsWith('##'))
    .filter(line => /(记得|建议|需要|带伞|舒适|最高|转晴|出行|注意)/.test(line))
    .slice(0, 4);

  return {
    title,
    temperature: temperature || '暂无温度',
    condition: condition || '天气信息',
    humidity,
    wind,
    rain,
    sun,
    tips,
    icon: weatherIconFor(`${title} ${condition} ${text}`),
  };
}

function parseGenericArtifact(text, copy) {
  const lines = text.split('\n').map(line => line.trim()).filter(Boolean);
  const headingIndex = lines.findIndex(line => /^#{1,3}\s+/.test(line));
  const title = headingIndex >= 0
    ? cleanMarkdownText(lines[headingIndex])
    : cleanMarkdownText(lines[0] || copy.artifact);
  const body = headingIndex >= 0
    ? lines.filter((_, index) => index !== headingIndex).join('\n')
    : lines.slice(1).join('\n');
  const kind = /\|[^|\n]+\|[^|\n]+\|/.test(text) || /(数据来源|趋势|均价|同比|环比|指标|报告|分析)/.test(text)
    ? '报告'
    : '回答';
  return { title: title || copy.artifact, body: body || text, kind };
}

function AiosWeatherArtifact({ weather }) {
  const metrics = [
    { label: '天气', value: weather.condition, icon: 'fa-cloud-sun' },
    { label: '湿度', value: weather.humidity, icon: 'fa-droplet' },
    { label: '风速', value: weather.wind, icon: 'fa-wind' },
    { label: '降水', value: weather.rain, icon: 'fa-cloud-rain' },
    { label: '日出/日落', value: weather.sun, icon: 'fa-sun' },
  ].filter(item => item.value);

  return (
    <div className="aios-generated-artifact aios-weather-card">
      <div className="aios-weather-hero">
        <div>
          <div className="aios-artifact-kicker">天气卡片</div>
          <h3>{weather.title}</h3>
          <div className="aios-weather-temp">{weather.temperature}</div>
        </div>
        <div className="aios-weather-icon">
          <i className={`fas ${weather.icon}`} />
        </div>
      </div>
      <div className="aios-weather-metrics">
        {metrics.map(metric => (
          <div key={metric.label} className="aios-weather-metric">
            <i className={`fas ${metric.icon}`} />
            <span>{metric.label}</span>
            <strong>{metric.value}</strong>
          </div>
        ))}
      </div>
      {weather.tips.length > 0 && (
        <div className="aios-weather-tips">
          {weather.tips.map((tip, index) => (
            <div key={`${tip}-${index}`} className="aios-weather-tip">
              <i className="fas fa-circle-info" />
              <span>{tip}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function AiosAnswerArtifact({ artifact }) {
  const html = useMemo(() => renderMarkdown(artifact.body || ''), [artifact.body]);
  return (
    <div className="aios-generated-artifact aios-answer-card">
      <div className="aios-artifact-kicker">{artifact.kind || '回答'}</div>
      <h3>{artifact.title}</h3>
      <div className="aios-answer-body" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}

function AiosGeneratedArtifact({ text, copy }) {
  const weather = parseWeatherArtifact(text);
  if (weather) return <AiosWeatherArtifact weather={weather} />;
  return <AiosAnswerArtifact artifact={parseGenericArtifact(text, copy)} />;
}

function artifactLabel(artifact, copy, index = 0) {
  return artifact?.basename || artifact?.name || artifact?.title || (artifact?.path ? String(artifact.path).split(/[\\/]/).pop() : '') || copy.artifactItem(index + 1);
}

function artifactExt(artifact) {
  const name = artifact?.path || artifact?.basename || artifact?.name || '';
  return String(name).split('.').pop().toLowerCase();
}

function AiosArtifactSkeleton({ copy }) {
  return (
    <div className="aios-artifact-skeleton">
      <div className="aios-artifact-skeleton-icon">
        <i className="fas fa-circle-notch fa-spin" />
      </div>
      <div>
        <strong>正在生成产物</strong>
        <span>{copy.processing}</span>
      </div>
      <div className="aios-skeleton-line wide" />
      <div className="aios-skeleton-line" />
      <div className="aios-skeleton-grid">
        <span />
        <span />
        <span />
        <span />
      </div>
    </div>
  );
}

function AiosArtifactPreview({ artifacts, artifactApi, copy, fallbackText = '' }) {
  const artifactsKey = useMemo(() => artifacts.map(item => item.path || item.basename || item.name || '').join('|'), [artifacts]);
  const [selectedPath, setSelectedPath] = useState('');
  const [preview, setPreview] = useState({ loading: true });
  const selected = useMemo(() => {
    if (!artifacts.length) return null;
    return artifacts.find(item => String(item.path || '') === String(selectedPath)) || artifacts[artifacts.length - 1];
  }, [artifacts, selectedPath]);

  useEffect(() => {
    const latest = artifacts[artifacts.length - 1];
    setSelectedPath(latest?.path || '');
  }, [artifactsKey, artifacts]);

  useEffect(() => {
    if (!selected?.path) {
      setPreview({ empty: true });
      return undefined;
    }
    if (!artifactApi) {
      setPreview({ unsupported: true });
      return undefined;
    }
    let cancelled = false;
    setPreview({ loading: true });
    (async () => {
      try {
        const ext = artifactExt(selected);
        const info = artifactApi.artifactInfo ? await artifactApi.artifactInfo(selected.path) : null;
        if (cancelled) return;
        if (info && info.exists === false) {
          setPreview({ missing: true, info });
          return;
        }
        if (['html', 'htm'].includes(ext) && artifactApi.readArtifactText) {
          const html = await artifactApi.readArtifactText(selected.path, selected.sessionId || selected.session_id);
          if (!cancelled) setPreview({ kind: 'html', html, info });
          return;
        }
        if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'].includes(ext) && artifactApi.readArtifactImageB64) {
          const url = await artifactApi.readArtifactImageB64(selected.path, selected.sessionId || selected.session_id);
          if (!cancelled) setPreview({ kind: 'image', url, info });
          return;
        }
        if (['md', 'markdown', 'txt', 'csv', 'json', 'log'].includes(ext) && artifactApi.readArtifactText) {
          const text = await artifactApi.readArtifactText(selected.path, selected.sessionId || selected.session_id);
          if (!cancelled) setPreview({ kind: 'text', text, info });
          return;
        }
        if (artifactApi.renderArtifactVisual) {
          const visual = await artifactApi.renderArtifactVisual(selected.path, selected.sessionId || selected.session_id);
          if (!cancelled) setPreview({ kind: 'visual', visual, info });
          return;
        }
        setPreview({ unsupported: true, info });
      } catch (error) {
        if (!cancelled) setPreview({ error: String(error) });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [selected?.path, selected?.sessionId, selected?.session_id, artifactApi]);

  function renderPreviewContent() {
    if (preview.loading) {
      if (fallbackText) {
        return (
          <div className="aios-artifact-pending-replace">
            <AiosGeneratedArtifact text={fallbackText} copy={copy} />
            <div className="aios-artifact-loading-badge">
              <i className="fas fa-circle-notch fa-spin" />
              <span>正在加载产物预览</span>
            </div>
          </div>
        );
      }
      return (
        <div className="aios-artifact-loading">
          <i className="fas fa-circle-notch fa-spin" />
          <span>{copy.processing}</span>
        </div>
      );
    }
    if (preview.kind === 'html') {
      return (
        <iframe
          className="aios-artifact-frame"
          sandbox="allow-same-origin allow-scripts"
          title={artifactLabel(selected, copy)}
          srcDoc={preview.html || ''}
        />
      );
    }
    if (preview.kind === 'image') {
      return <img className="aios-artifact-image" src={preview.url} alt={artifactLabel(selected, copy)} />;
    }
    if (preview.kind === 'text') {
      const ext = artifactExt(selected);
      if (['md', 'markdown'].includes(ext)) {
        return <div className="aios-artifact-text markdown" dangerouslySetInnerHTML={{ __html: renderMarkdown(preview.text || '') }} />;
      }
      return <pre className="aios-artifact-text">{preview.text}</pre>;
    }
    if (preview.kind === 'visual' && preview.visual?.mode === 'html') {
      return (
        <iframe
          className="aios-artifact-frame"
          sandbox="allow-same-origin allow-scripts"
          title={artifactLabel(selected, copy)}
          srcDoc={preview.visual.html || ''}
        />
      );
    }
    if (preview.kind === 'visual' && preview.visual?.mode === 'images') {
      return (
        <div className="aios-artifact-image-stack">
          {(preview.visual.images || []).map((src, index) => (
            <img key={`${src}-${index}`} className="aios-artifact-image" src={src} alt={`${artifactLabel(selected, copy)} ${index + 1}`} />
          ))}
        </div>
      );
    }
    if (fallbackText) {
      return (
        <div className="aios-artifact-pending-replace">
          <AiosGeneratedArtifact text={fallbackText} copy={copy} />
          <div className="aios-artifact-loading-badge error">
            <i className="fas fa-triangle-exclamation" />
            <span>产物预览失败，已保留可读结果</span>
          </div>
        </div>
      );
    }
    return (
      <div className="aios-artifact-fallback">
        <strong>{artifactLabel(selected, copy)}</strong>
        {selected?.path && <span>{selected.path}</span>}
        {preview.error && <em>{preview.error}</em>}
      </div>
    );
  }

  return (
    <div className="aios-real-artifact">
      {artifacts.length > 1 && (
        <div className="aios-artifact-tabs">
          {artifacts.map((artifact, index) => (
            <button
              key={`${artifact.path || artifact.name || index}-${index}`}
              type="button"
              className={selected?.path === artifact.path ? 'active' : ''}
              onClick={() => setSelectedPath(artifact.path || '')}
              title={artifactLabel(artifact, copy, index)}
            >
              {artifactLabel(artifact, copy, index)}
            </button>
          ))}
        </div>
      )}
      <div className="aios-real-artifact-body">
        {renderPreviewContent()}
      </div>
    </div>
  );
}

function artifactSearchHaystack(artifact, copy, index) {
  return [
    artifactLabel(artifact, copy, index),
    artifact?.ext,
    artifact?.category,
    artifact?.source,
    artifact?.path,
  ].map(value => String(value || '').toLowerCase()).join(' ');
}

function AiosArtifactLibraryModal({ copy, items, loading, error, artifactApi, onClose }) {
  const [query, setQuery] = useState('');
  const [selectedPath, setSelectedPath] = useState('');
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    const rows = Array.isArray(items) ? items : [];
    if (!needle) return rows;
    return rows.filter((artifact, index) => artifactSearchHaystack(artifact, copy, index).includes(needle));
  }, [items, query, copy]);
  const selected = useMemo(() => {
    if (!filtered.length) return null;
    return filtered.find(item => String(item.path || '') === String(selectedPath)) || filtered[0];
  }, [filtered, selectedPath]);

  useEffect(() => {
    setSelectedPath(filtered[0]?.path || '');
  }, [filtered]);

  useEffect(() => {
    function closeOnEscape(event) {
      if (event.key === 'Escape') onClose();
    }
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  return (
    <div
      className="modal-overlay active"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="aios-task-modal aios-artifact-library-modal glass-panel modal-content rounded-[2rem] p-6 relative flex flex-col" onMouseDown={(event) => event.stopPropagation()}>
        <div className="flex justify-between items-center mb-4 px-2 gap-4">
          <div className="min-w-0">
            <h2 className="text-2xl font-semibold text-gray-800 dark:text-gray-100 transition-colors truncate">产物库</h2>
            <div className="text-xs font-medium text-gray-500 dark:text-gray-400 mt-1 transition-colors">
              已索引 {items.length} 个产物
            </div>
          </div>
          <button
            type="button"
            className="w-8 h-8 rounded-full bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 flex items-center justify-center text-gray-600 dark:text-gray-300 transition-colors"
            onClick={onClose}
            aria-label={copy.closeTask}
            title={copy.closeTask}
          >
            <i className="fas fa-times" />
          </button>
        </div>

        <div className="aios-artifact-library-search">
          <i className="fas fa-magnifying-glass" />
          <input
            type="search"
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder="搜索产物、类型、会话..."
            autoFocus
          />
        </div>

        <div className="aios-artifact-library-body">
          <aside className="aios-artifact-library-list custom-scrollbar">
            {loading && (
              <div className="aios-artifact-library-state">
                <i className="fas fa-circle-notch fa-spin" />
                <span>正在加载产物...</span>
              </div>
            )}
            {!loading && error && (
              <div className="aios-artifact-library-state error">
                <i className="fas fa-triangle-exclamation" />
                <span>{error}</span>
              </div>
            )}
            {!loading && !error && filtered.length === 0 && (
              <div className="aios-artifact-library-state">
                <i className="far fa-folder-open" />
                <span>{query.trim() ? '没有匹配的产物' : '还没有可检索的产物'}</span>
              </div>
            )}
            {!loading && !error && filtered.map((artifact, index) => (
              <button
                key={`${artifact.path || artifact.name || index}-${index}`}
                type="button"
                className={selected?.path === artifact.path ? 'active' : ''}
                onClick={() => setSelectedPath(artifact.path || '')}
                title={artifact.path || artifactLabel(artifact, copy, index)}
              >
                <span className="aios-artifact-library-type">{String(artifact.ext || artifact.category || 'file').toUpperCase()}</span>
                <span className="aios-artifact-library-name">{artifactLabel(artifact, copy, index)}</span>
                <span className="aios-artifact-library-source">{artifact.source || artifact.path || 'Pinvou 产物'}</span>
              </button>
            ))}
          </aside>

          <section className="aios-artifact-library-preview artifact-panel">
            {selected ? (
              <AiosArtifactPreview artifacts={[selected]} artifactApi={artifactApi} copy={copy} />
            ) : (
              <div className="aios-artifact-empty">
                <p>选择一个产物预览</p>
              </div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}

function AiosArtifact({ artifacts, chatItems, busy, copy, artifactApi }) {
  const generatedText = latestAssistantText(chatItems, { busy });
  if (!artifacts || artifacts.length === 0) {
    if (generatedText) {
      return <AiosGeneratedArtifact text={generatedText} copy={copy} />;
    }
    if (busy) {
      return <AiosArtifactSkeleton copy={copy} />;
    }
    return (
      <div className="aios-artifact-empty">
        <p>{copy.noArtifact}</p>
      </div>
    );
  }

  return <AiosArtifactPreview artifacts={artifacts} artifactApi={artifactApi} copy={copy} fallbackText={generatedText} />;
}

function hasActiveUserInput(items) {
  return (Array.isArray(items) ? items : []).some(item => {
    const userInputItem = normalizeUserInputItem(item);
    return userInputItem && !userInputItem.resolved && userInputItem.cardState !== 'submitted' && userInputItem.cardState !== 'cancelled';
  });
}

function activeUserInputItem(items) {
  for (const item of Array.isArray(items) ? items : []) {
    const userInputItem = normalizeUserInputItem(item);
    if (userInputItem && !userInputItem.resolved && userInputItem.cardState !== 'submitted' && userInputItem.cardState !== 'cancelled') {
      return userInputItem;
    }
  }
  return null;
}

function sessionStatusMeta({ items, busy, artifacts, copy }) {
  if (hasActiveUserInput(items)) {
    return { icon: 'fa-hand-pointer', label: copy.root?.choiceTitle || '需要你的选择', tone: 'orange' };
  }
  if (busy) {
    return { icon: 'fa-circle-notch fa-spin', label: '正在处理', tone: 'blue' };
  }
  if (Array.isArray(artifacts) && artifacts.length > 0) {
    return { icon: 'fa-cube', label: '已生成产物', tone: 'green' };
  }
  if (latestAssistantText(items, { busy })) {
    return { icon: 'fa-check', label: '已生成结果', tone: 'green' };
  }
  return { icon: 'fa-wand-magic-sparkles', label: '准备就绪', tone: 'blue' };
}

function AiosSessionModal({
  copy,
  task,
  chatItems,
  busy,
  thinking,
  artifacts,
  artifactApi,
  voiceInput,
  onVoiceClick,
  onClose,
  onSend,
  onSubmitUserInput,
  onCancelUserInput,
  onCancelVoiceInput,
  onClearVoiceInput,
}) {
  const processRef = useRef(null);
  const [processOpen, setProcessOpen] = useState(false);
  const [voiceDraft, setVoiceDraft] = useState('');
  const finalAnswerText = useMemo(() => latestAssistantText(chatItems || [], { busy }), [chatItems, busy]);
  const hasRealArtifact = Array.isArray(artifacts) && artifacts.length > 0;
  const hasVisibleResult = hasRealArtifact || Boolean(finalAnswerText);
  const needsUserInput = hasActiveUserInput(chatItems);
  const statusMeta = useMemo(
    () => sessionStatusMeta({ items: chatItems || [], busy, artifacts, copy }),
    [chatItems, busy, artifacts, copy]
  );

  useEffect(() => {
    processRef.current?.scrollTo({ top: processRef.current.scrollHeight });
  }, [chatItems, busy, processOpen]);

  useEffect(() => {
    if (needsUserInput) {
      setProcessOpen(true);
      return;
    }
    if (hasVisibleResult) {
      setProcessOpen(false);
      return;
    }
    if (busy) setProcessOpen(true);
  }, [busy, hasVisibleResult, needsUserInput]);

  useEffect(() => {
    function closeOnEscape(event) {
      if (event.key === 'Escape') onClose();
    }
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  if (!task) return null;

  function sendVoiceDraft(nextText) {
    const text = String(nextText || voiceDraft || '').trim();
    if (!text) return;
    onSend(text);
    setVoiceDraft('');
  }

  return (
    <div
      className="modal-overlay active"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="aios-task-modal glass-panel modal-content rounded-[2rem] p-6 relative flex flex-col" onMouseDown={(event) => event.stopPropagation()}>
        <div className="flex justify-between items-center mb-4 px-2 gap-4">
          <div className="flex items-center space-x-4 min-w-0">
            <div>
              <div className="p-3 rounded-2xl bg-blue-100 dark:bg-blue-900/40 text-blue-500 dark:text-blue-400 icon-container transition-colors">
                <i className={`fas ${iconForTask(task, 0)} text-xl`} />
              </div>
            </div>
            <div className="min-w-0">
              <h2 className="text-2xl font-semibold text-gray-800 dark:text-gray-100 transition-colors truncate">
                {task.title || copy.untitledTask}
              </h2>
              <div className="text-xs font-medium text-gray-500 dark:text-gray-400 mt-1 transition-colors">
                <i className="far fa-clock mr-1" />
                {task.date || copy.recently}
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2 shrink-0">
            <button
              type="button"
              className={`aios-status-pill aios-status-${statusMeta.tone}`}
              onClick={() => setProcessOpen(true)}
            >
              <i className={`fas ${statusMeta.icon}`} />
              <span>{statusMeta.label}</span>
            </button>
            <button
              type="button"
              className="w-8 h-8 rounded-full bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 flex items-center justify-center text-gray-600 dark:text-gray-300 transition-colors"
              onClick={onClose}
              aria-label={copy.closeTask}
              title={copy.closeTask}
            >
              <i className="fas fa-times" />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-hidden relative">
          <div className="aios-product-stage w-full h-full artifact-panel p-6 overflow-y-auto custom-scrollbar relative">
            <div className="absolute top-4 right-4 flex space-x-2">
              <button type="button" className="w-8 h-8 rounded-full bg-white/50 dark:bg-gray-700/50 text-gray-500 dark:text-gray-400 hover:text-blue-500 dark:hover:text-blue-400 hover:bg-white dark:hover:bg-gray-600 transition-colors flex items-center justify-center shadow-sm" aria-label={copy.copyArtifact}>
                <i className="far fa-copy text-sm" />
              </button>
              <button type="button" className="w-8 h-8 rounded-full bg-white/50 dark:bg-gray-700/50 text-gray-500 dark:text-gray-400 hover:text-blue-500 dark:hover:text-blue-400 hover:bg-white dark:hover:bg-gray-600 transition-colors flex items-center justify-center shadow-sm" aria-label={copy.shareArtifact}>
                <i className="fas fa-share text-sm" />
              </button>
            </div>
            <div className="text-xs text-gray-400 dark:text-gray-500 font-medium mb-4 uppercase tracking-wider">
              {copy.artifact}
            </div>
            <div className="text-gray-800 dark:text-gray-200 leading-relaxed transition-colors">
              <AiosArtifact artifacts={artifacts} chatItems={chatItems || []} busy={busy} copy={copy} artifactApi={artifactApi} />
            </div>
          </div>

          {processOpen && (
            <div className="aios-process-layer">
              <button type="button" className="aios-process-scrim" aria-label="close process" onClick={() => setProcessOpen(false)} />
              <aside className="aios-process-drawer">
                <div className="aios-process-header">
                  <div>
                    <div className="aios-process-kicker">{copy.dialogue}</div>
                    <div className="aios-process-title">任务链路</div>
                  </div>
                  <button type="button" className="aios-process-close" onClick={() => setProcessOpen(false)} aria-label={copy.closeTask}>
                    <i className="fas fa-times" />
                  </button>
                </div>
                <div ref={processRef} className="aios-process-scroll custom-scrollbar">
                  <AiosDialogue
                    items={chatItems || []}
                    busy={busy}
                    thinking={thinking}
                    copy={copy}
                    onSubmitUserInput={onSubmitUserInput}
                    onCancelUserInput={onCancelUserInput}
                  />
                </div>
              </aside>
            </div>
          )}
        </div>

        <div className="aios-task-voice-dock" role="region" aria-label="继续对话">
          <AiosVoiceDock
            copy={copy}
            voiceInput={voiceInput}
            voiceDraft={voiceDraft}
            onVoiceClick={onVoiceClick}
            onCancelVoiceInput={onCancelVoiceInput}
            onClearVoiceInput={onClearVoiceInput}
            onSendDraft={sendVoiceDraft}
            onChangeDraft={setVoiceDraft}
          />
        </div>
      </div>
    </div>
  );
}

function AiosUserInputOverlay({ item, copy, onSubmit, onCancel }) {
  if (!item) return null;
  return (
    <div className="aios-user-input-overlay">
      <div className="aios-user-input-backdrop" />
      <div className="aios-user-input-dialog glass-panel">
        <div className="aios-user-input-dialog-header">
          <div>
            <div className="aios-process-kicker">{copy.root?.choiceTitle || '需要你的选择'}</div>
            <h2>{copy.root?.choiceTitle || '需要你的选择'}</h2>
          </div>
          <button type="button" className="aios-process-close" onClick={() => onCancel?.(item)} aria-label={copy.closeTask}>
            <i className="fas fa-times" />
          </button>
        </div>
        <AiosUserInputCard
          item={item}
          copy={copy}
          onSubmit={onSubmit}
          onCancel={onCancel}
        />
      </div>
    </div>
  );
}

function AiosCtripConnectorOverlay({ flow, copy, onAction }) {
  const [details, setDetails] = useState(flow?.details || {});
  const [authToken, setAuthToken] = useState('');

  useEffect(() => {
    setDetails(flow?.details || {});
  }, [flow?.taskId, flow?.step]);

  if (!flow?.visible) return null;
  const ctrip = copy.ctrip || {};
  const submitting = Boolean(flow.submitting);
  const error = flow.error || '';
  const disabled = submitting;

  function updateField(name, value) {
    setDetails(prev => ({ ...prev, [name]: value }));
  }

  function submitDetails() {
    onAction?.('submit_details', { details });
  }

  const handoffItems = [
    [ctrip.origin || '出发地', flow.details?.origin],
    [ctrip.destination || '目的地', flow.details?.destination],
    [ctrip.date || '日期', flow.details?.date],
    [ctrip.cabin || '舱位', flow.details?.cabin],
    [ctrip.adults || '成人', flow.details?.adults],
    [ctrip.children || '儿童', flow.details?.children],
    [ctrip.budget || '预算', flow.details?.budget],
    [ctrip.timePreference || '时间偏好', flow.details?.timePreference],
    [ctrip.ctripLink || '携程链接', flow.ctripUrl],
  ].filter(([, value]) => String(value || '').trim());

  function close() {
    onAction?.('close');
  }

  return (
    <div className="aios-user-input-overlay aios-ctrip-overlay">
      <div className="aios-user-input-backdrop" />
      <div className="aios-user-input-dialog aios-ctrip-dialog glass-panel">
        <div className="aios-user-input-dialog-header">
          <div>
            <div className="aios-process-kicker">{ctrip.kicker || '携程问道连接器'}</div>
            <h2>{ctrip.title || '携程问道查询'}</h2>
          </div>
          <button type="button" className="aios-process-close" onClick={close} aria-label={copy.closeTask}>
            <i className="fas fa-times" />
          </button>
        </div>

        {flow.step === 'checking_auth' && (
          <div className="aios-ctrip-state">
            <i className="fas fa-circle-notch fa-spin" />
            <h3>{ctrip.checkingAuth || '正在检查携程问道 Token'}</h3>
            <p>{ctrip.checkingAuthDesc || '我会先确认 WorkBuddy 的携程问道连接器能否查询。'}</p>
          </div>
        )}

        {(flow.step === 'auth' || flow.step === 'capability_blocked') && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-shield-halved" />
              <div>
                <h3>{flow.step === 'capability_blocked' ? (ctrip.capabilityBlocked || '连接器能力不足') : (ctrip.needAuth || '需要配置携程问道 Token')}</h3>
                <p>{flow.step === 'capability_blocked'
                  ? (ctrip.capabilityBlockedDesc || '当前连接器不可用，只能手动打开携程。')
                  : (ctrip.needAuthDesc || '我不会要求你的携程账号密码。这里使用携程问道开放平台 API Token，只用于查询旅行信息。')}</p>
              </div>
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.capabilityMatrix || '能力矩阵'}</span>
              <strong>{flow.capabilities?.provider || '携程问道（WorkBuddy）'}</strong>
            </div>
            {flow.step === 'auth' && (
              <div className="aios-ctrip-form aios-ctrip-form-single">
                <label className="aios-ctrip-field">
                  <span>{ctrip.apiToken || 'API Token'}</span>
                  <input
                    value={authToken}
                    type="password"
                    placeholder={ctrip.apiTokenPlaceholder || '粘贴携程问道开放平台 Token'}
                    disabled={disabled}
                    onChange={event => setAuthToken(event.target.value)}
                  />
                </label>
                {flow.auth?.docUrl && (
                  <button type="button" className="aios-ctrip-link-button" disabled={disabled} onClick={() => onAction?.('open_ctrip', { url: flow.auth.docUrl })}>
                    {ctrip.applyToken || '去携程问道开放平台申请 Token'}
                  </button>
                )}
              </div>
            )}
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('decline_auth')}>
                {ctrip.skipAuth || '先不配置'}
              </button>
              {flow.step !== 'capability_blocked' && (
                <button type="button" className="aios-input-submit" disabled={disabled || !authToken.trim()} onClick={() => onAction?.('connect', { token: authToken })}>
                  {submitting ? copy.processing : (ctrip.connect || '保存 Token')}
                </button>
              )}
            </div>
          </div>
        )}

        {flow.step === 'details' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-list-check" />
              <div>
                <h3>{ctrip.detailsTitle || '补全行程信息'}</h3>
                <p>{ctrip.detailsDesc || '为了避免连续追问，请一次性确认出发地、目的地、日期和偏好。'}</p>
              </div>
            </div>
            <div className="aios-ctrip-form">
              {[
                ['origin', ctrip.origin || '出发地', '北京'],
                ['destination', ctrip.destination || '目的地', '上海'],
                ['date', ctrip.date || '日期', '明天'],
                ['cabin', ctrip.cabin || '舱位', '经济舱'],
                ['adults', ctrip.adults || '成人', '1'],
                ['children', ctrip.children || '儿童', '0'],
                ['budget', ctrip.budget || '预算', '不限'],
                ['timePreference', ctrip.timePreference || '时间偏好', '不限'],
              ].map(([name, label, placeholder]) => (
                <label key={name} className="aios-ctrip-field">
                  <span>{label}</span>
                  <input
                    value={details[name] || ''}
                    placeholder={placeholder}
                    disabled={disabled}
                    onChange={event => updateField(name, event.target.value)}
                  />
                </label>
              ))}
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('disconnect')}>
                {ctrip.disconnect || '清除 Token'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled || !details.destination} onClick={submitDetails}>
                {submitting ? copy.processing : (ctrip.search || '调用携程问道')}
              </button>
            </div>
          </div>
        )}

        {flow.step === 'searching' && (
          <div className="aios-ctrip-state">
            <i className="fas fa-circle-notch fa-spin" />
            <h3>{ctrip.searching || '正在调用携程问道'}</h3>
            <p>{ctrip.searchingDesc || '我会只展示携程问道返回的 result，不暴露内部日志。'}</p>
          </div>
        )}

        {flow.step === 'result' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-route" />
              <div>
                <h3>{ctrip.resultTitle || '携程问道结果'}</h3>
                <p>{ctrip.resultDesc || '下面是查询结果。下单、登录和支付需要前往携程官方页面完成。'}</p>
              </div>
            </div>
            <div
              className="aios-ctrip-result msg-md"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(flow.result?.result || '') }}
            />
            <div className="aios-ctrip-capability">
              <span>{ctrip.orderBoundary || '下单边界'}</span>
              <strong>{ctrip.orderBoundaryDesc || 'Pinvou 负责查询与整理，最终下单和支付在携程完成'}</strong>
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('close')}>
                {copy.root?.cancel || '取消'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('start_browser_assist')}>
                {ctrip.openCtrip || '去携程继续办理'}
              </button>
            </div>
          </div>
        )}

        {flow.step === 'browser_prepare' && (
          <div className="aios-ctrip-state">
            <i className="fas fa-circle-notch fa-spin" />
            <h3>{ctrip.browserPrepare || '正在打开携程官方页面'}</h3>
            <p>{ctrip.browserPrepareDesc || '我会带着已确认的查询条件进入携程。登录、验证码、实名信息、提交订单和支付会由你亲自处理。'}</p>
          </div>
        )}

        {flow.step === 'browser_searching' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-magnifying-glass-location" />
              <div>
                <h3>{ctrip.browserSearchingTitle || '已在携程窗口尝试搜索'}</h3>
                <p>{ctrip.browserSearchingDesc || '我已打开携程专用窗口，并尝试填写出发地、目的地、日期和人数后触发搜索。页面结果、登录、验证码、实名信息和最终提交仍需要你在携程窗口确认。'}</p>
              </div>
            </div>
            <div className="aios-ctrip-handoff">
              <div className="aios-ctrip-handoff-title">{ctrip.autoSearchCard || '自动搜索记录'}</div>
              {[
                [ctrip.autoFilled || '已尝试填写', Array.isArray(flow.browserAssist?.filled) && flow.browserAssist.filled.length ? flow.browserAssist.filled.join(' / ') : (ctrip.autoFilledUnknown || '页面可识别项')],
                [ctrip.autoClickedSearch || '触发搜索', flow.browserAssist?.clickedSearch ? (copy.root?.yes || '是') : (copy.root?.no || '否')],
                [ctrip.ctripLink || '携程链接', flow.ctripUrl],
              ].map(([label, value]) => (
                <div key={label} className="aios-ctrip-handoff-row">
                  <span>{label}</span>
                  <strong>{value || '-'}</strong>
                </div>
              ))}
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.autoBoundary || '自动化边界'}</span>
              <strong>{ctrip.autoBoundaryDesc || 'Pinvou 只尝试搜索条件，不处理登录、验证码、实名信息、提交订单或支付'}</strong>
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('cancel_browser_assist')}>
                {ctrip.endAssist || '结束协助'}
              </button>
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('start_browser_assist')}>
                {ctrip.retryAutoSearch || '重新尝试搜索'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('browser_result_ready')}>
                {ctrip.resultReady || '我看到结果了'}
              </button>
            </div>
          </div>
        )}

        {(flow.step === 'user_action_required' || flow.step === 'browser_blocked') && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className={`fas ${flow.step === 'browser_blocked' ? 'fa-triangle-exclamation' : 'fa-hand-pointer'}`} />
              <div>
                <h3>{flow.step === 'browser_blocked' ? (ctrip.browserBlocked || '浏览器协助受阻') : (ctrip.userActionRequired || '需要你在携程接管')}</h3>
                <p>{flow.step === 'browser_blocked'
                  ? (ctrip.browserBlockedDesc || '携程页面可能出现风控、验证码、网络异常或页面结构变化。我已保留查询条件，方便你手动继续。')
                  : (ctrip.userActionRequiredDesc || '携程页面已打开。涉及登录、验证码、乘机人实名信息、提交订单和支付时，需要你亲自确认。')}</p>
              </div>
            </div>
            <div className="aios-ctrip-handoff">
              <div className="aios-ctrip-handoff-title">{ctrip.handoffCard || '接管信息'}</div>
              {handoffItems.map(([label, value]) => (
                <div key={label} className="aios-ctrip-handoff-row">
                  <span>{label}</span>
                  <strong>{value}</strong>
                </div>
              ))}
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.manualBoundary || '协助边界'}</span>
              <strong>{ctrip.manualBoundaryDesc || 'Pinvou 不输入账号密码、不绕过验证码；最终提交前必须确认，支付由你完成'}</strong>
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('cancel_browser_assist')}>
                {ctrip.endAssist || '结束协助'}
              </button>
              {flow.step === 'user_action_required' && (
                <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('continue_after_user_action')}>
                  {ctrip.continueAssist || '我已完成，继续协助'}
                </button>
              )}
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('open_ctrip')}>
                {ctrip.reopenCtrip || '重新打开携程'}
              </button>
            </div>
          </div>
        )}

        {flow.step === 'browser_order_review' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-clipboard-check" />
              <div>
                <h3>{ctrip.orderReviewTitle || '核对携程订单页'}</h3>
                <p>{ctrip.orderReviewDesc || '请以携程页面为准核对航班/酒店、乘机人数量、总价和退改规则。当前版本尚未接入网页 DOM 自动读取。'}</p>
              </div>
            </div>
            <div className="aios-ctrip-handoff">
              <div className="aios-ctrip-handoff-title">{ctrip.handoffCard || '接管信息'}</div>
              {handoffItems.map(([label, value]) => (
                <div key={label} className="aios-ctrip-handoff-row">
                  <span>{label}</span>
                  <strong>{value}</strong>
                </div>
              ))}
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.reviewBoundary || '核对边界'}</span>
              <strong>{ctrip.reviewBoundaryDesc || '价格、库存、退改规则以携程页面实时显示为准；有变化必须重新确认'}</strong>
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('open_ctrip')}>
                {ctrip.reopenCtrip || '重新打开携程'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('request_submit_confirmation')}>
                {ctrip.goSubmitConfirm || '进入提交确认'}
              </button>
            </div>
          </div>
        )}

        {flow.step === 'submit_confirmation_required' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-file-signature" />
              <div>
                <h3>{ctrip.submitConfirmTitle || '最终提交前确认'}</h3>
                <p>{ctrip.submitConfirmDesc || '请再次核对携程页面显示的总价、乘机人数量和退改规则。当前版本不能自动点击提交，确认后会回到携程页面由你手动提交。'}</p>
              </div>
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.paymentBoundary || '支付边界'}</span>
              <strong>{ctrip.paymentBoundaryDesc || '进入支付渠道、选择支付方式、付款确认和支付验证码全部由你本人完成'}</strong>
            </div>
            <div className="aios-ctrip-handoff">
              <div className="aios-ctrip-handoff-title">{ctrip.submitChecklist || '提交前检查'}</div>
              {[
                [ctrip.checkRoute || '航班/酒店', ctrip.checkRouteDesc || '以携程页面当前展示为准'],
                [ctrip.checkPrice || '总价', ctrip.checkPriceDesc || '确认没有价格或库存变化'],
                [ctrip.checkPeople || '乘机人/入住人', ctrip.checkPeopleDesc || '只核对数量，不在 Pinvou 展示完整证件号'],
                [ctrip.checkRules || '退改规则', ctrip.checkRulesDesc || '确认退改签/取消规则可以接受'],
              ].map(([label, value]) => (
                <div key={label} className="aios-ctrip-handoff-row">
                  <span>{label}</span>
                  <strong>{value}</strong>
                </div>
              ))}
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('continue_after_user_action')}>
                {ctrip.backToReview || '返回核对'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('confirm_submit_order')}>
                {ctrip.manualSubmit || '我已确认，去携程手动提交'}
              </button>
            </div>
          </div>
        )}

        {flow.step === 'payment_required' && (
          <div className="aios-ctrip-section">
            <div className="aios-ctrip-hero">
              <i className="fas fa-credit-card" />
              <div>
                <h3>{ctrip.paymentRequiredTitle || '支付由你完成'}</h3>
                <p>{ctrip.paymentRequiredDesc || '如果你已在携程提交订单，请在携程页面完成支付。Pinvou 不点击支付入口、不选择支付方式、不读取或提交支付验证码。'}</p>
              </div>
            </div>
            <div className="aios-ctrip-capability">
              <span>{ctrip.paymentBoundary || '支付边界'}</span>
              <strong>{ctrip.paymentBoundaryDesc || '进入支付渠道、选择支付方式、付款确认和支付验证码全部由你本人完成'}</strong>
            </div>
            {error && <div className="aios-input-error">{error}</div>}
            <div className="aios-input-actions">
              <button type="button" className="aios-input-cancel" disabled={disabled} onClick={() => onAction?.('cancel_browser_assist')}>
                {ctrip.endAssist || '结束协助'}
              </button>
              <button type="button" className="aios-input-submit" disabled={disabled} onClick={() => onAction?.('open_ctrip')}>
                {ctrip.reopenCtrip || '重新打开携程'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export function AiosShell({
  theme,
  t,
  tasks,
  activeSessionId,
  activeTask,
  chatItems,
  busy,
  thinking,
  artifacts,
  artifactApi,
  onOpenTask,
  onDeleteTask,
  onCloseTask,
  onSubmitPrompt,
  onSendActive,
  onToggleTheme,
  voiceInput,
  onVoiceInput,
  onCancelVoiceInput,
  onClearVoiceInput,
  onSubmitUserInput,
  onCancelUserInput,
  ctripFlow,
  onCtripAction,
}) {
  const copy = t.uiAios;
  const [clock, setClock] = useState(formatClock);
  const [artifactLibraryOpen, setArtifactLibraryOpen] = useState(false);
  const [artifactLibrary, setArtifactLibrary] = useState({ loading: false, items: [], error: '' });
  const visibleTasks = tasks || [];
  const hasTasks = visibleTasks.length > 0;
  const isDark = theme === 'dark';
  const localizedCopy = useMemo(() => ({ ...copy, root: t }), [copy, t]);
  const fallbackArtifacts = useMemo(() => (
    (artifacts || []).map(item => ({
      ...item,
      sessionId: item.sessionId || item.session_id || activeTask?.id,
      source: item.source || activeTask?.title || '',
    }))
  ), [artifacts, activeTask]);
  const libraryItems = artifactLibrary.items.length > 0 ? artifactLibrary.items : fallbackArtifacts;
  const needsUserInput = activeUserInputItem(chatItems);
  const activeGeneratedText = latestAssistantText(chatItems || [], { busy });
  const activeHasArtifact = Array.isArray(artifacts) && artifacts.length > 0;
  const activeArtifactName = activeHasArtifact ? artifactLabel(artifacts[artifacts.length - 1], localizedCopy, artifacts.length - 1) : '';
  const renderedTasks = visibleTasks.map(task => {
    if (!activeSessionId || String(task.id) !== String(activeSessionId)) return task;
    return {
      ...task,
      status: needsUserInput ? 'waiting_input' : busy ? 'processing' : task.status,
      needsUserInput: Boolean(needsUserInput),
      hasArtifact: activeHasArtifact,
      artifactName: activeArtifactName,
      resultPreview: activeHasArtifact
        ? activeArtifactName
        : (activeGeneratedText ? truncateText(cleanMarkdownText(activeGeneratedText), 160) : task.resultPreview),
    };
  });

  useEffect(() => {
    const timer = window.setInterval(() => setClock(formatClock()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!artifactLibraryOpen) return undefined;
    let cancelled = false;
    setArtifactLibrary(prev => ({ ...prev, loading: true, error: '' }));
    (async () => {
      try {
        const indexed = artifactApi?.listDeliverableIndex ? await artifactApi.listDeliverableIndex() : [];
        if (!cancelled) {
          setArtifactLibrary({ loading: false, items: Array.isArray(indexed) ? indexed : [], error: '' });
        }
      } catch (error) {
        if (!cancelled) {
          setArtifactLibrary({ loading: false, items: [], error: String(error) });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [artifactLibraryOpen, artifactApi]);

  return (
    <div className={`aios-root transition-colors duration-500 ${isDark ? 'dark' : ''}`} data-testid="app-root" data-current-view="aios">
      <div className="aios-noise" aria-hidden="true" />
      <div className="flex-1 flex flex-col p-6 lg:p-12 pb-32 h-full w-full relative z-10 overflow-hidden">
        <header className="flex justify-between items-center mb-8 text-gray-800 dark:text-gray-100 drop-shadow-sm transition-colors">
          <div className="text-xl font-medium tracking-tight">{clock}</div>
          <div className="flex items-center space-x-5">
            <button
              type="button"
              className="aios-header-icon-button"
              onClick={() => setArtifactLibraryOpen(true)}
              aria-label="搜索产物"
              title="搜索产物"
            >
              <i className="fas fa-magnifying-glass" />
            </button>
            <button
              type="button"
              className="aios-header-icon-button"
              onClick={onToggleTheme}
              aria-label={copy.toggleTheme}
              title={copy.toggleTheme}
            >
              <i className={`fas ${isDark ? 'fa-sun' : 'fa-moon'}`} />
            </button>
            <i className="fas fa-wifi text-sm" />
            <i className="fas fa-battery-full text-sm" />
          </div>
        </header>

        <main className="flex-1 overflow-y-auto pr-4 pb-12 flex flex-col custom-scrollbar">
          <h1 className="text-3xl font-semibold mb-6 text-gray-800 dark:text-gray-100 drop-shadow-md tracking-tight transition-colors">
            {greetingFor(copy)}
          </h1>
          {hasTasks ? (
            <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-6" aria-label={copy.taskGrid}>
              {renderedTasks.map((task, index) => (
                <AiosTaskCard
                  key={`${task.taskKind || 'chat'}:${task.id}`}
                  task={task}
                  index={index}
                  copy={copy}
                  onOpen={onOpenTask}
                  onDelete={onDeleteTask}
                />
              ))}
            </div>
          ) : (
            <AiosEmptyState copy={copy} onQuickPrompt={onSubmitPrompt} />
          )}
        </main>
      </div>

      <div className="aios-global-composer-dock fixed bottom-0 left-0 w-full p-6 lg:p-8 flex justify-center z-40 pointer-events-none">
        <AiosComposer
          copy={copy}
          placeholder={copy.globalInputPlaceholder}
          voiceOnly
          autoSubmitVoice
          voiceInput={voiceInput}
          onVoiceClick={onVoiceInput}
          onCancelVoiceInput={onCancelVoiceInput}
          onClearVoiceInput={onClearVoiceInput}
          onSubmit={onSubmitPrompt}
        />
      </div>

      {activeTask && (
        <AiosSessionModal
          copy={localizedCopy}
          task={activeTask}
          chatItems={chatItems}
          busy={busy}
          thinking={thinking}
          artifacts={artifacts}
          artifactApi={artifactApi}
          voiceInput={voiceInput}
          onVoiceClick={onVoiceInput}
          onCancelVoiceInput={onCancelVoiceInput}
          onClearVoiceInput={onClearVoiceInput}
          onClose={onCloseTask}
          onSend={onSendActive}
          onSubmitUserInput={onSubmitUserInput}
          onCancelUserInput={onCancelUserInput}
        />
      )}

      {artifactLibraryOpen && (
        <AiosArtifactLibraryModal
          copy={localizedCopy}
          items={libraryItems}
          loading={artifactLibrary.loading}
          error={artifactLibrary.error}
          artifactApi={artifactApi}
          onClose={() => setArtifactLibraryOpen(false)}
        />
      )}

      {needsUserInput && !activeTask && (
        <AiosUserInputOverlay
          item={needsUserInput}
          copy={localizedCopy}
          onSubmit={onSubmitUserInput}
          onCancel={onCancelUserInput}
        />
      )}

      {ctripFlow?.visible && (
        <AiosCtripConnectorOverlay
          flow={ctripFlow}
          copy={localizedCopy}
          onAction={onCtripAction}
        />
      )}
    </div>
  );
}
