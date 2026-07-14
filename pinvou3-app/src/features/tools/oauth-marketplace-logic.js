export function resolveOAuthInstallOutcome(toolName, loginResult, authStatus) {
  const connected = loginResult?.status === 'connected' && !!authStatus?.oauth_token_present;
  if (connected) {
    return {
      connected: true,
      authState: authStatus,
      selectedToolPatch: {
        installed: true,
        authStatus: 'connected',
        authMessage: authStatus?.message || '',
      },
      alert: {
        visible: true,
        loading: false,
        title: `已连接「${toolName}」`,
        isInstall: true,
        isError: false,
      },
    };
  }

  const status = loginResult?.status === 'connected' ? 'auth_failed' : (loginResult?.status || 'failed');
  const title = status === 'timeout'
    ? `${toolName}授权超时`
    : status === 'service_error'
      ? `${toolName}授权服务错误`
      : `${toolName}授权失败`;
  const message = loginResult?.status === 'connected'
    ? '授权流程返回成功，但本地未检测到 OAuth token，当前不会启用该工具。请重新授权。'
    : (loginResult?.message || '授权未完成，请重新连接。');

  return {
    connected: false,
    authState: {
      ...(authStatus || {}),
      installed: true,
      mcp_configured: authStatus?.mcp_configured ?? true,
      oauth_required: true,
      oauth_token_present: false,
      status,
      message,
    },
    selectedToolPatch: {
      installed: false,
      authStatus: status,
      authMessage: message,
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
