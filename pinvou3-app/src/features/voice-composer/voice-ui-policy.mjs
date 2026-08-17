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

function isVoiceActive(voiceInput) {
  const status = voiceInput && voiceInput.status;
  return status === 'requesting_permission'
    || status === 'recording'
    || status === 'transcribing'
    || status === 'postprocessing';
}

function isVoiceBusy(voiceInput) {
  const status = voiceInput && voiceInput.status;
  return status === 'transcribing' || status === 'postprocessing';
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

export {
  isVoiceActive,
  isVoiceBusy,
  normalizeVoiceMode,
  voicePostprocessingLabel,
  voiceStatusLabel,
};
