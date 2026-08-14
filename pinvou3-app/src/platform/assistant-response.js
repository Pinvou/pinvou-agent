import { isWeb } from '../shared/platform.js';
import { invokeTauri } from './tauri/client.js';

function downloadInBrowser({ content, filename, mimeType }) {
  if (typeof document === 'undefined' || typeof Blob === 'undefined' || !globalThis.URL?.createObjectURL) {
    throw new Error('Browser download is unavailable');
  }
  const url = URL.createObjectURL(new Blob([content], { type: mimeType }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.style.display = 'none';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
  return true;
}

export async function saveAssistantResponseFile(file) {
  if (isWeb) return downloadInBrowser(file);
  return invokeTauri('export_assistant_response', {
    content: file.content,
    defaultName: file.filename,
    format: file.format,
  });
}

export async function shareAssistantResponseWithSystem({ title, text }) {
  const share = globalThis.navigator?.share;
  if (typeof share !== 'function') return 'unavailable';
  try {
    await share.call(globalThis.navigator, { title, text });
    return 'shared';
  } catch (error) {
    if (error?.name === 'AbortError') return 'cancelled';
    throw error;
  }
}

export async function openAssistantShareTarget(target) {
  if (isWeb) return false;
  await invokeTauri('open_assistant_share_target', { target });
  return true;
}
