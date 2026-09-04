import { useEffect, useRef, useState } from 'react';
import { copyClipboardText } from '../shared/clipboard.js';

/**
 * Consolidated copy + transient "copied" flash hook. reader / code viewer / workspace panel
 * previously each kept their own copied state + setTimeout reset and called navigator.clipboard directly
 * (no execCommand fallback — it failed silently in WebViews without the clipboard API).
 * @param {number} resetMs - how long `copied` stays set (sites used 1200/900/1600ms; pass per site)
 * @returns {[string, (key: string, text: string) => boolean]} [copiedKey, copy]
 *   copy(key, text): returns false without changing state when text is empty; otherwise routes through copyClipboardText
 *   (with fallback), sets copiedKey, and resets it to '' after resetMs.
 */
export function useCopyFlash(resetMs = 1200) {
  const [copied, setCopied] = useState('');
  const timerRef = useRef(null);
  useEffect(() => () => {
    if (timerRef.current) window.clearTimeout(timerRef.current);
  }, []);
  const copy = (key, text) => {
    if (text == null || text === '') return false;
    copyClipboardText(text);
    setCopied(key);
    if (timerRef.current) window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => setCopied(''), resetMs);
    return true;
  };
  return [copied, copy];
}
