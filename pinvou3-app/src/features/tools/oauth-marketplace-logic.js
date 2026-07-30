// 默认中文文案：UI 边界（ToolStoreView）会按当前语言传入 storeCopy.oauthOutcome；
// 缺省保持中文以兼容既有调用与测试。
const DEFAULT_OUTCOME_COPY = {
  connectedTitle: (name) => `已连接「${name}」`,
  timeoutTitle: (name) => `${name}授权超时`,
  cancelledTitle: (name) => `${name}授权已取消`,
  serviceErrorTitle: (name) => `${name}授权服务错误`,
  failedTitle: (name) => `${name}授权失败`,
  tokenMissing: '授权流程返回成功，但本地未检测到 OAuth token，当前不会启用该工具。请重新授权。',
  incomplete: '授权未完成，请重新连接。',
};

export function resolveOAuthInstallOutcome(toolName, loginResult, authStatus, copy = DEFAULT_OUTCOME_COPY) {
  const connected = !!authStatus?.oauth_token_present;
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
