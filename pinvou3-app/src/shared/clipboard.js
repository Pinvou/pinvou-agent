export function fallbackCopyText(text) {
  return new Promise((resolve) => {
    if (typeof document === 'undefined' || !document.body) {
      resolve(false);
      return;
    }
    let textarea = null;
    try {
      textarea = document.createElement('textarea');
      textarea.value = String(text || '');
      textarea.setAttribute('readonly', '');
      textarea.style.position = 'fixed';
      textarea.style.left = '-9999px';
      textarea.style.top = '-9999px';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.focus();
      textarea.select();
      textarea.setSelectionRange(0, textarea.value.length);
      resolve(Boolean(document.execCommand('copy')));
    } catch {
      resolve(false);
    } finally {
      if (textarea?.parentNode) textarea.parentNode.removeChild(textarea);
    }
  });
}

export function copyClipboardText(text) {
  const value = String(text || '');
  if (!value) return Promise.resolve(false);
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    return navigator.clipboard.writeText(value)
      .then(() => true)
      .catch(() => fallbackCopyText(value));
  }
  return fallbackCopyText(value);
}
