export const ATTACHMENT_LIMIT_ERROR_CODES = Object.freeze({
  fileTooLarge: 'attachment_file_too_large',
  archiveTooManyEntries: 'attachment_archive_too_many_entries',
  archiveExpandedTooLarge: 'attachment_archive_expanded_too_large',
  archiveUnsafeEntry: 'attachment_archive_unsafe_entry',
});

/** @typedef {{ code?: unknown, message?: unknown }} AttachmentLimitErrorLike */
/** @typedef {{ fileTooLarge?: string, archiveTooManyEntries?: string, archiveExpandedTooLarge?: string, archiveUnsafeEntry?: string }} AttachmentLimitCopy */

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
      return copy.fileTooLarge || '';
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
