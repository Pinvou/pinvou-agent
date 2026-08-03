export function runtimeNoticeMode(status) {
  if (!status) return 'checking';
  if (!status.bridge_ready) return 'bridge_unavailable';
  if (!status.installed) return 'install';
  if (!status.authenticated) return 'login';
  if (status.error) return 'error';
  return 'ready';
}

export function runtimeOperationFor(operations, agentId) {
  if (!agentId || !operations) return '';
  return operations[agentId] || '';
}

export function runtimeInstallInProgress(status, operation = '') {
  return Boolean(status?.installing || operation === 'install');
}

export function runtimeLoginInProgress(status, operation = '') {
  return Boolean(
    status?.login_in_progress
      || operation === 'login'
      || operation === 'switch-account',
  );
}
