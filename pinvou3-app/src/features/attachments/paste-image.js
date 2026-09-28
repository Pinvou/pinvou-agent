// Common front half of paste-image attachments: filter image files out of the clipboard event + read them as bytes via FileReader.
// ChatView (bridge addPasteImage) and CodexAcpView (save_paste_image / direct device upload) previously
// each inlined the same WebKit-compatible filter+read, and the jpeg->jpg extension normalization only existed on the codex side;
// now unified (chat-side image/jpeg pastes normalize the stored name from .jpeg to .jpg).

/**
 * Returns the image Files extracted from a paste event. Does not call preventDefault — whether to consume
 * the event (and letting it pass when no channel is available) is the caller's decision.
 * WebKit's DataTransferItemList has no Symbol.iterator, so for...of/spread throw
 * TypeError; always use Array.from (all Safari/WKWebView versions).
 */
export function collectClipboardImages(event) {
  // eslint-disable-next-line unicorn/prefer-spread -- DataTransferItemList is not iterable on any Safari/WKWebView version
  const items = Array.from((event.clipboardData && event.clipboardData.items) || []);
  return items
    .filter((item) => item.type && item.type.startsWith('image/'))
    .map((item) => item.getAsFile())
    .filter(Boolean);
}

/**
 * Reads into a byte array via FileReader (Safari 14 lacks Blob#arrayBuffer, so the paste bridge path keeps
 * the FileReader approach) and derives the extension; jpeg normalizes to jpg.
 * @returns {Promise<{ bytes: number[], ext: string }>} Resolves with the file bytes and the normalized extension.
 */
export function readPasteImageAsBytes(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      resolve({
        bytes: [...new Uint8Array(reader.result)],
        ext: (file.type.split('/')[1] || 'png').replace('jpeg', 'jpg'),
      });
    };
    reader.onerror = () => reject(reader.error || new Error('read paste image failed'));
    reader.readAsArrayBuffer(file);
  });
}

/**
 * True when the paste event carries no data at all (no image items, empty types) —
 * the signature WebKitGTK (Linux) produces for an image clipboard, verified against
 * 2.52.6: image-only and image+text clipboards both surface an empty clipboardData
 * (the text part is swallowed too). Windows (WebView2) and macOS (WKWebView) always
 * list the image among the items, and text pastes expose text/plain, so those
 * events never match and keep the default paste path.
 */
export function pasteEventNeedsClipboardFallback(event) {
  const data = event && event.clipboardData;
  if (!data) return false;
  if (collectClipboardImages(event).length) return false;
  const types = data.types;
  return !types || types.length === 0;
}

/**
 * The native clipboard-image fallback exists on Linux only (see the platform
 * capability `paste_image_clipboard_read`); the flag reaches the frontend through
 * get_platform_capabilities as `pasteImageClipboardRead` on the bridge's
 * platformCapabilities slice. `loaded` guards the pre-startup window: a paste
 * before capabilities arrive keeps the previous "nothing pasted" behavior instead
 * of a command roundtrip that other platforms cannot answer. Note the conservative
 * failure mode: if the capability load itself fails, `loaded` never turns true and
 * the fallback stays off for the whole session (the bridge logs the failure).
 */
export function pasteImageClipboardFallbackAvailable(platformCapabilities) {
  return !!(platformCapabilities
    && platformCapabilities.loaded
    && platformCapabilities.pasteImageClipboardRead === true);
}
