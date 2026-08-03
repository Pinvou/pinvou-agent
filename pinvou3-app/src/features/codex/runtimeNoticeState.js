export function runtimeNoticeMode(status) {
  if (!status) return 'checking';
  if (!status.bridge_ready) return 'bridge_unavailable';
  if (!status.installed) return 'install';
  if (!status.authenticated) return 'login';
  if (status.error) return 'error';
  return 'ready';
}
