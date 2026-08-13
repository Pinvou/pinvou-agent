let activeTarget = null;

function registerVoiceTarget(target) {
  if (!target || !target.targetId) throw new Error('voice target requires targetId');
  activeTarget = target;
  return () => {
    if (activeTarget && activeTarget.targetId === target.targetId) activeTarget = null;
  };
}

function activateVoiceTarget(target) {
  if (!target || !target.targetId) return;
  activeTarget = target;
}

function getActiveVoiceTarget() {
  return activeTarget;
}

function clearActiveVoiceTarget(targetId) {
  if (!targetId || (activeTarget && activeTarget.targetId === targetId)) activeTarget = null;
}

function isActiveVoiceTarget(targetId, voiceSessionId) {
  if (!activeTarget || activeTarget.targetId !== targetId) return false;
  if (voiceSessionId && activeTarget.voiceSessionId && activeTarget.voiceSessionId !== voiceSessionId) return false;
  if (typeof activeTarget.isStillActive === 'function' && !activeTarget.isStillActive()) return false;
  return true;
}

export {
  activateVoiceTarget,
  clearActiveVoiceTarget,
  getActiveVoiceTarget,
  isActiveVoiceTarget,
  registerVoiceTarget,
};
