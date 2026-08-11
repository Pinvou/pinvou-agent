const DRAG_THRESHOLD_BY_POINTER = Object.freeze({
  mouse: 4,
  pen: 6,
  touch: 8,
});

const FLOATING_VOICE_CLICK_SUPPRESSION_MS = 800;

function dragThreshold(pointerType) {
  return DRAG_THRESHOLD_BY_POINTER[pointerType] || DRAG_THRESHOLD_BY_POINTER.pen;
}

function canStartFloatingVoiceDrag({ pointerType, isPrimary, button }) {
  if (pointerType === 'touch') return isPrimary === true;
  if (pointerType === 'mouse' || pointerType === 'pen') {
    return isPrimary === true && button === 0;
  }
  return false;
}

function createFloatingVoiceDragSession({ pointerId, pointerType, clientX, clientY, offsetX, offsetY }) {
  return {
    pointerId,
    pointerType: pointerType || 'mouse',
    startX: clientX,
    startY: clientY,
    offsetX,
    offsetY,
    dragging: false,
    suppressClick: false,
    suppressedPointerId: null,
    suppressedPointerType: null,
  };
}

function moveFloatingVoiceDrag(session, { pointerId, clientX, clientY, buttons }) {
  if (!session || session.pointerId !== pointerId) return { kind: 'ignored' };
  if ((session.pointerType === 'mouse' || session.pointerType === 'pen') && buttons === 0) {
    return { kind: 'released' };
  }

  const distance = Math.hypot(clientX - session.startX, clientY - session.startY);
  if (!session.dragging && distance < dragThreshold(session.pointerType)) {
    return { kind: 'pending' };
  }

  const started = !session.dragging;
  session.dragging = true;
  return {
    kind: 'move',
    started,
    x: clientX - session.offsetX,
    y: clientY - session.offsetY,
  };
}

function finishFloatingVoiceDrag(session, pointerId, { suppressCompatibleClick = false } = {}) {
  if (!session || session.pointerId !== pointerId) {
    return { matched: false, wasDragging: false };
  }
  const wasDragging = session.dragging;
  session.suppressClick = wasDragging && suppressCompatibleClick;
  session.suppressedPointerId = session.suppressClick ? pointerId : null;
  session.suppressedPointerType = session.suppressClick ? session.pointerType : null;
  session.pointerId = null;
  session.dragging = false;
  return { matched: true, wasDragging };
}

function consumeFloatingVoiceDragClick(session, { detail, pointerId, pointerType } = {}) {
  if (!session || !session.suppressClick) return false;
  if (detail === 0) return false;
  if (pointerId !== session.suppressedPointerId || pointerType !== session.suppressedPointerType) return false;
  clearFloatingVoiceDragClick(session);
  return true;
}

function clearFloatingVoiceDragClick(session) {
  if (!session) return;
  session.suppressClick = false;
  session.suppressedPointerId = null;
  session.suppressedPointerType = null;
}

export {
  DRAG_THRESHOLD_BY_POINTER,
  FLOATING_VOICE_CLICK_SUPPRESSION_MS,
  canStartFloatingVoiceDrag,
  clearFloatingVoiceDragClick,
  consumeFloatingVoiceDragClick,
  createFloatingVoiceDragSession,
  finishFloatingVoiceDrag,
  moveFloatingVoiceDrag,
};
