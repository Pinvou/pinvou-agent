import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Static contract: the detached chat window must subscribe to every bridge
// state domain its embedded ChatView consumes. Missing `computerUse` here
// made the consent banner and grant/confirm dialogs unrenderable in detached
// windows while the agent controlled the machine (PR #468 round-10 M1).

const source = (relative) =>
  readFileSync(new URL(`../src/${relative}`, import.meta.url), 'utf8');

const detachedBlock = source('app/DetachedShell.jsx').match(
  /const bs = useBridgeState\(\[([^\]]*)\]\)/,
);
assert.ok(detachedBlock, 'useDetachedBase domain list must exist');
const detachedDomains = detachedBlock[1]
  .split(',')
  .map((entry) => entry.trim().replace(/^'|'$/g, ''))
  .filter(Boolean);

const mainBlock = source('app/main.jsx').match(
  /const APP_BRIDGE_STATE_DOMAINS = \[([^\]]*)\]/,
);
assert.ok(mainBlock, 'APP_BRIDGE_STATE_DOMAINS must exist');
const mainDomains = mainBlock[1]
  .split(',')
  .map((entry) => entry.trim().replace(/^'|'$/g, ''))
  .filter(Boolean);

assert.ok(
  detachedDomains.includes('computerUse'),
  'detached windows must subscribe to the computerUse domain for consent surfaces',
);
for (const domain of detachedDomains) {
  assert.ok(
    mainDomains.includes(domain),
    `detached domain "${domain}" must be declared in APP_BRIDGE_STATE_DOMAINS`,
  );
}
