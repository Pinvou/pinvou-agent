import { useEffect, useRef, useState } from 'react';

/**
 * Register the currently visible composer as the sole owner of file drops.
 *
 * The platform bridges only implement file transport. Keeping drag ownership
 * in React prevents a hidden Work composer from consuming files dropped on
 * the Design or ACP Code surfaces.
 */
export function useAttachmentDrop({ enabled = true, onFiles }) {
  const [active, setActive] = useState(false);
  const onFilesRef = useRef(onFiles);
  // eslint-disable-next-line react-hooks/refs -- latest-ref sync: read only inside drop callbacks; render output does not depend on it
  onFilesRef.current = onFiles;

  useEffect(() => {
    if (!enabled || typeof window === 'undefined' || typeof document === 'undefined') {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- synchronously reset the drag highlight when enabled flips to false; one-shot mirror
      setActive(false);
      return;
    }
    const controller = window.PinvouAttachmentDropController;
    if (!controller || typeof controller.install !== 'function') {
      console.warn('[attachment] drop controller is unavailable');
      return;
    }

    let disposed = false;
    const uninstall = controller.install({
      document,
      onActiveChange(next) {
        if (!disposed) setActive(Boolean(next));
      },
      onFiles(files) {
        return onFilesRef.current ? onFilesRef.current(files) : undefined;
      },
    });

    return () => {
      disposed = true;
      if (typeof uninstall === 'function') uninstall();
    };
  }, [enabled]);

  return enabled && active;
}
