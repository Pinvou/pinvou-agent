// Round-32 minor 10 (review #455): the round-31 m6 masking helper extracted
// pure — error-phase flows get the localized generic on en/ja
// (showRawErrors=false), zh keeps raw diagnostics, everything else passes
// through untouched.
import assert from 'node:assert/strict';
import { maskedConnectorFlow } from '../src/features/tools/connector_flow_mask.js';

const rawCopy = { showRawErrors: true, actions: { operationFailed: 'GENERIC' } };
const maskedCopy = { showRawErrors: false, actions: { operationFailed: 'GENERIC' } };
const rawErr = 'backend text with credentials-looking noise';

// Error phase + masked locale → generic replaces err; the rest of the flow
// (steps, pct) survives for the card.
{
  const flow = { phase: 'error', err: rawErr, pct: 42, steps: { cli: 'active' } };
  const out = maskedConnectorFlow(flow, maskedCopy);
  assert.strictEqual(out.err, 'GENERIC');
  assert.strictEqual(out.pct, 42);
  assert.strictEqual(out.steps.cli, 'active');
  assert.strictEqual(flow.err, rawErr, 'masking happens at render: flow state keeps the raw text');
}

// zh (showRawErrors=true) keeps the raw diagnostics.
{
  const flow = { phase: 'error', err: rawErr };
  assert.strictEqual(maskedConnectorFlow(flow, rawCopy).err, rawErr);
}

// Non-error phases and null flows pass through untouched (same reference).
{
  const running = { phase: 'running', err: '' };
  assert.strictEqual(maskedConnectorFlow(running, maskedCopy), running);
  assert.strictEqual(maskedConnectorFlow(null, maskedCopy), null);
}

console.log('connector_flow_mask: ok');
