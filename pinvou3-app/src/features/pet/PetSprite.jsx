import React, { useEffect, useMemo, useState } from 'react';
import { useReducedMotion } from '../../hooks/useReducedMotion.js';
import { buildPetSpritePlayback, PET_FRAME_H, PET_FRAME_W } from './pet-animation.js';

export function PetSprite({ pet, animation }) {
  const reducedMotion = useReducedMotion();
  const playback = useMemo(
    () => buildPetSpritePlayback(pet, animation, { reducedMotion }),
    [animation, pet, reducedMotion],
  );
  const { sequence } = playback;
  const [frameIndex, setFrameIndex] = useState(0);
  useEffect(() => setFrameIndex(0), [sequence]);
  useEffect(() => {
    if ((reducedMotion && !playback.animateWithReducedMotion) || sequence.frames.length <= 1) return undefined;
    const frame = sequence.frames[frameIndex] || sequence.frames[0];
    const timer = window.setTimeout(() => {
      setFrameIndex((current) => (
        current + 1 < sequence.frames.length ? current + 1 : sequence.loopStartIndex
      ));
    }, frame.durationMs);
    return () => window.clearTimeout(timer);
  }, [frameIndex, playback.animateWithReducedMotion, reducedMotion, sequence]);

  const frame = sequence.frames[frameIndex] || sequence.frames[0];
  return (
    <div
      className="pet-sprite"
      style={{
        width: PET_FRAME_W,
        height: PET_FRAME_H,
        backgroundImage: `url(${playback.sheetUrl})`,
        backgroundPosition: `-${frame.column * PET_FRAME_W}px -${frame.row * PET_FRAME_H}px`,
        transform: playback.flipX ? 'scaleX(-1)' : undefined,
      }}
    />
  );
}
