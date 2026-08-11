import React from 'react';
import { FileTypeIcon } from '../../components/files/FileTypeIcon.jsx';

export function SessionAttachmentTitle({ presentation }) {
  const text = presentation?.text || '';
  const attachments = presentation?.attachments || [];
  const firstAttachment = attachments[0];
  if (!firstAttachment) return text;

  return (
    <span className="flex min-w-0 items-center gap-1 overflow-hidden">
      {text && <span className="max-w-[45%] shrink truncate">{text}</span>}
      <span className="flex min-w-0 flex-1 items-center gap-1">
        <FileTypeIcon name={firstAttachment} className="h-4 w-4 shrink-0" />
        <span className="truncate">{firstAttachment}</span>
      </span>
      {attachments.length > 1 && (
        <span className="shrink-0 opacity-60">+{attachments.length - 1}</span>
      )}
    </span>
  );
}
