import React, { useEffect, useState } from 'react';
import { AlertTriangle, RefreshCw, Sparkles, Terminal } from '../../components/icons.jsx';
import {
  runtimeInstallInProgress,
  runtimeLoginInProgress,
  runtimeNoticeMode,
} from './runtimeNoticeState.js';

function setupHintText(copy, hint) {
  return copy.setupHints?.[hint] || '';
}

export function RuntimeNotice({
  status,
  working,
  managementAvailable,
  operation,
  error,
  onInstall,
  onLogin,
  onOpenLogin,
  onSubmitLoginCode,
  onRefresh,
  resetKey,
  suppressAdvisoryUpgrade = false,
  copy,
}) {
  const [authorizationCode, setAuthorizationCode] = useState('');
  const [declinedUpgrade, setDeclinedUpgrade] = useState(false);
  useEffect(() => {
    setAuthorizationCode('');
  }, [status?.agent_id, status?.login_in_progress]);
  useEffect(() => {
    setDeclinedUpgrade(false);
  }, [resetKey, status?.agent_id, status?.installed, status?.latest_version]);
  const noticeMode = runtimeNoticeMode(status, declinedUpgrade || suppressAdvisoryUpgrade);
  if (noticeMode === 'checking') return <div className="text-[13px] text-gray-400">{copy.checking}</div>;
  const rawError = error || status.error;
  const visibleError = rawError
    ? (copy.showRawErrors ? rawError : copy.operationFailed)
    : '';
  if (noticeMode === 'bridge_unavailable') {
    const isCodex = status.agent_id === 'codex';
    return (
      <div className="rounded-2xl border border-red-500/20 bg-red-500/[0.05] p-4 flex items-start gap-3">
        <AlertTriangle size={19} className="text-red-500 shrink-0 mt-0.5" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{isCodex ? copy.bridgeUnavailable : copy.setupRequired}</div>
          <div className="mt-1 text-[12px] text-gray-500">{setupHintText(copy, status.setup_hint) || copy.bridgeRepair}</div>
          {visibleError && <div className="mt-2 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {!isCodex && (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-red-500/20 text-[12px] font-medium">
            {copy.recheck}
          </button>
        )}
      </div>
    );
  }
  if (noticeMode === 'install') {
    const agentName = status.agent_name || 'Agent';
    const action = status.install_action || 'manual';
    const isPackageManagerUpgrade = action === 'brew_upgrade' || action === 'npm_upgrade';
    const canAutoUpgrade = (status.update_available || status.update_required || isPackageManagerUpgrade)
      && action !== 'manual'
      && action !== 'none';
    const canDeferUpgrade = status.update_available && status.installed && !status.update_required;
    const installing = runtimeInstallInProgress(status, operation);
    const installHints = {
      official_script: copy.officialScriptHint(agentName),
    };
    const installButtons = {
      official_script: copy.confirmInstall,
    };
    const hint = !managementAvailable
      ? copy.manageAgentOnDesktop(agentName)
      : isPackageManagerUpgrade
        ? copy.packageManagerUpgradeHint(status.install_source)
        : installHints[action] || setupHintText(copy, status.setup_hint) || copy.manualInstallHint(agentName);
    const busyLabel = copy.installing;
    return (
      <div className="rounded-2xl border border-blue-500/20 bg-blue-500/[0.05] p-4 flex items-center gap-3">
        <Terminal size={19} className="text-blue-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">
            {status.update_required
              ? copy.cliUpdateRequired(agentName, status.version, status.latest_version)
              : status.update_available
                ? copy.cliUpdateAvailable(agentName, status.version, status.latest_version)
                : status.version
                  ? copy.cliOutdated(status.version, status.min_version)
                  : copy.cliMissing(agentName)}
          </div>
          <div className="mt-0.5 text-[12px] text-gray-500">{hint}</div>
          {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {!managementAvailable ? (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-blue-500/20 text-[12px] font-medium">
            {copy.recheck}
          </button>
        ) : canAutoUpgrade ? (
          <div className="flex shrink-0 items-center gap-2">
            {canDeferUpgrade && (
              <button onClick={() => setDeclinedUpgrade(true)} disabled={working || installing} className="px-3 py-1.5 rounded-xl border border-blue-500/20 text-[12px] font-medium disabled:opacity-50">
                {copy.declineUpgrade}
              </button>
            )}
            <button onClick={() => onInstall()} disabled={working || installing} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50 inline-flex items-center gap-1.5">
              {installing && <RefreshCw size={12} className="animate-spin" />}
              {installing ? busyLabel : copy.upgrade}
            </button>
          </div>
        ) : installButtons[action] ? (
          <button onClick={() => onInstall()} disabled={working || installing} className="px-3 py-1.5 rounded-xl bg-blue-600 text-white text-[12px] font-medium disabled:opacity-50 inline-flex items-center gap-1.5">
            {installing && <RefreshCw size={12} className="animate-spin" />}
            {installing ? busyLabel : installButtons[action]}
          </button>
        ) : (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-blue-500/20 text-[12px] font-medium">
            {copy.recheck}
          </button>
        )}
      </div>
    );
  }
  if (noticeMode === 'login') {
    const waitingForLogin = runtimeLoginInProgress(status, operation);
    const loginUrlReady = waitingForLogin && Boolean(status.login_url);
    const agentName = status.agent_name || 'Agent';
    const waitingTitle = copy.waitingAgentLogin
      ? copy.waitingAgentLogin(agentName)
      : copy.waitingLogin;
    const signedOutTitle = copy.agentNotLoggedIn
      ? copy.agentNotLoggedIn(agentName)
      : copy.notLoggedIn;
    const loginHint = !managementAvailable
      ? copy.manageAgentOnDesktop(agentName)
      : copy.agentLoginHint
        ? copy.agentLoginHint(agentName)
        : (setupHintText(copy, status.setup_hint) || copy.loginHint);
    return (
      <div className="rounded-2xl border border-amber-500/20 bg-amber-500/[0.06] p-4 flex items-start gap-3">
        <Sparkles size={19} className="text-amber-500 shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold">{waitingForLogin ? waitingTitle : signedOutTitle}</div>
          <div className="text-[12px] text-gray-500">
            {loginUrlReady
              ? (copy.finishAgentAuth ? copy.finishAgentAuth(agentName) : copy.finishBrowserAuth)
              : waitingForLogin
                ? copy.openingAuth
                : loginHint}
          </div>
          {managementAvailable && status.login_code && (
            <div className="mt-2 inline-flex rounded-lg border border-amber-500/25 bg-white/70 px-2.5 py-1 font-mono text-[13px] font-semibold tracking-wider text-amber-800 dark:bg-black/20 dark:text-amber-200">
              {copy.deviceCode ? copy.deviceCode(status.login_code) : status.login_code}
            </div>
          )}
          {managementAvailable && waitingForLogin && status.login_input_required && (
            <form
              className="mt-2 flex max-w-md items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                const code = authorizationCode.trim();
                if (code) onSubmitLoginCode(code);
              }}
            >
              <input
                value={authorizationCode}
                onChange={event => setAuthorizationCode(event.target.value)}
                placeholder={copy.authorizationCodePlaceholder}
                aria-label={copy.authorizationCodePlaceholder}
                autoComplete="off"
                className="min-w-0 flex-1 rounded-lg border border-amber-500/25 bg-white/80 px-2.5 py-1.5 text-[12px] outline-none focus:border-amber-500 dark:bg-black/20"
              />
              <button
                type="submit"
                disabled={!authorizationCode.trim()}
                className="rounded-lg border border-amber-500/30 px-2.5 py-1.5 text-[12px] font-medium text-amber-700 disabled:opacity-40 dark:text-amber-300"
              >
                {copy.submitAuthorizationCode}
              </button>
            </form>
          )}
          {visibleError && <div className="mt-1 text-[11px] text-red-500">{visibleError}</div>}
        </div>
        {managementAvailable ? (
          <>
            {loginUrlReady && (
              <button onClick={onOpenLogin} className="px-3 py-1.5 rounded-xl border border-amber-500/30 text-amber-700 dark:text-amber-300 text-[12px] font-medium">
                {copy.reopenAuth}
              </button>
            )}
            <button onClick={onLogin} disabled={working || waitingForLogin} className="px-3 py-1.5 rounded-xl bg-amber-500 text-white text-[12px] font-medium disabled:opacity-50">
              {waitingForLogin ? copy.waitAuth : copy.authorize}
            </button>
          </>
        ) : (
          <button onClick={onRefresh} className="px-3 py-1.5 rounded-xl border border-amber-500/30 text-amber-700 dark:text-amber-300 text-[12px] font-medium">
            {copy.recheck}
          </button>
        )}
      </div>
    );
  }
  if (noticeMode === 'error') return <div className="rounded-xl bg-red-500/8 text-red-600 dark:text-red-300 px-3 py-2 text-[12px]">{visibleError}</div>;
  return null;
}

export function runtimeSourceLabel(status, copy) {
  if (!status) return '';
  return copy?.runtimeSources?.[status.runtime_source] || '';
}

export function AgentServiceFailureNotice({
  failure,
  agentName,
  working,
  managementAvailable,
  onSwitchAccount,
  onManageProviders,
  onDismiss,
  copy,
  providerCopy,
}) {
  if (!failure) return null;
  const recoverWithAccount = managementAvailable
    && ['entitlement', 'quota', 'authentication'].includes(failure.kind);
  const title = failure.kind === 'entitlement'
    ? copy.entitlementUnavailable(agentName)
    : failure.kind === 'quota'
      ? copy.quotaUnavailable(agentName)
      : failure.kind === 'authentication'
        ? copy.authorizationExpired(agentName)
        : copy.serviceUnavailable(agentName);
  const description = recoverWithAccount
    ? copy.accountRecoveryHint
    : copy.serviceRecoveryHint;
  return (
    <div data-testid="acp-service-failure" className="rounded-2xl border border-red-500/20 bg-red-500/[0.055] p-4">
      <div className="flex items-start gap-3">
        <AlertTriangle size={19} className="mt-0.5 shrink-0 text-red-500" />
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold text-red-700 dark:text-red-300">{title}</div>
          <div className="mt-1 text-[12px] leading-5 text-gray-500 dark:text-gray-400">{description}</div>
          <details className="mt-2">
            <summary className="cursor-pointer text-[11px] text-gray-400">{copy.errorDetails}</summary>
            <div className="mt-1 break-words text-[11px] text-red-500">{failure.detail}</div>
          </details>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {recoverWithAccount && (
            <button
              type="button"
              onClick={onSwitchAccount}
              disabled={working}
              className="rounded-xl bg-red-500 px-3 py-1.5 text-[12px] font-medium text-white disabled:opacity-50"
            >
              {copy.switchAccount}
            </button>
          )}
          {onManageProviders && providerCopy && (
            <button
              type="button"
              data-testid="acp-failure-manage-providers"
              onClick={onManageProviders}
              disabled={working}
              className="rounded-xl border border-red-500/20 px-3 py-1.5 text-[12px] font-medium text-red-600 disabled:opacity-50 dark:text-red-300"
            >
              {providerCopy.faultManage}
            </button>
          )}
          <button
            type="button"
            onClick={onDismiss}
            disabled={working}
            className="rounded-xl border border-red-500/20 px-3 py-1.5 text-[12px] font-medium text-red-600 disabled:opacity-50 dark:text-red-300"
          >
            {copy.dismissNotice}
          </button>
        </div>
      </div>
    </div>
  );
}
