// copy 文案由 UI 边界(ToolStoreView)按当前语言传入 storeCopy.oauthOutcome,必填。
export function resolveOAuthInstallOutcome(toolName, loginResult, authStatus, copy) {
  const connected = !!authStatus?.oauth_token_present;
  if (connected) {
    return {
      connected: true,
      authState: authStatus,
      selectedToolPatch: {
        installed: true,
        authStatus: 'connected',
      },
      alert: {
        visible: true,
        loading: false,
        title: copy.connectedTitle(toolName),
        isInstall: true,
        isError: false,
      },
    };
  }

  const status = loginResult?.status === 'connected' ? 'auth_failed' : (loginResult?.status || 'failed');
  const title = status === 'timeout'
    ? copy.timeoutTitle(toolName)
    : status === 'cancelled'
      ? copy.cancelledTitle(toolName)
    : status === 'service_error'
      ? copy.serviceErrorTitle(toolName)
      : copy.failedTitle(toolName);
  const message = loginResult?.status === 'connected'
    ? copy.tokenMissing
    : (loginResult?.message || copy.incomplete);

  return {
    connected: false,
    authState: {
      ...authStatus,
      installed: true,
      oauth_token_present: false,
      status,
      message,
    },
    selectedToolPatch: {
      installed: false,
      authStatus: status,
    },
    alert: {
      visible: true,
      loading: false,
      title,
      subtitle: message,
      isInstall: false,
      isError: true,
    },
  };
}
