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
const codexViewSource = await readFile(
  new URL('../src/features/codex/CodexAcpView.jsx', import.meta.url),
  'utf8',
);
const dropHookSource = await readFile(
  new URL('../src/features/attachments/useAttachmentDrop.js', import.meta.url),
  'utf8',
);
const composerDropOverlaySource = await readFile(
  new URL('../src/features/attachments/ComposerAttachmentDropOverlay.jsx', import.meta.url),
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
const tauriChatBridgeSource = await readFile(
  new URL('../src/platform/tauri/bridge/chat.js', import.meta.url),
  'utf8',
);
const webDomainAdapterSource = await readFile(
  new URL('../src/platform/web/bridge/domain-adapter.js', import.meta.url),
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
assert.match(chatViewSource, /ComposerAttachmentDropOverlay/);
assert.match(codexViewSource, /ComposerAttachmentDropOverlay/);
assert.match(dropHookSource, /PinvouAttachmentDropController/);
assert.match(composerDropOverlaySource, /useAttachmentDrop\(\{ enabled, onFiles \}\)/);
assert.doesNotMatch(chatViewSource, /bs\.attachmentDragActive/);
assert.equal(
  JSON.parse(tauriConfigSource).app.windows[0].dragDropEnabled,
  false,
  'Windows WebView2 default drag feedback requires the Tauri file-drop interceptor to be disabled',
);
assert.match(dropControllerSource, /dataTransfer\.dropEffect = "copy"/);
assert.match(tauriAttachmentBridgeSource, /ingest_draft_file_chunk/);
assert.match(tauriAttachmentBridgeSource, /adopt_draft_attachment/);
assert.match(tauriAttachmentBridgeSource, /cancel_draft_file_upload/);
assert.match(
  tauriChatBridgeSource,
  /function abandonPreparedAttachments\(\)[\s\S]*?discardManagedAttachment\(attachment\.result\)/,
  'switching sessions during draft adoption must release attachments already moved into the old session',
);
assert.match(
  tauriChatBridgeSource,
  /if \(state\.activeSessionId !== sid\) \{\s*abandonPreparedAttachments\(\);\s*return;/,
  'an attachment send interrupted by navigation must not leave a stale chip for the next session',
);
assert.match(desktopUploadSource, /workspace\.join\("attachments"\)/);
assert.match(desktopUploadSource, /draft_attachment_workspace/);
assert.match(desktopUploadSource, /adopt_draft_upload/);
assert.doesNotMatch(
  tauriAttachmentBridgeSource,
  /onDragDropEvent/,
  'desktop must not reinstall the native Tauri handler that blocks WebView2 drag feedback',
);
assert.doesNotMatch(
  tauriAttachmentBridgeSource,
  /PinvouAttachmentDropController\.install/,
  'the platform bridge must not own drag routing globally',
);
for (const [name, bridgeSource] of [
  ['Tauri', tauriBridgeSource],
  ['Web', webDomainAdapterSource],
]) {
  assert.match(
    bridgeSource,
    /chat: \[[^\]]*"attachmentDragActive"/,
    `${name} chat state slice must publish attachmentDragActive to React`,
  );
}

console.log('attachment drop overlay contract tests passed');
