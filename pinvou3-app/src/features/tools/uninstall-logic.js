// Known runtime-release refusals have actionable, translated copy. Preserve
// existing filesystem/transaction details for all other uninstall failures.
export const uninstallErrorCopy = (error, copy) => {
  const message = String(error && error.message ? error.message : error);
  if (message === 'marketplace_uninstall_busy') return copy.uninstallBusy;
  if (message === 'marketplace_uninstall_release_failed') return copy.uninstallReleaseFailed;
  return message.slice(0, 240);
};
