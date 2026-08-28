// Navigation IPC acknowledges dispatch, not a completed page load. Publish the
// optimistic address before dispatch so a fast Finished event can replace it;
// never write the requested URL again after the command resolves.
export async function dispatchBrowserNavigation({ target, dispatch, publishInput }) {
  if (typeof dispatch !== 'function') throw new TypeError('dispatch must be a function');
  if (typeof publishInput !== 'function') throw new TypeError('publishInput must be a function');
  publishInput(target);
  await dispatch();
}
