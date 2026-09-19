import { useEffect, useRef, useState } from 'react';
import { copyClipboardText } from '../shared/clipboard.js';

/**
 * Consolidated copy + transient "copied" flash hook. reader / code viewer / workspace panel
 * previously each kept their own copied state + setTimeout reset and called navigator.clipboard directly
 * (no execCommand fallback — it failed silently in WebViews without the clipboard API).
 * @param {number} resetMs - how long `copied` stays set (sites used 1200/900/1600ms; pass per site)
 * @returns {[string, (key: string, text: string) => boolean]} [copiedKey, copy]
 *   copy(key, text): returns false without changing state when text is empty; otherwise routes through
 *   copyClipboardText (with fallback) and flashes `copiedKey` only when the clipboard write actually
 *   succeeds, resetting it to '' after resetMs. A newer attempt supersedes an in-flight one.
 */
export function useCopyFlash(resetMs = 1200) {
  const [copied, setCopied] = useState('');
  const timerRef = useRef(null);
  const attemptRef = useRef(0);
  useEffect(() => () => {
    if (timerRef.current) window.clearTimeout(timerRef.current);
  }, []);
  const copy = (key, text) => {
    if (text == null || text === '') return false;
    const attempt = ++attemptRef.current;
    Promise.resolve(copyClipboardText(text)).then((ok) => {
      if (!ok || attempt !== attemptRef.current) return;
      setCopied(key);
      if (timerRef.current) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(() => setCopied(''), resetMs);
    });
    return true;
  };
  return [copied, copy];
}
