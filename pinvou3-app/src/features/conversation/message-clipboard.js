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

export function readClipboardText() {
  if (typeof navigator !== 'undefined' && navigator.clipboard?.readText) {
    return navigator.clipboard.readText().catch(() => '');
  }
  return Promise.resolve('');
}

export function normalizeAssistantMessageText(value) {
  return String(value || '')
    .replace(/\u00a0/g, ' ')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

export function assistantMessageText(target) {
  if (!target) return '';
  const rendered = typeof target.innerText === 'string' ? target.innerText : target.textContent;
  return normalizeAssistantMessageText(rendered);
}

export function assistantResponseText(turn) {
  if (!turn) return '';
  const items = Array.isArray(turn.items) && turn.items.length
    ? turn.items
    : Array.isArray(turn.presentation)
      ? turn.presentation
      : [];
  const messages = items
    .filter(item => item?.type === 'agent_message' && item.phase !== 'commentary')
    .map(item => normalizeAssistantMessageText(item.text))
    .filter(Boolean);
  return normalizeAssistantMessageText(messages.length ? messages.join('\n\n') : turn.assistantText);
}
