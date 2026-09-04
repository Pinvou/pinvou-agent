let activeTarget = null;

function registerVoiceTarget(target) {
  if (!target || !target.targetId) throw new Error('voice target requires targetId');
  activeTarget = target;
  return () => {
    if (activeTarget && activeTarget.targetId === target.targetId) activeTarget = null;
  };
}

function getActiveVoiceTarget() {
  return activeTarget;
}

function isActiveVoiceTarget(targetId, voiceSessionId) {
  if (!activeTarget || activeTarget.targetId !== targetId) return false;
  if (voiceSessionId && activeTarget.voiceSessionId && activeTarget.voiceSessionId !== voiceSessionId) return false;
  if (typeof activeTarget.isStillActive === 'function' && !activeTarget.isStillActive()) return false;
  return true;
}

export {
  getActiveVoiceTarget,
  isActiveVoiceTarget,
  registerVoiceTarget,
};
