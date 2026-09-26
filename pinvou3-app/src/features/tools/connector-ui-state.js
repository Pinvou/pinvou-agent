// Connector flow-card failure state (feishu / wecom / dingtalk / tmeet).
// Failures are stored as a stable error code so the flow card renders the
// message in the current UI language; the raw backend diagnostic never
// reaches the card (matching the Official repo's PR #442 behavior).
import { errorCode } from '../../shared/user-facing-error.js';

// Fallback code per flow step when the backend did not attach a code.
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
    errStep: step,
    steps: { ...current.steps, [step]: 'error' },
  };
}

export { applyConnectorFailure, connectorErrorCodeForStep, connectorFailure, connectorUiStep };
