function normalizeVoiceMode(mode) {
  if (mode === 'task') return 'task';
  if (mode === 'edit' || mode === 'voice_edit' || mode === 'draft_edit') return 'edit';
  return 'dictation';
}

function voicePostprocessingLabel(mode, copy) {
  const voiceMode = normalizeVoiceMode(mode);
  if (voiceMode === 'task') return copy.voiceTaskPostprocessing;
  if (voiceMode === 'edit') return copy.voiceEditPostprocessing || copy.voicePostprocessing;
  return copy.voicePostprocessing;
}

// 语音会话状态集(单一来源):isVoiceActive/isVoiceBusy 与 UI 层(如
// VoiceRecordingPill)的状态判定都从这里派生,新增状态只改这两处。
export const VOICE_ACTIVE_STATUSES = ['requesting_permission', 'recording', 'transcribing', 'postprocessing'];
export const VOICE_BUSY_STATUSES = ['transcribing', 'postprocessing'];

function isVoiceActive(voiceInput) {
  return VOICE_ACTIVE_STATUSES.includes(voiceInput && voiceInput.status);
}

function isVoiceBusy(voiceInput) {
  return VOICE_BUSY_STATUSES.includes(voiceInput && voiceInput.status);
}

function isVoiceRecording(voiceInput) {
  return !!voiceInput && voiceInput.status === 'recording';
}

function shouldShowVoicePill(voiceInput) {
  const status = voiceInput && voiceInput.status;
  return status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing'
    || (status === 'requesting_permission' && voiceInput.stage !== 'device');
}

function voiceStatusLabel(voiceInput, mode, copy) {
  const status = voiceInput && voiceInput.status;
  const voiceMode = normalizeVoiceMode(mode || (voiceInput && voiceInput.mode));
  if (status === 'requesting_permission') return copy.voiceRequesting;
  if (status === 'recording') return copy.voiceRecording;
  if (status === 'postprocessing') return voicePostprocessingLabel(voiceMode, copy);
  if (status === 'transcribing') return copy.voiceTranscribing;
  if (status === 'completed') return copy.voiceCompleted;
  return (voiceInput && voiceInput.message) || copy.voiceInputFailed;
}

function primaryVoiceLabel(voiceInput, mode, copy) {
  const status = voiceInput && voiceInput.status;
  const voiceMode = normalizeVoiceMode(mode || (voiceInput && voiceInput.mode));
  if (status === 'recording') return copy.voiceStop;
  if (status === 'failed') return copy.voiceRetry;
  if (status === 'requesting_permission') return copy.voiceCancel;
  if (status === 'transcribing') return copy.voiceTranscribing;
  if (status === 'postprocessing') return voicePostprocessingLabel(voiceMode, copy);
  return copy.voiceStart;
}

function voiceAsrProgressPercent(voiceAsrSetup) {
  const progress = (voiceAsrSetup && voiceAsrSetup.progress) || {};
  if (progress.stage === 'model' && progress.total) {
    return Math.floor(progress.downloaded / progress.total * 100);
  }
  return null;
}

function voiceAsrBusyState(voiceAsrSetup, chatCopy) {
  const setup = voiceAsrSetup || {};
  const busy = !!(setup.installing || setup.cancelling);
  const cancelling = !!setup.cancelling;
  const pct = voiceAsrProgressPercent(setup);
  const label = cancelling
    ? chatCopy.cancelling
    : chatCopy.downloadingModel(pct != null ? pct + '%' : '...');
  return {
    busy,
    cancelling,
    pct,
    label,
  };
}

export {
  isVoiceActive,
  isVoiceBusy,
  isVoiceRecording,
  normalizeVoiceMode,
  primaryVoiceLabel,
  shouldShowVoicePill,
  voiceAsrBusyState,
  voicePostprocessingLabel,
  voiceStatusLabel,
};
