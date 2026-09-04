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
 * @returns {Promise<{ bytes: number[], ext: string }>}
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
