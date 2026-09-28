/// Reads an image from the system clipboard and encodes it to PNG bytes;
/// returns `None` when the clipboard holds no image or the platform provides
/// no reader.
///
/// Only Linux provides this today: WebKitGTK does not carry image data in the
/// paste event (an image clipboard surfaces an empty `clipboardData`,
/// observed on 2.52.6), so composer pastes fall back to a native read. The
/// macOS/Windows WebViews carry image items natively in the paste event and
/// their stubs always return `None`. Callers must gate on the frontend
/// capability bit `pasteImageClipboardRead`, not probe this function's
/// availability.
pub fn read_clipboard_image(app: &tauri::AppHandle) -> Option<Vec<u8>> {
    super::super::platform::read_clipboard_image(app)
}
