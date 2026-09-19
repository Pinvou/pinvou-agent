import { useEffect, useState } from 'react';

/**
 * Shared sprite frame player for the pet atlas components (PetWindow sprite and
 * the settings-card hover preview). Advances frameIndex once per frame's
 * durationMs and loops back to sequence.loopStartIndex; returns the frame to
 * render. The frame index resets whenever resetKey changes (defaults to the
 * sequence identity), and { reducedMotion } holds the first frame instead of
 * animating.
 */
export function useSpriteFramePlayer(sequence, { reducedMotion = false, resetKey = sequence } = {}) {
  const [frameIndex, setFrameIndex] = useState(0);

  // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset the frame index on animation-sequence switch; one-shot mirror
  useEffect(() => setFrameIndex(0), [resetKey]);
  useEffect(() => {
    if (reducedMotion || sequence.frames.length <= 1) return;
    const frame = sequence.frames[frameIndex] || sequence.frames[0];
    const timer = window.setTimeout(() => {
      setFrameIndex((current) => (
        current + 1 < sequence.frames.length ? current + 1 : sequence.loopStartIndex
      ));
    }, frame.durationMs);
    return () => window.clearTimeout(timer);
  }, [frameIndex, reducedMotion, sequence]);

  return sequence.frames[frameIndex] || sequence.frames[0];
}
