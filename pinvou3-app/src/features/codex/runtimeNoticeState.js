export function runtimeNoticeMode(status, latestUpgradeDeferred = false) {
  if (!status) return 'checking';
  if (!status.bridge_ready) return 'bridge_unavailable';
  if (!status.installed || status.update_required || (status.update_available && !latestUpgradeDeferred)) return 'install';
  if (!status.authenticated) return 'login';
  if (status.error) return 'error';
  return 'ready';
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

const ACP_AUTHENTICATION_FAILURE = /HTTP\s*401|authentication[_ ]failed|authentication required|failed to authenticate|oauth.{0,80}expired|not logged in|model\.not_configured|llm not set|send\s+["']?\/login|尚未完成模型配置|请重新登录/i;

export function isAcpAuthenticationFailure(envelope) {
  if (envelope?.event?.type !== 'turn_completed') return false;
  const error = String(envelope.event?.data?.error || '');
  return ACP_AUTHENTICATION_FAILURE.test(error);
}

export function classifyAcpServiceFailure(envelope) {
  if (envelope?.event?.type !== 'turn_completed') return null;
  const detail = String(envelope.event?.data?.error || '').trim();
  if (!detail) return null;
  let kind = 'service';
  if (/HTTP\s*402|会员.{0,12}(权益|额度|到期|失效)|订阅.{0,12}(到期|失效)|payment required/i.test(detail)) {
    kind = 'entitlement';
  } else if (/HTTP\s*429|rate.?limit|quota|额度.{0,12}(不足|用尽)|用量.{0,12}(超出|耗尽)/i.test(detail)) {
    kind = 'quota';
  } else if (ACP_AUTHENTICATION_FAILURE.test(detail)) {
    kind = 'authentication';
  } else if (/network|connection|timeout|timed out|网络|连接.{0,8}(失败|超时)/i.test(detail)) {
    kind = 'network';
  }
  return {
    kind,
    detail,
    key: `${envelope.seq || ''}:${envelope.timestamp || ''}:${detail}`,
  };
}

// Agent-side runtime notices cover adapter diagnostics and host watchdog recovery.
// They are informational and recoverable rather than model-service failures.
const AGENT_RUNTIME_NOTICE_KINDS = new Set([
  'agent_stderr',
  'agent_stall',
  'agent_stall_cancel',
  'agent_stall_settled',
  'agent_stall_restart',
  'agent_session_restarted',
  'agent_session_restarted_fresh',
  'cancel_timeout',
]);

const PRE_TERMINAL_AGENT_NOTICE_KINDS = new Set([
  'agent_stall',
  'agent_stall_cancel',
]);

/**
 * Return the latest agent-side runtime notice.
 *
 * Expiration follows proven recovery, not merely another user message: a
 * restart notice may itself be emitted while processing the next message, so a
 * newer `turn_started` cannot clear it. A later **Completed** turn proves
 * recovery. Pre-terminal `agent_stall` and `agent_stall_cancel` notices clear on
 * any terminal event; settlement, restart, and cancel-timeout outcomes survive
 * interrupted completion to explain why the turn ended. Users may also dismiss.
 */
export function latestAgentRuntimeNotice(events) {
  if (!Array.isArray(events)) return null;
  let notice = null;
  for (const envelope of events) {
    const type = envelope?.event?.type;
    if (type === 'runtime_notice') {
      const kind = String(envelope?.event?.data?.kind || '');
      if (AGENT_RUNTIME_NOTICE_KINDS.has(kind)) notice = envelope;
      continue;
    }
    const turnCompleted = type === 'turn_completed';
    const recovered = turnCompleted
      && String(envelope?.event?.data?.status || '') === 'Completed';
    const preTerminalNoticeEnded = turnCompleted
      && PRE_TERMINAL_AGENT_NOTICE_KINDS.has(String(notice?.event?.data?.kind || ''));
    if ((recovered || preTerminalNoticeEnded)
      && notice
      && Number(envelope.seq || 0) > Number(notice.seq || 0)) {
      notice = null;
    }
  }
  if (!notice) return null;
  const data = notice.event.data || {};
  return {
    kind: String(data.kind || ''),
    detail: String(data.detail || ''),
    key: `${notice.seq || ''}:${notice.timestamp || ''}`,
  };
}
