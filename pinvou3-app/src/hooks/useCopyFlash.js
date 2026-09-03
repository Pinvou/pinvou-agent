import { useEffect, useRef, useState } from 'react';
import { copyClipboardText } from '../shared/clipboard.js';

/**
 * 复制 + 「已复制」短暂反馈的收敛 hook。此前 reader / code viewer / workspace 面板
 * 各自维护 copied 状态 + setTimeout 复位，且直连 navigator.clipboard（无
 * execCommand 回退，在不支持剪贴板 API 的 WebView 里静默失败）。
 * @param {number} resetMs - copied 保持时长（各站点原有 1200/900/1600ms，按站点传）
 * @returns {[string, (key: string, text: string) => boolean]} [copiedKey, copy]
 *   copy(key, text)：text 为空返回 false 且不改变状态；否则走 copyClipboardText
 *   （含回退）并置 copiedKey，resetMs 后复位为 ''。
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
