import { useEffect, useState } from 'react';

// The pill fades out before unmounting; this window must outlast the
// .voice-pop-out animation (0.14s) so the fade completes before removal.
export const VOICE_PILL_EXIT_MS = 160;

// Drives the pill's presence animation. `phase` is "shown" while the voice
// input is active, "closing" through the exit-animation window, and "hidden"
// once the pill can leave the tree. `snapshot` freezes the last active
// voiceInput so the fade-out replays the pre-completion content instead of
// flashing a terminal status the active pill never shows.
//
// Phase flips follow React's "adjust state during render" pattern (compare
// against the previous render); the effect below only arms the unmount timer
// for the closing phase, so no effect ever calls setState synchronously.
export function useVoicePillPresence(visible, input) {
  const [phase, setPhase] = useState(() => (visible ? 'shown' : 'hidden'));
  const [snapshot, setSnapshot] = useState(input || null);
  const [prevVisible, setPrevVisible] = useState(visible);
  if (visible !== prevVisible) {
    setPrevVisible(visible);
    setPhase((prev) => (visible ? 'shown' : prev === 'shown' ? 'closing' : prev));
  }
  // Track the latest active input each visible render; once the pill starts
  // closing, updates stop and the snapshot keeps the last on-screen content.
  if (visible && input && input !== snapshot) setSnapshot(input);
  useEffect(() => {
    if (phase !== 'closing') return;
    const timer = setTimeout(() => setPhase('hidden'), VOICE_PILL_EXIT_MS);
    return () => clearTimeout(timer);
  }, [phase]);
  const closing = phase === 'closing';
  return { mounted: phase !== 'hidden', closing, snapshot };
}
