import '../../shared/chunked-file-upload.js';

const { MAX_FILE_BYTES: ATTACHMENT_MAX_FILE_BYTES } = globalThis.PinvouChunkedFileUpload;

// The localized fileTooLarge copy takes the MB figure as an argument (same
// shape as the skill-store zipTooLarge copy) so the "20 MB" users see cannot
// drift from the cap it describes — the browser preflight mirror of
// file_ingest::MAX_FILE_BYTES, locked as a literal by the web_access contract
// test. Derive it once here and inject it at render time.
const MAX_FILE_MB = Math.round(ATTACHMENT_MAX_FILE_BYTES / (1024 * 1024));

export const ATTACHMENT_LIMIT_ERROR_CODES = Object.freeze({
  fileTooLarge: 'attachment_file_too_large',
  uploadCacheFull: 'web_attachment_upload_cache_full',
  parsedCacheTooLarge: 'web_attachment_parsed_cache_too_large',
  attachmentCacheFull: 'web_attachment_cache_full',
  archiveTooManyEntries: 'attachment_archive_too_many_entries',
  archiveExpandedTooLarge: 'attachment_archive_expanded_too_large',
  archiveUnsafeEntry: 'attachment_archive_unsafe_entry',
});

/** @typedef {{ code?: unknown, message?: unknown }} AttachmentLimitErrorLike */
/** @typedef {{ fileTooLarge?: (mb: number) => string, deviceUploadCacheFull?: string, deviceParsedTooLarge?: string, deviceCacheFull?: string, archiveTooManyEntries?: string, archiveExpandedTooLarge?: string, archiveUnsafeEntry?: string }} AttachmentLimitCopy */

/**
 * Normalize browser-side size failures and backend attachment rejection codes.
 * @param {unknown} error - Rejected invoke value or browser upload error.
 * @returns {string} Stable attachment limit code, or an empty string.
 */
export function attachmentLimitErrorCode(error) {
  const errorObject = error && typeof error === 'object'
    ? /** @type {AttachmentLimitErrorLike} */ (error)
    : null;
  if (errorObject?.code === 'device_upload_too_large') {
    return ATTACHMENT_LIMIT_ERROR_CODES.fileTooLarge;
  }
  const raw = String(errorObject && 'message' in errorObject
    ? errorObject.message
    : error || '').trim();
  const codes = /** @type {readonly string[]} */ (Object.values(ATTACHMENT_LIMIT_ERROR_CODES));
  return codes.includes(raw) ? raw : '';
}

/**
 * Render a stable limit rejection with the active UI language.
 * @param {unknown} error - Rejected invoke value or browser upload error.
 * @param {AttachmentLimitCopy} copy - Current language's uiAttachments dictionary.
 * @returns {string} Localized limit message, or an empty string.
 */
export function formatAttachmentLimitError(error, copy = {}) {
  switch (attachmentLimitErrorCode(error)) {
    case ATTACHMENT_LIMIT_ERROR_CODES.fileTooLarge:
      // Tolerate a missing copy object (the `copy = {}` default) the way the
      // old `|| ''` fallback did instead of throwing inside an error renderer.
      return typeof copy.fileTooLarge === 'function' ? copy.fileTooLarge(MAX_FILE_MB) : '';
    case ATTACHMENT_LIMIT_ERROR_CODES.uploadCacheFull:
      return copy.deviceUploadCacheFull || '';
    case ATTACHMENT_LIMIT_ERROR_CODES.parsedCacheTooLarge:
      return copy.deviceParsedTooLarge || '';
    case ATTACHMENT_LIMIT_ERROR_CODES.attachmentCacheFull:
      return copy.deviceCacheFull || '';
    case ATTACHMENT_LIMIT_ERROR_CODES.archiveTooManyEntries:
      return copy.archiveTooManyEntries || '';
    case ATTACHMENT_LIMIT_ERROR_CODES.archiveExpandedTooLarge:
      return copy.archiveExpandedTooLarge || '';
    case ATTACHMENT_LIMIT_ERROR_CODES.archiveUnsafeEntry:
      return copy.archiveUnsafeEntry || '';
    default:
      return '';
  }
}
