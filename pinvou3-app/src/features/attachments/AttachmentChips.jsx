import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';

/**
 * Uploaded / parsing attachment chip record as produced by the chat and ACP
 * views (`id` + `basename` + lifecycle `status`; `progress`/`error` arrive
 * with the matching upload events).
 * @typedef {object} AttachmentChip
 * @property {string} id - Stable attachment id used for removal.
 * @property {string} basename - File name shown on the chip.
 * @property {string} status - Lifecycle: uploading | parsing | ready | error.
 * @property {number} [progress] - Upload progress percentage.
 * @property {string | null} [error] - Error message for failed attachments.
 */

/** @type {AttachmentChip[]} */
const EMPTY_ATTACHMENTS = [];
const EMPTY_LABEL = () => '';

export function AttachmentChips({
  attachments = EMPTY_ATTACHMENTS,
  onRemove,
  dark = false,
  parsingLabel = '',
  uploadingLabel = EMPTY_LABEL,
  failedLabel = '',
  removeLabel = EMPTY_LABEL,
  formatError = EMPTY_LABEL,
  className = '',
}) {
  if (!attachments.length) return null;
  return (
    <div className={`flex flex-wrap gap-2 ${className}`}>
      {attachments.map(attachment => {
        const friendlyError = attachment.status === 'error'
          ? formatError(attachment.error)
          : '';
        return (
          <div
            key={attachment.id}
            className={`flex items-center gap-1.5 pl-1.5 pr-1.5 py-1 rounded-full text-[12px] ${
              dark ? 'bg-[#1E1F20] text-[#E3E3E3]' : 'bg-white text-[#1F1F1F] shadow-sm'
            }`}
          >
            <span className={`inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-[6px] ${dark ? 'bg-white/[0.08]' : 'bg-black/[0.04]'}`}>
              <FileTypeIcon name={attachment.basename} className="h-4 w-4" />
            </span>
            <span className="max-w-[160px] truncate" title={attachment.basename}>
              {attachment.basename}
            </span>
            <span className={
              attachment.status === 'error'
                ? 'text-[#F28B82]'
                : attachment.status === 'parsing' || attachment.status === 'uploading'
                  ? 'opacity-60'
                  : 'text-[#93D5A6]'
            }>
              {attachment.status === 'uploading'
                ? uploadingLabel(attachment.progress || 0)
                : attachment.status === 'parsing'
                  ? parsingLabel
                  : attachment.status === 'error'
                    ? failedLabel
                    : '✓'}
            </span>
            {friendlyError && (
              <span
                title={friendlyError}
                className="min-w-0 max-w-[min(520px,calc(100vw-240px))] truncate text-[#F28B82] opacity-90"
              >
                ：{friendlyError}
              </span>
            )}
            {onRemove && (
              <button
                type="button"
                onClick={() => onRemove(attachment.id)}
                aria-label={removeLabel(attachment.basename)}
                className={`w-5 h-5 rounded-full flex items-center justify-center ${
                  dark ? 'hover:bg-[#333537]' : 'hover:bg-[#F0F4F9]'
                }`}
              >
                ×
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
