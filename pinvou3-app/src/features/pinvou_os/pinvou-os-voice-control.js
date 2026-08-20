const VoiceControlAction = Object.freeze({
  BEGIN: 'begin',
  CANCEL: 'cancel',
  STOP_RECORDING: 'stop_recording',
});

export function isVoiceCaptureActive(status) {
  return ['requesting_permission', 'recording', 'transcribing'].includes(String(status || 'idle'));
}

export function getVoiceControlState(status, voice) {
  const normalizedStatus = String(status || 'idle');
  const action = normalizedStatus === 'recording'
    ? VoiceControlAction.STOP_RECORDING
    : normalizedStatus === 'requesting_permission' || normalizedStatus === 'transcribing'
      ? VoiceControlAction.CANCEL
      : VoiceControlAction.BEGIN;
  const requiredMethod = action === VoiceControlAction.CANCEL
    ? voice && voice.cancelVoiceInput
    : voice && voice.startVoiceInput;

  return {
    action,
    disabled: typeof requiredMethod !== 'function',
  };
}

export function activateVoiceControl(control, voice, beginVoiceInput) {
  if (!control || control.disabled) return false;
  if (control.action === VoiceControlAction.CANCEL) {
    voice.cancelVoiceInput();
    return true;
  }
  if (control.action === VoiceControlAction.STOP_RECORDING) {
    // The voice bridge owns the recording session and performs its one-shot cleanup.
    // Calling start again is its existing stop-and-transcribe protocol.
    voice.startVoiceInput('', () => {});
    return true;
  }
  beginVoiceInput();
  return true;
}
