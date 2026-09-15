export function isPendingAcpAttachment(attachment) {
  return ['parsing', 'uploading'].includes(attachment?.status);
}

export function cancelPendingAcpAttachments(attachments, cancelledIds) {
  for (const attachment of attachments || []) {
    if (isPendingAcpAttachment(attachment)) cancelledIds.add(attachment.id);
  }
}

export async function runAcpAttachmentTask({
  id,
  cancelledIds,
  load,
  discard,
  onReady,
  onError,
}) {
  try {
    const result = await load();
    if (cancelledIds.has(id)) {
      await Promise.resolve(discard(result)).catch(() => {});
      return false;
    }
    onReady(result);
    return true;
  } catch (error) {
    if (!cancelledIds.has(id)) onError(error);
    return false;
  } finally {
    cancelledIds.delete(id);
  }
}
