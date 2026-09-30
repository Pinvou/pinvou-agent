// Connector flow-card failure state (feishu / wecom / dingtalk / tmeet).
// Failures are stored as a stable error code so the flow card renders the
// message in the current UI language; the raw backend diagnostic never
// reaches the card (matching the Official repo's PR #442 behavior).

// Round-37 F3 (review #455): the ONE consent-failure marker string, mirroring
// `scope::CONSENT_SYNC_FAILURE_MARKER` on the Rust side (both pinned by
// consent_marker_frontend.test.mjs + the Rust emitter pins). The three JS
// matchers import this instead of hardcoding the literal, so one-site drift
// cannot survive the pin.
export const CONSENT_SYNC_FAILURE_MARKER =
  'persisting their default-off consent state failed';

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

// Own-key lookup for the flow card's copy. A bare `errors[code]` would resolve
// Object.prototype members (`constructor`, `toString`, `valueOf` all pass the
// snake_case pattern) to inherited functions — truthy, and crashing React when
// rendered. Codes only ever come from this repo's emitters today; this keeps
// the render safe if a future source ever forwards an outside string.
function connectorErrorCopy(errors, code) {
  if (!errors || typeof code !== 'string' || !Object.prototype.hasOwnProperty.call(errors, code)) return '';
  const copy = errors[code];
  return typeof copy === 'string' ? copy : '';
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

export { applyConnectorFailure, connectorErrorCopy, connectorErrorCodeForStep, connectorFailure, connectorUiStep, errorCode };
