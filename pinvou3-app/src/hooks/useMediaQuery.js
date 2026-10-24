import { useEffect, useState } from 'react';

/**
 * Subscribe to a matchMedia query, including runtime changes (not just a
 * mount-time snapshot). Seeds from the live query; false when matchMedia is
 * unavailable. useReducedMotion / useSystemDarkMode / useCompactViewport
 * previously re-spelled this subscription.
 * @param {string} query - media query to watch
 * @returns {boolean} whether the query currently matches
 */
export function useMediaQuery(query) {
  const [matches, setMatches] = useState(() => (
    typeof window.matchMedia === 'function' && window.matchMedia(query).matches
  ));

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(query);
    const onChange = () => setMatches(media.matches);
    media.addEventListener?.('change', onChange);
    return () => media.removeEventListener?.('change', onChange);
  }, [query]);

  return matches;
}
