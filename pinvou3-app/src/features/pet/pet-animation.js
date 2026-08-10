// 图集协议常量：所有内置宠物共享同一帧规格。行数允许 9（仅协议用到的
// 行 0-8）或 11（含两行保留位）；总高度由行数决定，消费方不得假设定值。
export const PET_FRAME_W = 192;
export const PET_FRAME_H = 208;
export const PET_ATLAS_COLS = 8;

export const CODEX_IDLE_FRAME_DURATIONS_MS = Object.freeze([280, 110, 110, 140, 140, 320]);
const PREVIEW_ACTION_DURATION_SCALE = 1.8;
const VIVI_WALK_FRAME_COUNT = 8;
const VIVI_WALK_FRAME_DURATION_MS = 130;
const VIVI_DRAG_FRAME_DURATIONS_MS = Object.freeze([420, 260, 300, 420, 320, 260, 170, 170]);
const VIVI_BLINK_INTERVAL_MS = 8_000;
export const VIVI_IDLE_SPECIAL_DURATION_MS = 2_100;

export const CODEX_ANIMATIONS = Object.freeze({
  idle: Object.freeze({ row: 0, frames: 6, frameDurationMs: 140 }),
  'running-right': Object.freeze({ row: 1, frames: 8, frameDurationMs: 120, lastFrameDurationMs: 220 }),
  'running-left': Object.freeze({ row: 2, frames: 8, frameDurationMs: 120, lastFrameDurationMs: 220 }),
  waving: Object.freeze({ row: 3, frames: 4, frameDurationMs: 140, lastFrameDurationMs: 280 }),
  jumping: Object.freeze({ row: 4, frames: 5, frameDurationMs: 140, lastFrameDurationMs: 280 }),
  failed: Object.freeze({ row: 5, frames: 8, frameDurationMs: 140, lastFrameDurationMs: 240 }),
  waiting: Object.freeze({ row: 6, frames: 6, frameDurationMs: 150, lastFrameDurationMs: 260 }),
  running: Object.freeze({ row: 7, frames: 6, frameDurationMs: 120, lastFrameDurationMs: 220 }),
  review: Object.freeze({ row: 8, frames: 6, frameDurationMs: 150, lastFrameDurationMs: 280 }),
});

function actionFrames(animation) {
  const spec = CODEX_ANIMATIONS[animation] || CODEX_ANIMATIONS.idle;
  return Array.from({ length: spec.frames }, (_, column) => ({
    row: spec.row,
    column,
    durationMs: column === spec.frames - 1
      ? (spec.lastFrameDurationMs || spec.frameDurationMs)
      : spec.frameDurationMs,
  }));
}

function slowIdleFrames() {
  return CODEX_IDLE_FRAME_DURATIONS_MS.map((durationMs, column) => ({
    row: CODEX_ANIMATIONS.idle.row,
    column,
    durationMs: durationMs * 6,
  }));
}

function previewActionFrames(animation) {
  return actionFrames(animation).map((frame) => ({
    ...frame,
    durationMs: Math.round(frame.durationMs * PREVIEW_ACTION_DURATION_SCALE),
  }));
}

/**
 * Selector-card hover preview: a compact multi-action introduction, then the
 * slow idle rest loop.
 * Reduced-motion mode renders the single idle rest frame instead.
 */
export function buildPreviewSequence({ reducedMotion = false } = {}) {
  if (reducedMotion) {
    return {
      frames: [{ row: CODEX_ANIMATIONS.idle.row, column: 0, durationMs: CODEX_IDLE_FRAME_DURATIONS_MS[0] }],
      loopStartIndex: 0,
    };
  }
  const introduction = ['waving', 'jumping', 'review'].flatMap(previewActionFrames);
  return {
    frames: [...introduction, ...slowIdleFrames()],
    loopStartIndex: introduction.length,
  };
}

/**
 * Codex Desktop plays an active status three times, then rests on its slow idle
 * loop. Reduced-motion mode always renders a single representative frame.
 */
export function buildAnimationSequence(animation, { reducedMotion = false } = {}) {
  const name = CODEX_ANIMATIONS[animation] ? animation : 'idle';
  const activeFrames = actionFrames(name);

  if (reducedMotion) {
    return { frames: [activeFrames[0]], loopStartIndex: 0 };
  }

  if (name === 'idle') {
    return { frames: slowIdleFrames(), loopStartIndex: 0 };
  }

  const repeated = [...activeFrames, ...activeFrames, ...activeFrames];
  return {
    frames: [...repeated, ...slowIdleFrames()],
    loopStartIndex: repeated.length,
  };
}

export function buildPetSpritePlayback(pet, animation, { reducedMotion = false } = {}) {
  const walking = pet?.id === 'vivi'
    && !!pet.walkSheetUrl
    && (animation === 'running-right' || animation === 'running-left');
  const hovering = pet?.id === 'vivi'
    && !!pet.dragSheetUrl
    && animation === 'hover-special';
  if (pet?.id === 'vivi' && animation === 'idle') {
    return {
      sequence: {
        frames: reducedMotion
          ? [{ row: 0, column: 0, durationMs: VIVI_BLINK_INTERVAL_MS }]
          : [
            { row: 0, column: 0, durationMs: VIVI_BLINK_INTERVAL_MS },
            { row: 0, column: 1, durationMs: 90 },
            { row: 0, column: 2, durationMs: 100 },
            { row: 0, column: 1, durationMs: 90 },
            { row: 0, column: 0, durationMs: VIVI_BLINK_INTERVAL_MS },
          ],
        loopStartIndex: 0,
      },
      sheetUrl: pet.sheetUrl,
      flipX: false,
      animateWithReducedMotion: false,
    };
  }
  if (!walking && !hovering) {
    return {
      sequence: buildAnimationSequence(animation, { reducedMotion }),
      sheetUrl: pet?.sheetUrl,
      flipX: false,
      animateWithReducedMotion: false,
    };
  }
  return {
    sequence: {
      frames: (hovering
        ? Array.from({ length: VIVI_WALK_FRAME_COUNT - 1 }, (_, index) => index + 1)
        : Array.from({ length: VIVI_WALK_FRAME_COUNT }, (_, index) => index)
      ).map((column) => ({
        row: 0,
        column,
        durationMs: hovering
          ? (VIVI_DRAG_FRAME_DURATIONS_MS[column] || VIVI_WALK_FRAME_DURATION_MS)
          : VIVI_WALK_FRAME_DURATION_MS,
      })),
      loopStartIndex: 0,
    },
    sheetUrl: hovering ? pet.dragSheetUrl : pet.walkSheetUrl,
    flipX: walking && animation === 'running-right',
    animateWithReducedMotion: true,
  };
}

export function nextViviIdleSpecialDelay(random = Math.random) {
  const unit = Math.min(1, Math.max(0, Number(random()) || 0));
  return 20_000 + Math.round(unit * 20_000);
}
