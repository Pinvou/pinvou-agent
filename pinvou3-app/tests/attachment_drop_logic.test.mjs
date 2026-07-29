import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

const overlaySource = await readFile(
  new URL('../src/features/attachments/AttachmentDropOverlay.jsx', import.meta.url),
  'utf8',
);
const chatViewSource = await readFile(
  new URL('../src/features/chat/ChatView.jsx', import.meta.url),
  'utf8',
);
const tauriBridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge.js', import.meta.url),
  'utf8',
);
const tauriAttachmentBridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge/artifacts.js', import.meta.url),
  'utf8',
);
const webBridgeSource = await readFile(
  new URL('../src/platform/web/bridge.js', import.meta.url),
  'utf8',
);
const tauriConfigSource = await readFile(
  new URL('../src-tauri/tauri.conf.json', import.meta.url),
  'utf8',
);
const desktopUploadSource = await readFile(
  new URL('../src-tauri/src/features/files/attachment_upload.rs', import.meta.url),
  'utf8',
);
const dropControllerSource = await readFile(
  new URL('../src/features/attachments/attachment-drop-controller.js', import.meta.url),
  'utf8',
);
assert.doesNotMatch(
  overlaySource,
  /desktop-dragged-file-icon|DesktopDraggedFileIcon/,
  'desktop must preserve the native operating-system drag image',
);
assert.match(overlaySource, /data-variant="desktop"/);
assert.match(overlaySource, /data-variant="web"/);
assert.match(
  overlaySource,
  /createPortal\(overlay, document\.body\)/,
  'Web drop feedback must escape ChatView stacking contexts and cover the viewport',
);
assert.match(chatViewSource, /attachmentDragActive = !!\(bs && bs\.attachmentDragActive\)/);
assert.doesNotMatch(chatViewSource, /useAttachmentDropOverlay/);
assert.equal(
  JSON.parse(tauriConfigSource).app.windows[0].dragDropEnabled,
  false,
  'Windows WebView2 default drag feedback requires the Tauri file-drop interceptor to be disabled',
);
assert.match(dropControllerSource, /dataTransfer\.dropEffect = "copy"/);
assert.match(tauriAttachmentBridgeSource, /ingest_dropped_file_chunk/);
assert.match(tauriAttachmentBridgeSource, /sessionId: sessionId/);
assert.match(tauriAttachmentBridgeSource, /cancel_dropped_file_upload/);
assert.match(desktopUploadSource, /workspace\.join\("attachments"\)/);
assert.doesNotMatch(
  desktopUploadSource,
  /dropped-attachments/,
  'desktop drops must be owned by the session workspace instead of a global permanent cache',
);
assert.doesNotMatch(
  tauriAttachmentBridgeSource,
  /onDragDropEvent/,
  'desktop must not reinstall the native Tauri handler that blocks WebView2 drag feedback',
);
for (const [name, bridgeSource] of [
  ['Tauri', tauriBridgeSource],
  ['Web', webBridgeSource],
]) {
  assert.match(
    bridgeSource,
    /chat: \[[^\]]*"attachmentDragActive"/,
    `${name} chat state slice must publish attachmentDragActive to React`,
  );
}

console.log('attachment drop overlay contract tests passed');
