function sessionRoute(chat) {
  if (chat && chat.taskKind === 'codex') return 'codex';
  if (chat && chat.scheduledRun) return 'scheduled';
  return 'chat';
}

async function runSessionBatch(items, action, handlers) {
  const rows = Array.isArray(items) ? items.filter(item => item && item.id) : [];
  const operations = rows.map(item => async () => {
    if (action === 'archive' && sessionRoute(item) === 'codex') {
      if (typeof handlers.archiveCodex !== 'function') return false;
      return handlers.archiveCodex(item.id);
    }
    const handler = handlers && handlers[action];
    if (typeof handler !== 'function') return false;
    return handler(item.id);
  });
  const settled = await Promise.allSettled(operations.map(operation => operation()));
  const failed = settled.filter(result => result.status === 'rejected' || result.value === false).length;
  return {
    total: settled.length,
    succeeded: settled.length - failed,
    failed,
  };
}

export { runSessionBatch, sessionRoute };
