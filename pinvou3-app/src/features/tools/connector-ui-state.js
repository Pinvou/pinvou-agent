// Connector flow-card failure state (feishu / wecom / dingtalk / tmeet).
// Failures are stored as a stable error code so the flow card renders the
// message in the current UI language; the raw backend diagnostic never
// reaches the card (matching the Official repo's PR #442 behavior).

// Stable machine-readable error codes for user-facing messages.
// Backends attach a snake_case `code` (or prefix the message with `code: ...`);
// the UI maps the code to a localized string and never shows raw backend text
// that may be in a different language than the current UI.
const ERROR_CODE_PATTERN = /^[a-z][a-z0-9_]*$/;

function errorCode(value) {
  if (value && typeof value === 'object' && typeof value.code === 'string') {
    const code = value.code.trim();
    if (ERROR_CODE_PATTERN.test(code)) return code;
  }

  const message = typeof value === 'string'
    ? value
    : (value && typeof value.message === 'string' ? value.message : '');
  const separator = message.indexOf(':');
  if (separator <= 0) return '';
  const code = message.slice(0, separator).trim();
  return ERROR_CODE_PATTERN.test(code) ? code : '';
}

// Fallback code per flow step when the backend did not attach a code. The map
// covers the flow card's whole step vocabulary: `runtime` / `connect` cannot
// fail today (nothing runs while only those steps are active), but the total
// mapping keeps future failure points on a specific message instead of the
// generic `unknown` copy.
const STEP_ERROR_CODES = {
  runtime: 'runtime_prepare_failed',
  cli: 'cli_install_failed',
  connect: 'auth_start_failed',
  register: 'registration_failed',
  authorize: 'auth_failed',
  qr: 'auth_failed',
};

function connectorErrorCodeForStep(step) {
  return STEP_ERROR_CODES[String(step || '').trim().toLowerCase()] || 'unknown';
}

function connectorFailure(value, step) {
  return {
    errorCode: errorCode(value) || connectorErrorCodeForStep(step),
  };
}

// Map a backend phase onto the flow-card step that should turn red.
// Backend phases: register (feishu app registration) / authorize (QR or
// browser sign-in); UI steps: runtime / cli / connect / qr.
// The phase-only step (null flow) is the canonical step for that phase; the
// applied step may differ once the card's active step has advanced (e.g. a
// register-phase failure after the QR showed still marks `qr` red).
function connectorUiStep(flow, phase) {
  const active = String((flow && flow.active) || '').trim().toLowerCase();
  const normalizedPhase = String(phase || '').trim().toLowerCase();
  if (normalizedPhase === 'authorize') return 'qr';
  if (normalizedPhase === 'register') return active === 'qr' ? 'qr' : 'connect';
  if (['runtime', 'cli', 'connect', 'qr'].includes(normalizedPhase)) return normalizedPhase;
  return active || 'cli';
}

function applyConnectorFailure(flow, value, phase) {
  const current = flow || { steps: {} };
  const step = connectorUiStep(current, phase);
  return {
    ...current,
    phase: 'error',
    ...connectorFailure(value, phase || step),
    steps: { ...current.steps, [step]: 'error' },
  };
}

export { applyConnectorFailure, connectorErrorCodeForStep, connectorFailure, connectorUiStep, errorCode };
